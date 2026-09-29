"""EDHREC payload parsing. Their JSON is undocumented and has already moved once, so every shape
the code claims to handle gets a test."""
from __future__ import annotations

import pytest

from edh_scout.edhrec import _entry, edhrec_cards, edhrec_commanders


# --- individual entries --------------------------------------------------------------------------

@pytest.mark.parametrize("raw,expected", [
    (["Sol Ring", 1], ("Sol Ring", 1)),
    (["Forest", 7], ("Forest", 7)),
    (["Sol Ring"], ("Sol Ring", 1)),                              # quantity omitted
    (("Sol Ring", 2), ("Sol Ring", 2)),                           # tuple
    ("Sol Ring", ("Sol Ring", 1)),                                # bare name
    ({"name": "Sol Ring", "quantity": 3}, ("Sol Ring", 3)),
    ({"card": "Sol Ring", "q": 2}, ("Sol Ring", 2)),
    ({"name": "Sol Ring"}, ("Sol Ring", 1)),
])
def test_entry_shapes(raw, expected):
    assert _entry(raw) == expected


def test_entry_tolerates_empty_names():
    assert _entry(None) == ("", 1)


# --- the 99 --------------------------------------------------------------------------------------

def test_cards_grouped_by_type():
    data = {"deck": {"cards": {
        "Creature": [["Birds of Paradise", 1], ["Llanowar Elves", 1]],
        "Land": [["Forest", 6]],
    }}}
    assert edhrec_cards(data) == {"Birds of Paradise": 1, "Llanowar Elves": 1, "Forest": 6}


def test_cards_as_a_flat_list():
    data = {"deck": {"cards": [["Sol Ring", 1], ["Forest", 4]]}}
    assert edhrec_cards(data) == {"Sol Ring": 1, "Forest": 4}


def test_cards_legacy_top_level_list():
    assert edhrec_cards({"cards": ["Sol Ring", "Forest"]}) == {"Sol Ring": 1, "Forest": 1}


def test_cards_legacy_deck_is_the_list():
    assert edhrec_cards({"deck": ["Sol Ring", "Forest"]}) == {"Sol Ring": 1, "Forest": 1}


def test_cards_empty_payload():
    assert edhrec_cards({}) == {}


def test_cards_tolerates_empty_groups():
    data = {"deck": {"cards": {"Creature": [], "Land": None, "Artifact": [["Sol Ring", 1]]}}}
    assert edhrec_cards(data) == {"Sol Ring": 1}


def test_duplicate_entries_accumulate():
    data = {"deck": {"cards": {"A": [["Forest", 2]], "B": [["Forest", 3]]}}}
    assert edhrec_cards(data) == {"Forest": 5}


# --- commanders ----------------------------------------------------------------------------------

def test_commanders_top_level():
    assert edhrec_commanders({"commanders": ["Atraxa, Praetors' Voice"]}) == \
        ["Atraxa, Praetors' Voice"]


def test_commanders_from_commander_v2():
    data = {"deck": {"commander_v2": [["Thrasios, Triton Hero", 1], ["Tymna the Weaver", 1]]}}
    assert edhrec_commanders(data) == ["Thrasios, Triton Hero", "Tymna the Weaver"]


def test_commanders_from_legacy_commander_key():
    assert edhrec_commanders({"deck": {"commander": ["Krenko, Mob Boss"]}}) == ["Krenko, Mob Boss"]


def test_commanders_prefers_the_top_level_list():
    data = {"commanders": ["Top Level"], "deck": {"commander_v2": [["Nested", 1]]}}
    assert edhrec_commanders(data) == ["Top Level"]


def test_commanders_absent():
    assert edhrec_commanders({"deck": {}}) == []


def test_commanders_drops_empty_entries():
    assert edhrec_commanders({"commanders": ["Real", None, ""]}) == ["Real"]
