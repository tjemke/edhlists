"""The Deck model and the text formats it is read from and written to."""
from __future__ import annotations

import argparse
import re
import unicodedata
from collections import Counter
from dataclasses import dataclass, field


@dataclass
class Deck:
    commanders: list[str]
    cards: Counter                      # the 99 (commanders excluded)
    ident: str = "deck"
    source: str = ""
    edhrec: str = ""
    meta: dict = field(default_factory=dict)

    def lines(self) -> list[str]:
        return ["Commander", *(f"1 {c}" for c in self.commanders), "", "Deck",
                *(f"{q} {n}" for n, q in sorted(self.cards.items()))]


DECKLINE_RE = re.compile(
    r"^(?:(\d+)\s*x?\s+)?(.+?)"
    r"(?:\s+\([A-Za-z0-9]{2,6}\)(?:\s+[\w★-]+)?)?"   # (SET) 123
    r"(?:\s+\[[^\]]*\])*"                             # [Archidekt categories]
    r"(?:\s+\*[A-Za-z]+\*)*\s*$")                     # *F* / *CMDR*
SECTIONS = {"commander", "commanders", "deck", "main", "mainboard", "sideboard", "maybeboard", "companion"}


def parse_decklist(text: str, commanders: list[str] | None = None) -> Deck:
    cmdrs, cards, section = list(commanders or []), Counter(), None
    for line in text.splitlines():
        s = line.strip()
        if not s or s.startswith(("//", "#")):
            continue
        if s.lower().rstrip(":") in SECTIONS:
            section = s.lower().rstrip(":")
            continue
        m = DECKLINE_RE.match(s)
        if not m or section in ("sideboard", "maybeboard"):
            continue
        qty, name = int(m.group(1) or 1), m.group(2).strip()
        if section in ("commander", "commanders") or "*CMDR*" in s or "[Commander" in s:
            if name not in cmdrs:
                cmdrs.append(name)
        else:
            cards[name] += qty
    for c in cmdrs:
        cards.pop(c, None)
    return Deck(commanders=cmdrs, cards=cards)


def slugify(name: str) -> str:
    s = unicodedata.normalize("NFKD", name.split(" // ")[0]).encode("ascii", "ignore").decode().lower()
    s = s.replace("'", "").replace("+", "plus-")
    return re.sub(r"[^a-z0-9]+", "-", s).strip("-")


def commander_slug(commanders: list[str]) -> str:
    # Partner pages appear to be ordered alphabetically -- pass --slug if EDHREC disagrees.
    return "-".join(sorted(slugify(c) for c in commanders))


def bracket_spec(spec: str) -> set[int]:
    """A declared-bracket selector: '3' -> {3}, '1-3' -> {1,2,3}, '2,4' -> {2,4}."""
    out = set()
    for part in spec.split(","):
        part = part.strip()
        if m := re.fullmatch(r"(\d)\s*-\s*(\d)", part):
            lo, hi = sorted(int(g) for g in m.groups())
            out.update(range(lo, hi + 1))
        elif part.isdigit():
            out.add(int(part))
        else:
            raise argparse.ArgumentTypeError(f"bad bracket {part!r} -- use e.g. 3, 1-3 or 2,4")
    if not out <= {1, 2, 3, 4, 5}:
        raise argparse.ArgumentTypeError(f"brackets must be 1-5, got {sorted(out - {1, 2, 3, 4, 5})}")
    return out


def fmt_brackets(want: set[int]) -> str:
    return "B" + ",".join(map(str, sorted(want)))
