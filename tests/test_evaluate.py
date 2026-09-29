"""Bracket triage: the floor, the reasons behind it, and the "cut N Game Changers" hint."""
from __future__ import annotations

from collections import Counter

import pytest

from edh_scout.cards import CardDB
from edh_scout.deck import Deck
from edh_scout.evaluate import Result, deck_stats, evaluate
from edh_scout.fetch import Http

from .conftest import load_deck


@pytest.fixture
def offline() -> Http:
    return Http(offline=True)


def make_db(cards: dict, combos=()) -> CardDB:
    aliases = {n.lower(): n for n in cards}
    return CardDB(cards, aliases, list(combos), schema=2)


def card(**kw) -> dict:
    base = {"gc": False, "ban": False, "mld": False, "xt": False, "tut": False, "ramp": False,
            "mv": 2.0, "land": False, "usd": 1.0, "rank": 500, "res": False, "col": ["U"],
            "dfc": False}
    base.update(kw)
    return base


# --- the floor -----------------------------------------------------------------------------------

def test_clean_deck_floors_at_one(offline):
    db = make_db({"Harmless": card()})
    r = evaluate(Deck(commanders=[], cards=Counter({"Harmless": 1})), db, offline)
    assert r.floor == 1


def test_a_single_game_changer_lifts_to_three(offline):
    db = make_db({"Changer": card(gc=True)})
    r = evaluate(Deck(commanders=[], cards=Counter({"Changer": 1})), db, offline)
    assert r.floor == 3
    assert r.game_changers == ["Changer"]


def test_more_than_three_game_changers_lifts_to_four(offline):
    db = make_db({f"GC{i}": card(gc=True) for i in range(4)})
    r = evaluate(Deck(commanders=[], cards=Counter({f"GC{i}": 1 for i in range(4)})), db, offline)
    assert r.floor == 4


def test_mass_land_denial_lifts_to_four(offline):
    db = make_db({"Armageddon": card(mld=True)})
    r = evaluate(Deck(commanders=[], cards=Counter({"Armageddon": 1})), db, offline)
    assert r.floor == 4


def test_one_extra_turn_is_two_but_several_is_four(offline):
    db = make_db({"Walk": card(xt=True), "Warp": card(xt=True)})
    one = evaluate(Deck(commanders=[], cards=Counter({"Walk": 1})), db, offline)
    two = evaluate(Deck(commanders=[], cards=Counter({"Walk": 1, "Warp": 1})), db, offline)
    assert one.floor == 2
    assert two.floor == 4


def test_a_banned_card_overrides_everything(offline):
    db = make_db({"Crypt": card(ban=True)})
    r = evaluate(Deck(commanders=[], cards=Counter({"Crypt": 1})), db, offline)
    assert r.floor == 99
    assert r.banned == ["Crypt"]


def test_combo_severity_feeds_the_floor(offline):
    combos = [["c1", "R", ["Alpha", "Beta"], ["Infinite mana"]]]
    db = make_db({"Alpha": card(), "Beta": card()}, combos)
    r = evaluate(Deck(commanders=[], cards=Counter({"Alpha": 1, "Beta": 1})), db, offline)
    assert r.floor == 4
    assert r.combos[0][0] == "R"


def test_unknown_names_are_collected(offline):
    db = make_db({"Known": card()})
    r = evaluate(Deck(commanders=[], cards=Counter({"Known": 1, "Mystery Card": 1})), db, offline)
    assert r.unknown == ["Mystery Card"]


# --- power level wiring --------------------------------------------------------------------------

def test_power_level_is_computed_by_default(db, offline):
    r = evaluate(load_deck("atraxa_four_colour.txt"), db, offline)
    assert r.power is not None
    assert r.power.level > 0


def test_power_level_can_be_switched_off(db, offline):
    r = evaluate(load_deck("atraxa_four_colour.txt"), db, offline, power_level=False)
    assert r.power is None


def test_power_level_suppressed_on_a_legacy_db(legacy_db, offline):
    r = evaluate(load_deck("atraxa_four_colour.txt"), legacy_db, offline)
    assert r.power is None


# --- deck_stats ----------------------------------------------------------------------------------

def test_deck_stats_counts_lands_and_averages_only_nonlands():
    db = make_db({"Forest": card(land=True, mv=0.0), "Bear": card(mv=2.0), "Dragon": card(mv=6.0)})
    deck = Deck(commanders=["Bear"], cards=Counter({"Forest": 10, "Bear": 1, "Dragon": 1}))
    canon = {n: n for n in ["Forest", "Bear", "Dragon"]}
    stats = deck_stats(deck, canon, db)
    assert stats["lands"] == 10
    assert stats["avg_mv"] == 4.0          # (2 + 6) / 2, lands excluded
    assert stats["cards"] == 13            # 12 in the 99 plus one commander


def test_deck_stats_handles_a_landless_deck():
    db = make_db({"Bear": card(mv=2.0)})
    stats = deck_stats(Deck(commanders=[], cards=Counter({"Bear": 1})), {"Bear": "Bear"}, db)
    assert stats["avg_mv"] == 2.0


# --- reasons and hints ---------------------------------------------------------------------------

def test_reasons_lists_each_signal():
    r = Result(banned=["Crypt"], game_changers=["Rhystic Study"], mld=["Armageddon"],
               extra_turns=["Time Warp"])
    text = "\n".join(r.reasons())
    assert "BANNED: Crypt" in text
    assert "1 GC: Rhystic Study" in text
    assert "MLD? Armageddon" in text        # offline engine hedges with a question mark
    assert "extra turns: Time Warp" in text


def test_reasons_marks_mld_as_certain_when_spellbook_answered():
    assert "MLD: Armageddon" in "\n".join(Result(mld=["Armageddon"], engine="spellbook").reasons())


def test_reasons_truncates_long_combo_lists():
    combos = [("R", [f"A{i}", f"B{i}"], ["Win"]) for i in range(6)]
    lines = Result(combos=combos).reasons()
    assert sum(1 for line in lines if line.startswith("combo")) == 3
    assert "... +3 more combos" in lines[-1]


def test_hint_suggests_cutting_game_changers():
    r = Result(floor=4, game_changers=["a", "b", "c", "d", "e", "f"],
               parts={"gc": 4, "mld": 1, "combo": 1})
    assert r.hint(3) == "cut 3 Game Changer(s) to fit B3"


def test_no_hint_when_the_deck_already_fits():
    assert Result(floor=2, game_changers=["a"], parts={"gc": 2}).hint(3) == ""


def test_no_hint_when_something_other_than_game_changers_is_the_problem():
    r = Result(floor=4, game_changers=["a"], parts={"gc": 3, "mld": 4})
    assert r.hint(3) == ""
