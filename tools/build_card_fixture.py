#!/usr/bin/env python3
"""Rebuild tests/data/cards.json: the card records the test decks need, and nothing else.

The test suite must never touch the network -- prices and EDHREC ranks move daily, and golden
power-level values would rot within a week. So the fixture pins a snapshot of exactly the cards
referenced by tests/data/decks/*.txt.

Requires a synced card DB (`edh-scout sync`). Run from the repo root:

    python tools/build_card_fixture.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "src"))

from edh_scout.config import DB_CARDS  # noqa: E402
from edh_scout.deck import parse_decklist  # noqa: E402
from edh_scout.storage import load_gz  # noqa: E402

DECKS = ROOT / "tests" / "data" / "decks"
OUT = ROOT / "tests" / "data" / "cards.json"


def main() -> None:
    if not DB_CARDS.exists():
        sys.exit(f"no card DB at {DB_CARDS} -- run `edh-scout sync` first")
    db = load_gz(DB_CARDS)
    cards, aliases = db["cards"], db["aliases"]

    wanted: set[str] = set()
    for path in sorted(DECKS.glob("*.txt")):
        deck = parse_decklist(path.read_text(encoding="utf-8"))
        wanted.update(deck.commanders)
        wanted.update(deck.cards)
    # A couple of cards the unit tests reference directly for their override behaviour.
    wanted.update(["Sol Ring", "Gaea's Cradle", "Force of Will", "Mystic Remora",
                   "Agadeem's Awakening // Agadeem, the Undercrypt", "Wastes", "Snow-Covered Forest"])

    out_cards, out_aliases, missing = {}, {}, []
    for name in sorted(wanted):
        canon = aliases.get(name.strip().lower()) or aliases.get(name.strip().lower().split(" // ")[0])
        if canon is None or canon not in cards:
            missing.append(name)
            continue
        out_cards[canon] = cards[canon]
        out_aliases[name.strip().lower()] = canon
        out_aliases[canon.lower()] = canon
        for face in canon.split(" // "):
            out_aliases.setdefault(face.lower(), canon)

    OUT.write_text(json.dumps(
        {"schema": db.get("schema", 2), "cards": out_cards, "aliases": out_aliases},
        indent=1, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(out_cards)} cards, {len(out_aliases)} aliases")
    if missing:
        print(f"note: {len(missing)} name(s) not in the card DB (left for the missing-card tests): "
              + ", ".join(missing[:5]))


if __name__ == "__main__":
    main()
