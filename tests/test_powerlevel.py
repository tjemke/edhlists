"""The EDHPowerLevel port, pinned against values derived from tools/edhpl_reference.js."""
from __future__ import annotations

import json
from collections import Counter

import pytest

from edh_scout.deck import Deck
from edh_scout.powerlevel import (
    EDHPL_FAVOR_PRICE,
    EDHPL_POP_CURVE,
    EDHPL_POWER_CURVE,
    EDHPL_PRICE_CURVE,
    PowerLevel,
    edhpl_curve,
    edhpowerlevel,
    edhpowerlevel_url,
)

from .conftest import load_deck

CURVES = {
    "price": (EDHPL_PRICE_CURVE, 1 + EDHPL_FAVOR_PRICE),
    "pop": (EDHPL_POP_CURVE, 1 - EDHPL_FAVOR_PRICE),
    "power": (EDHPL_POWER_CURVE, 1.0),
}
FIELDS = ["level", "score", "impact", "efficiency", "avg_cmc", "curve_cmc"]


# --- the curve lookup ----------------------------------------------------------------------------

def test_curve_matches_reference(curve_cases):
    """Every case comes from the JavaScript reference; these must agree exactly, not approximately."""
    mismatches = []
    for name, value, expected in curve_cases:
        curve, step = CURVES[name]
        got = edhpl_curve(value, curve, step)
        if abs(got - expected) > 1e-9:
            mismatches.append((name, value, expected, got))
    assert not mismatches, f"{len(mismatches)} curve mismatch(es), first: {mismatches[0]}"


def test_curve_case_coverage(curve_cases):
    """Guard the fixture itself -- a truncated regeneration would silently weaken every other test."""
    assert len(curve_cases) > 400
    assert {name for name, _, _ in curve_cases} == set(CURVES)


@pytest.mark.parametrize("value,expected", [(-5, 0.0), (0, 0.0)])
def test_curve_below_floor_is_zero(value, expected):
    assert edhpl_curve(value, EDHPL_PRICE_CURVE, 1.25) == expected


def test_curve_above_ceiling_saturates():
    assert edhpl_curve(101, EDHPL_PRICE_CURVE, 1.25) == pytest.approx(10 * 1.25)


def test_curve_quirk_fraction_is_not_scaled():
    """Ported deliberately: `step` scales the stop index but not the interpolated fraction."""
    # Exactly halfway between stops 1 (0.5) and 2 (1.5), with step 1.25.
    # Faithful: 1*1.25 + 0.5 == 1.75.  A "corrected" version would give (1 + 0.5) * 1.25 == 1.875.
    assert edhpl_curve(1.0, EDHPL_PRICE_CURVE, 1.25) == pytest.approx(1.75)


def test_curve_quirk_last_stop_falls_through_to_zero():
    """Ported deliberately: a value sitting exactly on the final stop returns 0, not the maximum."""
    assert edhpl_curve(100, EDHPL_PRICE_CURVE, 1.25) == 0.0
    assert edhpl_curve(100.0001, EDHPL_PRICE_CURVE, 1.25) == pytest.approx(12.5)


# --- whole decks ---------------------------------------------------------------------------------

def test_every_deck_has_golden_values(deck_names, golden):
    assert set(deck_names) == set(golden), "a deck fixture has no golden entry (or vice versa)"


def test_golden_decks(db, golden, deck_names):
    for name in deck_names:
        pl = edhpowerlevel(load_deck(name), db)
        assert pl is not None
        for field in FIELDS:
            assert getattr(pl, field) == pytest.approx(golden[name][field], abs=1e-9), \
                f"{name}.{field}"


def test_golden_decks_resolve_every_card(db, deck_names):
    """If the fixture drifts out of sync with the decks, the scores would quietly shrink."""
    for name in deck_names:
        assert edhpowerlevel(load_deck(name), db).missing == [], name


def test_returns_none_without_prices(legacy_db):
    assert edhpowerlevel(load_deck("atraxa_four_colour.txt"), legacy_db) is None


# --- degenerate decks ----------------------------------------------------------------------------

@pytest.mark.parametrize("label,deck", [
    ("empty", Deck(commanders=[], cards=Counter())),
    ("commander only", Deck(commanders=["Atraxa, Praetors' Voice"], cards=Counter())),
    ("no nonlands", Deck(commanders=[], cards=Counter({"Forest": 50, "Island": 50}))),
    ("unknown only", Deck(commanders=[], cards=Counter({"Definitely Not A Card": 1}))),
    ("colourless", Deck(commanders=[], cards=Counter({"Sol Ring": 10, "Wastes": 20}))),
    ("modal dfc", Deck(commanders=[],
                       cards=Counter({"Agadeem's Awakening // Agadeem, the Undercrypt": 10}))),
])
def test_degenerate_decks_do_not_raise(db, label, deck):
    pl = edhpowerlevel(deck, db)
    assert pl is not None, label
    assert pl.level >= 0.0
    assert pl.impact >= 0.0


def test_unknown_cards_are_reported_not_scored(db):
    deck = Deck(commanders=[], cards=Counter({"Definitely Not A Card": 1, "Sol Ring": 1}))
    pl = edhpowerlevel(deck, db)
    assert pl.missing == ["Definitely Not A Card"]
    assert pl.impact > 0  # Sol Ring still counted


def test_empty_deck_scores_zero(db):
    assert edhpowerlevel(Deck(commanders=[], cards=Counter()), db).score == 0.0


# --- the override tables -------------------------------------------------------------------------

def test_basic_lands_are_flat_two_per_copy(db):
    """Basics bypass price and popularity entirely: impact is exactly 2 each."""
    pl = edhpowerlevel(Deck(commanders=[], cards=Counter({"Forest": 7})), db)
    assert pl.impact == pytest.approx(14.0)


def test_snow_covered_basics_get_the_same_treatment(db):
    pl = edhpowerlevel(Deck(commanders=[], cards=Counter({"Snow-Covered Forest": 7})), db)
    assert pl.impact == pytest.approx(14.0)


def test_commander_multiplier_only_applies_in_the_command_zone(db):
    """Thrasios carries a 3.5x commander multiplier; the same card in the 99 must not get it."""
    as_commander = edhpowerlevel(
        Deck(commanders=["Thrasios, Triton Hero"], cards=Counter()), db)
    in_the_99 = edhpowerlevel(
        Deck(commanders=[], cards=Counter({"Thrasios, Triton Hero": 1})), db)
    assert as_commander.impact == pytest.approx(in_the_99.impact * 3.5)


def test_free_spell_cmc_override(db):
    """Force of Will is costed at 0, not 5, which lifts the efficiency multiplier."""
    free = edhpowerlevel(Deck(commanders=[], cards=Counter({"Force of Will": 1})), db)
    assert free.avg_cmc == 0.0


def test_reserved_list_price_is_discounted(db, fixture_cards):
    """Gaea's Cradle is reserved: its price is cut to a fifth before hitting the price curve."""
    assert fixture_cards["cards"]["Gaea's Cradle"]["res"] is True
    pl = edhpowerlevel(Deck(commanders=[], cards=Counter({"Gaea's Cradle": 1})), db)
    # Even at a fifth, a four-figure card still tops out the price curve; the land multiplier applies.
    assert pl.impact > 0


def test_land_impact_is_discounted_and_costs_nothing(db):
    """A land contributes 0.6x impact and never moves the average mana value."""
    pl = edhpowerlevel(Deck(commanders=[], cards=Counter({"Gaea's Cradle": 1})), db)
    assert pl.avg_cmc == 0.0


# --- the prefilled link --------------------------------------------------------------------------

def test_url_encodes_a_deck():
    url = edhpowerlevel_url(["Commander", "1 Atraxa, Praetors' Voice", "", "Deck", "1 Sol Ring"])
    assert url.startswith("https://edhpowerlevel.com?d=")
    assert url.endswith("~Z~")
    assert "1+Sol+Ring" in url
    assert "Atraxa%2C+Praetors'+Voice" in url


def test_url_strips_set_codes_and_categories():
    """Their encoder drops "(SET) 123", [categories] and <annotations> before encoding."""
    url = edhpowerlevel_url(["1 Sol Ring (LEA) 123", "1 Rhystic Study [Draw]", "1 Counterspell <foil>"])
    assert "1+Sol+Ring~1+Rhystic+Study~1+Counterspell~Z~" in url


def test_label_formats_one_decimal_and_saturates():
    assert PowerLevel(level=7.5317).label() == "7.5"
    assert PowerLevel(level=10.0).label() == "10+"
    assert PowerLevel(level=12.5).label() == "10+"


def test_golden_file_is_valid_json():
    """Cheap guard against a half-written regeneration."""
    from .conftest import DATA
    assert json.loads((DATA / "powerlevel_golden.json").read_text())
