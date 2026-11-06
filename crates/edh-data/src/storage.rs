//! Small file helpers: gzip sniffing, atomic writes, gzipped JSON.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Open a file for reading, transparently decompressing it if it starts with the gzip magic.
pub fn open_maybe_gz(path: &Path) -> Result<Box<dyn BufRead>> {
    let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut magic = [0u8; 2];
    let n = f.read(&mut magic)?;
    drop(f);
    let f = File::open(path)?;
    Ok(if n == 2 && magic == [0x1f, 0x8b] {
        Box::new(BufReader::with_capacity(1 << 20, MultiGzDecoder::new(f)))
    } else {
        Box::new(BufReader::with_capacity(1 << 20, f))
    })
}

/// Write via a temporary sibling and rename, so a crash never leaves a half-written file behind.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn write_json_gz<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part");
    {
        let f = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        let mut enc = GzEncoder::new(BufWriter::new(f), Compression::default());
        serde_json::to_writer(&mut enc, value)?;
        enc.finish()?.flush()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_json_gz<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let r = open_maybe_gz(path)?;
    serde_json::from_reader(r).with_context(|| format!("parsing {}", path.display()))
}

/// Age of a file in whole days, if it exists.
pub fn age_days(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.elapsed().ok()?.as_secs() / 86_400)
}
