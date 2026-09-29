#!/usr/bin/env python3
"""Emit the per-card JSON that tools/edhpl_reference.js scores, for the decks in tests/data/decks.

Reads only the committed fixture, so it needs no network and no synced DB. See tools/README.md.

    python tools/dump_reference_input.py > /tmp/decks.json
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "src"))

from edh_scout.deck import parse_decklist  # noqa: E402

DECKS = ROOT / "tests" / "data" / "decks"
FIXTURE = ROOT / "tests" / "data" / "cards.json"


def deck_payload(deck, cards: dict, aliases: dict) -> list[dict]:
    entries = [(c, 1, True) for c in deck.commanders] + [(n, q, False) for n, q in deck.cards.items()]
    out = []
    for name, qty, is_commander in entries:
        key = name.strip().lower()
        canon = aliases.get(key) or aliases.get(key.split(" // ")[0])
        card = cards.get(canon or "")
        if not card:
            continue
        out.append({
            "name": canon,
            "quantity": qty,
            "isCommander": is_commander,
            "cmc": card["mv"],
            "price": card["usd"],
            # The reference mirrors the site's JS, which has no notion of a missing rank; the Python
            # port maps that to the bottom of the popularity curve, so feed it the same value.
            "edhrec_rank": card["rank"] if card["rank"] is not None else 27000,
            "reserved": card["res"],
            "colors": card["col"],
            "layout": "modal_dfc" if card["dfc"] else "normal",
            # The reference only reads type_line to decide "is this a land", which the fixture
            # already answers.
            "type_line": "Land" if card["land"] else "Creature",
        })
    return out


def main() -> None:
    fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))
    cards, aliases = fixture["cards"], fixture["aliases"]
    payload = [deck_payload(parse_decklist(p.read_text(encoding="utf-8")), cards, aliases)
               for p in sorted(DECKS.glob("*.txt"))]
    json.dump(payload, sys.stdout)


if __name__ == "__main__":
    main()
