"""Shared fixtures. Everything here is offline: no network, no synced DB, no clock dependence."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from edh_scout.cards import CardDB
from edh_scout.deck import parse_decklist

DATA = Path(__file__).parent / "data"
DECKS = DATA / "decks"


@pytest.fixture(scope="session")
def fixture_cards() -> dict:
    """A pinned snapshot of the card records the test decks need (see tools/build_card_fixture.py)."""
    return json.loads((DATA / "cards.json").read_text(encoding="utf-8"))


@pytest.fixture
def db(fixture_cards) -> CardDB:
    return CardDB(fixture_cards["cards"], fixture_cards["aliases"], combos=[],
                  schema=fixture_cards["schema"])


@pytest.fixture
def legacy_db(fixture_cards) -> CardDB:
    """A schema-1 DB: loads fine, but has none of the fields power levels need."""
    stripped = {n: {k: v for k, v in c.items() if k not in ("usd", "rank", "res", "col", "dfc")}
                for n, c in fixture_cards["cards"].items()}
    return CardDB(stripped, fixture_cards["aliases"], combos=[], schema=1)


@pytest.fixture(scope="session")
def golden() -> dict:
    return json.loads((DATA / "powerlevel_golden.json").read_text(encoding="utf-8"))


@pytest.fixture(scope="session")
def curve_cases() -> list:
    return json.loads((DATA / "curve_golden.json").read_text(encoding="utf-8"))


def load_deck(name: str):
    return parse_decklist((DECKS / name).read_text(encoding="utf-8"))


@pytest.fixture
def deck_names() -> list[str]:
    return sorted(p.name for p in DECKS.glob("*.txt"))
