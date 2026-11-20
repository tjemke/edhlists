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

/// Write `path` with `write`, into a temporary file beside it that then replaces it whole. A crash
/// never leaves a half-written file behind, and runs writing the same file at once each write
/// their own temporary file: the last one to finish wins, never a mix of them. The temporary file
/// is removed if the write fails.
pub fn atomic_write_with<T>(path: &Path, write: impl FnOnce(&mut File) -> Result<T>) -> Result<T> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let prefix = format!(".{}.", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
    let mut temp = tempfile::Builder::new();
    temp.prefix(&prefix).suffix(".part");
    // As readable as a file written directly (tempfile's default is owner-only).
    #[cfg(unix)]
    temp.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o666));
    let mut tmp = temp.tempfile_in(dir).with_context(|| format!("creating a temporary file in {}", dir.display()))?;
    let out = write(tmp.as_file_mut()).with_context(|| format!("writing {}", path.display()))?;
    tmp.persist(path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(out)
}

/// [`atomic_write_with`] these bytes.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with(path, |f| Ok(f.write_all(bytes)?))
}

pub fn write_json_gz<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    atomic_write_with(path, |f| {
        let mut enc = GzEncoder::new(BufWriter::new(f), Compression::default());
        serde_json::to_writer(&mut enc, value)?;
        enc.finish()?.flush()?;
        Ok(())
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn runs_writing_one_file_at_once_never_mix_their_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("cards.json.gz"));
        let writers = 8;
        let start = Arc::new(Barrier::new(writers));
        let threads: Vec<_> = (0..writers)
            .map(|i| {
                let (path, start) = (Arc::clone(&path), Arc::clone(&start));
                std::thread::spawn(move || {
                    // Each writer's content has its own byte and length, so a mix of two shows.
                    let mine = vec![b'a' + i as u8; 4096 * (i + 1)];
                    start.wait();
                    for _ in 0..50 {
                        atomic_write(&path, &mine).unwrap_or_else(|e| panic!("writer {i} failed: {e:#}"));
                        let got = fs::read(&*path).unwrap();
                        let whole = got.first().is_some_and(|&b| {
                            got.iter().all(|&c| c == b) && got.len() == 4096 * (usize::from(b - b'a') + 1)
                        });
                        assert!(whole, "writer {i} read back {} bytes that aren't one writer's whole file", got.len());
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 1, "temporary files were left behind: {left:?}");
    }
}
