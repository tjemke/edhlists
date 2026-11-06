//! A deliberately slow, sequential, cached HTTP client.
//!
//! Rules this module enforces for every caller:
//! * one request at a time, with a per-host minimum gap ([`crate::config::host_gap`]);
//! * every response is cached on disk; a fresh cache entry means no request at all;
//! * `Retry-After` is honoured, and a host that rate-limits us twice is not contacted again
//!   for the rest of the run (callers get [`HostBlocked`] and carry on with what they have);
//! * if the network fails but a stale cache entry exists, the stale copy is used;
//! * `--offline` never touches the network.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::config::{host_gap, user_agent};
use crate::storage::atomic_write;

/// `--offline` was given and the thing asked for isn't cached.
#[derive(Debug)]
pub struct OfflineMiss(pub String);
impl fmt::Display for OfflineMiss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "offline and not cached: {}", self.0)
    }
}
impl std::error::Error for OfflineMiss {}

/// The host told us to slow down more than once; we stopped talking to it for this run.
#[derive(Debug)]
pub struct HostBlocked(pub String);
impl fmt::Display for HostBlocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} asked us to back off; no further requests to it this run", self.0)
    }
}
impl std::error::Error for HostBlocked {}

/// A non-success HTTP status.
#[derive(Debug)]
pub struct HttpStatus {
    pub url: String,
    pub code: u16,
}
impl fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HTTP {} for {}", self.code, self.url)
    }
}
impl std::error::Error for HttpStatus {}

/// How long a cached response stays good.
#[derive(Clone, Copy, Debug)]
pub enum Ttl {
    Forever,
    For(Duration),
}

pub struct Http {
    agent: ureq::Agent,
    offline: bool,
    cache_dir: PathBuf,
    last: RefCell<HashMap<String, Instant>>,
    counts: RefCell<BTreeMap<String, u32>>,
    strikes: RefCell<HashMap<String, u32>>,
    blocked: RefCell<HashSet<String>>,
    gap_override: RefCell<HashMap<String, Duration>>,
}

const MAX_ATTEMPTS: u32 = 3;
const MAX_RETRY_AFTER: u64 = 120;
const BODY_LIMIT: u64 = 64 << 20;

pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or("").split(':').next().unwrap_or("").to_ascii_lowercase()
}

impl Http {
    pub fn new(cache_dir: PathBuf, offline: bool) -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(user_agent())
            .timeout_global(Some(Duration::from_secs(300)))
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
            offline,
            cache_dir,
            last: RefCell::default(),
            counts: RefCell::default(),
            strikes: RefCell::default(),
            blocked: RefCell::default(),
            gap_override: RefCell::default(),
        }
    }

    pub fn offline(&self) -> bool {
        self.offline
    }

    /// Slow a host down further than the default (never speeds it up).
    pub fn set_min_gap(&self, host: &str, gap: Duration) {
        if gap > host_gap(host) {
            self.gap_override.borrow_mut().insert(host.to_string(), gap);
        }
    }

    /// Requests actually sent this run, per host.
    pub fn request_counts(&self) -> BTreeMap<String, u32> {
        self.counts.borrow().clone()
    }

    pub fn is_blocked(&self, host: &str) -> bool {
        self.blocked.borrow().contains(host)
    }

    fn pace(&self, host: &str) {
        let gap = self.gap_override.borrow().get(host).copied().unwrap_or_else(|| host_gap(host));
        if let Some(prev) = self.last.borrow().get(host) {
            let since = prev.elapsed();
            if since < gap {
                std::thread::sleep(gap - since);
            }
        }
        self.last.borrow_mut().insert(host.to_string(), Instant::now());
    }

    /// Send one GET with pacing and retries; returns the successful response.
    fn send(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        if self.offline {
            return Err(OfflineMiss(url.to_string()).into());
        }
        let host = host_of(url);
        if self.is_blocked(&host) {
            return Err(HostBlocked(host).into());
        }
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.pace(&host);
            *self.counts.borrow_mut().entry(host.clone()).or_default() += 1;
            let result = self.agent.get(url).header("Accept", "application/json, text/html;q=0.9, */*;q=0.5").call();
            let resp = match result {
                Ok(r) => r,
                Err(e) if attempt < MAX_ATTEMPTS => {
                    let wait = 2u64.pow(attempt);
                    eprintln!("  network error for {url} ({e}); retrying in {wait}s");
                    std::thread::sleep(Duration::from_secs(wait));
                    continue;
                }
                Err(e) => return Err(e).with_context(|| format!("GET {url}")),
            };
            let code = resp.status().as_u16();
            if (200..300).contains(&code) {
                return Ok(resp);
            }
            if code == 429 || code == 503 {
                let strikes = {
                    let mut s = self.strikes.borrow_mut();
                    let n = s.entry(host.clone()).or_default();
                    *n += 1;
                    *n
                };
                if strikes >= 2 {
                    self.blocked.borrow_mut().insert(host.clone());
                    return Err(HostBlocked(host).into());
                }
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(30)
                    .min(MAX_RETRY_AFTER);
                eprintln!("  {host} answered {code}; waiting {wait}s as asked");
                std::thread::sleep(Duration::from_secs(wait));
                continue;
            }
            if code >= 500 && attempt < MAX_ATTEMPTS {
                std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
                continue;
            }
            return Err(HttpStatus { url: url.to_string(), code }.into());
        }
    }

    /// GET a URL into memory, uncached.
    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut resp = self.send(url)?;
        Ok(resp.body_mut().with_config().limit(BODY_LIMIT).read_to_vec()?)
    }

    /// Stream a (large) download to `dest`, atomically.
    pub fn download(&self, url: &str, dest: &Path) -> Result<u64> {
        let resp = self.send(url)?;
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = dest.with_extension("part");
        let mut out = File::create(&tmp)?;
        let mut reader = resp.into_body().into_reader();
        let n = io::copy(&mut reader, &mut out)?;
        out.flush()?;
        fs::rename(&tmp, dest)?;
        Ok(n)
    }

    fn cache_path(&self, key: &str) -> PathBuf {
        let digest = Sha256::digest(key.as_bytes());
        let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
        self.cache_dir.join(&hex[..2]).join(hex)
    }

    /// Is there a cache entry for `key` at all (fresh or not)?
    pub fn is_cached(&self, key: &str) -> bool {
        self.cache_path(key).exists()
    }

    /// GET text through the disk cache. `key` defaults to the URL.
    pub fn cached_text(&self, url: &str, key: Option<&str>, ttl: Ttl) -> Result<String> {
        let path = self.cache_path(key.unwrap_or(url));
        let fresh = match (ttl, fs::metadata(&path).and_then(|m| m.modified())) {
            (_, Err(_)) => None,
            (Ttl::Forever, Ok(_)) => Some(true),
            (Ttl::For(d), Ok(t)) => Some(SystemTime::now().duration_since(t).map(|age| age < d).unwrap_or(true)),
        };
        if fresh == Some(true) || (fresh.is_some() && self.offline) {
            return Ok(fs::read_to_string(&path)?);
        }
        match self.get_bytes(url) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes).with_context(|| format!("non-UTF-8 body from {url}"))?;
                atomic_write(&path, text.as_bytes())?;
                Ok(text)
            }
            Err(e) if fresh.is_some() && e.downcast_ref::<HttpStatus>().is_none_or(|s| s.code != 404) => {
                eprintln!("  using stale cached copy of {url} ({e})");
                Ok(fs::read_to_string(&path)?)
            }
            Err(e) => Err(e),
        }
    }

    pub fn cached_json<T: serde::de::DeserializeOwned>(&self, url: &str, key: Option<&str>, ttl: Ttl) -> Result<T> {
        let text = self.cached_text(url, key, ttl)?;
        serde_json::from_str(&text).with_context(|| format!("unexpected JSON from {url}"))
    }
}

/// Render "edhrec.com 61, json.edhrec.com 2" for the end-of-run summary.
pub fn summarize_counts(counts: &BTreeMap<String, u32>) -> String {
    if counts.is_empty() {
        return "no network requests (everything was cached)".into();
    }
    let total: u32 = counts.values().sum();
    let parts: Vec<String> = counts.iter().map(|(h, n)| format!("{h} {n}")).collect();
    format!("{total} network request(s): {}", parts.join(", "))
}

pub fn ensure_online(http: &Http, what: &str) -> Result<()> {
    if http.offline() {
        bail!("{what} needs the network, but --offline is set");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_parsing() {
        assert_eq!(host_of("https://edhrec.com/deckpreview/x"), "edhrec.com");
        assert_eq!(host_of("https://json.edhrec.com/pages/decks/a.json"), "json.edhrec.com");
        assert_eq!(host_of("http://Example.com:8080?q"), "example.com");
    }

    #[test]
    fn offline_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let http = Http::new(dir.path().to_path_buf(), true);
        let err = http.cached_text("https://example.com/a", None, Ttl::Forever).unwrap_err();
        assert!(err.downcast_ref::<OfflineMiss>().is_some());
        let p = http.cache_path("https://example.com/a");
        atomic_write(&p, b"hello").unwrap();
        // offline: even an expired entry is served
        assert_eq!(http.cached_text("https://example.com/a", None, Ttl::For(Duration::ZERO)).unwrap(), "hello");
        assert!(http.request_counts().is_empty());
    }
}
