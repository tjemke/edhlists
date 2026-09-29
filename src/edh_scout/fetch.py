"""Polite, file-cached HTTP. Every response we fetch is cached; --offline reads the cache only."""
from __future__ import annotations

import hashlib
import json
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from .config import CACHE, UA
from .storage import gunzip_if_needed


class OfflineMiss(Exception):
    """Raised when --offline is set and the requested thing isn't cached."""


class Http:
    GAP = {"edhrec.com": 1.0, "json.edhrec.com": 1.0,       # seconds between requests per host
           "backend.commanderspellbook.com": 0.8, "api.scryfall.com": 0.12}

    def __init__(self, offline: bool = False, user_agent: str = UA):
        self.offline, self.ua, self._last = offline, user_agent, {}

    def _open(self, url, data=None, headers=None, retries=4):
        if self.offline:
            raise OfflineMiss(url)
        host = urllib.parse.urlsplit(url).hostname or ""
        hdrs = {"User-Agent": self.ua, "Accept": "application/json, */*;q=0.5", **(headers or {})}
        for attempt in range(retries):
            wait = self._last.get(host, 0) + self.GAP.get(host, 0.5) - time.monotonic()
            if wait > 0:
                time.sleep(wait)
            self._last[host] = time.monotonic()
            try:
                req = urllib.request.Request(url, data=data, headers=hdrs)
                return urllib.request.urlopen(req, timeout=120)
            except urllib.error.HTTPError as e:
                if e.code in (429, 500, 502, 503, 504) and attempt + 1 < retries:
                    time.sleep(float(e.headers.get("Retry-After") or 2 ** (attempt + 1)))
                    continue
                raise
        raise RuntimeError("unreachable")

    def get(self, url, data=None, headers=None) -> bytes:
        with self._open(url, data, headers) as r:
            return r.read()

    def download(self, url, dest: Path) -> None:
        dest.parent.mkdir(parents=True, exist_ok=True)
        tmp = dest.with_name(dest.name + ".part")
        with self._open(url) as r, open(tmp, "wb") as f:
            while chunk := r.read(1 << 20):
                f.write(chunk)
        tmp.replace(dest)

    @staticmethod
    def _path(key: str) -> Path:
        return CACHE / "http" / (hashlib.sha256(key.encode()).hexdigest()[:32] + ".json")

    def peek(self, key: str):
        p = self._path(key)
        return json.loads(p.read_text()) if p.exists() else None

    def json(self, url, *, ttl=None, key=None, data=None, headers=None):
        """GET/POST JSON through a file cache. ttl=None caches forever; --offline reads cache only."""
        path = self._path(key or url)
        if path.exists() and (self.offline or ttl is None or time.time() - path.stat().st_mtime < ttl):
            return json.loads(path.read_text())
        obj = json.loads(gunzip_if_needed(self.get(url, data, headers)))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(obj))
        return obj
