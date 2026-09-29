"""Compressed and streaming IO: the bulk dumps are far too big to hold in memory."""
from __future__ import annotations

import gzip
import io
import json
import re
from pathlib import Path


def gunzip_if_needed(b: bytes) -> bytes:
    while b[:2] == b"\x1f\x8b":
        b = gzip.decompress(b)
    return b


def open_text(path: Path) -> io.TextIOBase:
    with open(path, "rb") as f:
        gz = f.read(2) == b"\x1f\x8b"
    return io.TextIOWrapper(gzip.open(path, "rb") if gz else open(path, "rb"), encoding="utf-8")


def iter_json_array(fp: io.TextIOBase, key: str | None = None, chunk: int = 1 << 20):
    """Stream the items of a big JSON array (top-level, or under top-level `key`) without
    loading the whole document -- the Spellbook dump is several hundred MB uncompressed."""
    dec, buf, pos = json.JSONDecoder(), "", 0

    def fill() -> bool:
        nonlocal buf, pos
        data = fp.read(chunk)
        if not data:
            return False
        buf, pos = buf[pos:] + data, 0
        return True

    start = re.compile(r'"%s"\s*:\s*\[' % re.escape(key) if key else r"\s*\[")
    while not (m := start.search(buf) if key else start.match(buf)):
        if not fill():
            raise ValueError(f"JSON array {key or '<root>'} not found")
    pos = m.end()
    sep = re.compile(r"[\s,]*")
    while True:
        pos = sep.match(buf, pos).end()
        if pos >= len(buf):
            if not fill():
                return
            continue
        if buf[pos] == "]":
            return
        try:
            obj, end = dec.raw_decode(buf, pos)
        except json.JSONDecodeError:
            if not fill():
                raise
            continue
        yield obj
        pos = end


def iter_jsonl(fp: io.TextIOBase):
    """Stream a JSON Lines document (one object per line)."""
    for line in fp:
        line = line.strip().rstrip(",")
        if line.startswith("{"):
            yield json.loads(line)


def iter_bulk_cards(path: Path):
    """Scryfall ships bulk data as gzipped JSON Lines; older dumps were one big JSON array."""
    with open_text(path) as fp:
        is_array = fp.read(64).lstrip().startswith("[")
    return iter_json_array(open_text(path)) if is_array else iter_jsonl(open_text(path))


def save_gz(path: Path, obj) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with gzip.open(path, "wt", encoding="utf-8") as f:
        json.dump(obj, f, separators=(",", ":"))


def load_gz(path: Path):
    with gzip.open(path, "rt", encoding="utf-8") as f:
        return json.load(f)
