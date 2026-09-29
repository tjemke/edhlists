"""Streaming and compressed IO. The chunk-boundary cases are the ones that actually bite."""
from __future__ import annotations

import gzip
import io
import json

import pytest

from edh_scout.storage import (
    gunzip_if_needed,
    iter_bulk_cards,
    iter_json_array,
    iter_jsonl,
    load_gz,
    open_text,
    save_gz,
)


def text(s: str) -> io.StringIO:
    return io.StringIO(s)


# --- iter_json_array -----------------------------------------------------------------------------

def test_top_level_array():
    assert list(iter_json_array(text('[{"a":1},{"a":2}]'))) == [{"a": 1}, {"a": 2}]


def test_empty_array():
    assert list(iter_json_array(text("[]"))) == []


def test_array_under_a_key():
    doc = '{"meta": 1, "variants": [{"id": "x"}, {"id": "y"}]}'
    assert list(iter_json_array(text(doc), key="variants")) == [{"id": "x"}, {"id": "y"}]


def test_missing_key_raises():
    with pytest.raises(ValueError, match="variants"):
        list(iter_json_array(text('{"other": []}'), key="variants"))


@pytest.mark.parametrize("chunk", [1, 2, 3, 7, 16, 1024])
def test_survives_every_chunk_boundary(chunk):
    """The reader refills mid-object; objects must never be split or dropped."""
    records = [{"id": i, "name": f"Card {i}", "nested": {"x": [1, 2, 3]}} for i in range(25)]
    doc = json.dumps({"variants": records})
    assert list(iter_json_array(text(doc), key="variants", chunk=chunk)) == records


@pytest.mark.parametrize("chunk", [1, 4, 64])
def test_top_level_array_across_chunks(chunk):
    records = [{"id": i} for i in range(20)]
    assert list(iter_json_array(text(json.dumps(records)), chunk=chunk)) == records


def test_whitespace_and_newlines_between_records():
    assert list(iter_json_array(text('[\n  {"a": 1},\n\n  {"a": 2}\n]'))) == [{"a": 1}, {"a": 2}]


# --- iter_jsonl ----------------------------------------------------------------------------------

def test_json_lines():
    assert list(iter_jsonl(text('{"a":1}\n{"a":2}\n'))) == [{"a": 1}, {"a": 2}]


def test_json_lines_skips_blank_and_bracket_lines():
    """Some dumps wrap the lines in an array; the object lines are still the payload."""
    assert list(iter_jsonl(text('[\n{"a":1},\n{"a":2}\n]\n'))) == [{"a": 1}, {"a": 2}]


# --- iter_bulk_cards -----------------------------------------------------------------------------

def test_bulk_cards_reads_json_lines(tmp_path):
    p = tmp_path / "bulk.jsonl"
    p.write_text('{"name":"A"}\n{"name":"B"}\n', encoding="utf-8")
    assert [c["name"] for c in iter_bulk_cards(p)] == ["A", "B"]


def test_bulk_cards_reads_a_json_array(tmp_path):
    p = tmp_path / "bulk.json"
    p.write_text('[{"name":"A"},{"name":"B"}]', encoding="utf-8")
    assert [c["name"] for c in iter_bulk_cards(p)] == ["A", "B"]


def test_bulk_cards_reads_gzipped_json_lines(tmp_path):
    p = tmp_path / "bulk.jsonl.gz"
    with gzip.open(p, "wt", encoding="utf-8") as f:
        f.write('{"name":"A"}\n{"name":"B"}\n')
    assert [c["name"] for c in iter_bulk_cards(p)] == ["A", "B"]


def test_bulk_cards_reads_a_gzipped_array(tmp_path):
    p = tmp_path / "bulk.json.gz"
    with gzip.open(p, "wt", encoding="utf-8") as f:
        f.write('[{"name":"A"}]')
    assert [c["name"] for c in iter_bulk_cards(p)] == ["A"]


# --- open_text / gz helpers ----------------------------------------------------------------------

def test_open_text_handles_plain_and_gzipped(tmp_path):
    plain, gz = tmp_path / "a.txt", tmp_path / "b.txt.gz"
    plain.write_text("hello", encoding="utf-8")
    with gzip.open(gz, "wt", encoding="utf-8") as f:
        f.write("hello")
    assert open_text(plain).read() == "hello"
    assert open_text(gz).read() == "hello"


def test_gunzip_if_needed_passes_plain_bytes_through():
    assert gunzip_if_needed(b"plain") == b"plain"


def test_gunzip_if_needed_unwraps():
    assert gunzip_if_needed(gzip.compress(b"payload")) == b"payload"


def test_gunzip_if_needed_unwraps_double_compression():
    assert gunzip_if_needed(gzip.compress(gzip.compress(b"payload"))) == b"payload"


def test_save_and_load_round_trip(tmp_path):
    obj = {"cards": {"Sol Ring": {"mv": 1.0}}, "built": 123.0}
    path = tmp_path / "nested" / "db.json.gz"
    save_gz(path, obj)
    assert path.exists()          # parent directory created on demand
    assert load_gz(path) == obj


def test_save_gz_writes_actual_gzip(tmp_path):
    path = tmp_path / "db.json.gz"
    save_gz(path, {"a": 1})
    assert path.read_bytes()[:2] == b"\x1f\x8b"
