"""Decklist parsing and the slug/bracket helpers."""
from __future__ import annotations

import argparse

import pytest

from edh_scout.deck import (
    Deck,
    bracket_spec,
    commander_slug,
    fmt_brackets,
    parse_decklist,
    slugify,
)


# --- parse_decklist ------------------------------------------------------------------------------

def test_sections_assign_commanders():
    deck = parse_decklist("Commander\n1 Atraxa, Praetors' Voice\n\nDeck\n1 Sol Ring\n")
    assert deck.commanders == ["Atraxa, Praetors' Voice"]
    assert deck.cards == {"Sol Ring": 1}


def test_cmdr_annotation_marks_a_commander():
    deck = parse_decklist("1 Krenko, Mob Boss *CMDR*\n1 Sol Ring\n")
    assert deck.commanders == ["Krenko, Mob Boss"]
    assert "Krenko, Mob Boss" not in deck.cards


def test_archidekt_commander_category():
    deck = parse_decklist("1 Krenko, Mob Boss [Commander{top}]\n1 Sol Ring [Ramp]\n")
    assert deck.commanders == ["Krenko, Mob Boss"]
    assert deck.cards == {"Sol Ring": 1}


def test_set_codes_and_collector_numbers_are_stripped():
    deck = parse_decklist("Deck\n1 Sol Ring (LEA) 123\n1 Counterspell (7ED) 67★\n")
    assert deck.cards == {"Sol Ring": 1, "Counterspell": 1}


def test_categories_and_foil_markers_are_stripped():
    deck = parse_decklist("Deck\n1 Sol Ring [Ramp] *F*\n")
    assert deck.cards == {"Sol Ring": 1}


def test_quantities_including_x_notation():
    deck = parse_decklist("Deck\n4 Forest\n2x Island\nSwamp\n")
    assert deck.cards == {"Forest": 4, "Island": 2, "Swamp": 1}


def test_duplicate_lines_accumulate():
    deck = parse_decklist("Deck\n1 Forest\n3 Forest\n")
    assert deck.cards == {"Forest": 4}


def test_sideboard_and_maybeboard_are_dropped():
    deck = parse_decklist("Deck\n1 Sol Ring\n\nSideboard\n1 Ponder\n\nMaybeboard\n1 Brainstorm\n")
    assert deck.cards == {"Sol Ring": 1}


def test_comments_and_blank_lines_are_ignored():
    deck = parse_decklist("// a comment\n# another\n\nDeck\n1 Sol Ring\n")
    assert deck.cards == {"Sol Ring": 1}


def test_section_headers_tolerate_a_trailing_colon():
    deck = parse_decklist("Commander:\n1 Krenko, Mob Boss\nDeck:\n1 Sol Ring\n")
    assert deck.commanders == ["Krenko, Mob Boss"]
    assert deck.cards == {"Sol Ring": 1}


def test_commanders_never_appear_in_the_99():
    deck = parse_decklist("Commander\n1 Krenko, Mob Boss\n\nDeck\n1 Krenko, Mob Boss\n1 Sol Ring\n")
    assert deck.commanders == ["Krenko, Mob Boss"]
    assert deck.cards == {"Sol Ring": 1}


def test_explicit_commanders_argument_wins():
    deck = parse_decklist("Deck\n1 Sol Ring\n", commanders=["Krenko, Mob Boss"])
    assert deck.commanders == ["Krenko, Mob Boss"]


def test_partners_keep_their_order():
    deck = parse_decklist("Commander\n1 Thrasios, Triton Hero\n1 Tymna the Weaver\n\nDeck\n1 Sol Ring\n")
    assert deck.commanders == ["Thrasios, Triton Hero", "Tymna the Weaver"]


def test_empty_input():
    deck = parse_decklist("")
    assert deck.commanders == [] and deck.cards == {}


# --- Deck.lines round trip -----------------------------------------------------------------------

def test_lines_round_trips_through_the_parser():
    original = parse_decklist(
        "Commander\n1 Atraxa, Praetors' Voice\n\nDeck\n2 Forest\n1 Sol Ring\n")
    reparsed = parse_decklist("\n".join(original.lines()))
    assert reparsed.commanders == original.commanders
    assert reparsed.cards == original.cards


def test_lines_sorts_the_99():
    deck = Deck(commanders=["Krenko, Mob Boss"], cards={"Sol Ring": 1, "Forest": 2})
    assert deck.lines() == ["Commander", "1 Krenko, Mob Boss", "", "Deck", "2 Forest", "1 Sol Ring"]


# --- slugs ---------------------------------------------------------------------------------------

@pytest.mark.parametrize("name,expected", [
    ("Atraxa, Praetors' Voice", "atraxa-praetors-voice"),
    ("Krenko, Mob Boss", "krenko-mob-boss"),
    ("Jodah, the Unifier", "jodah-the-unifier"),
    ("Tergrid, God of Fright // Tergrid's Lantern", "tergrid-god-of-fright"),
    ("Juzám Djinn", "juzam-djinn"),
    ("Rick, Steadfast Leader", "rick-steadfast-leader"),
])
def test_slugify(name, expected):
    assert slugify(name) == expected


def test_commander_slug_sorts_partners_alphabetically():
    pair = ["Tymna the Weaver", "Kraum, Ludevic's Opus"]
    assert commander_slug(pair) == commander_slug(list(reversed(pair)))
    assert commander_slug(pair) == "kraum-ludevics-opus-tymna-the-weaver"


# --- bracket selectors ---------------------------------------------------------------------------

@pytest.mark.parametrize("spec,expected", [
    ("3", {3}),
    ("1-3", {1, 2, 3}),
    ("2,4", {2, 4}),
    ("3-1", {1, 2, 3}),          # reversed range still works
    (" 2 , 4 ", {2, 4}),         # whitespace tolerated
    ("1-5", {1, 2, 3, 4, 5}),
])
def test_bracket_spec(spec, expected):
    assert bracket_spec(spec) == expected


@pytest.mark.parametrize("spec", ["0", "6", "1-9", "x", "", "3-"])
def test_bracket_spec_rejects_nonsense(spec):
    with pytest.raises(argparse.ArgumentTypeError):
        bracket_spec(spec)


def test_fmt_brackets():
    assert fmt_brackets({3, 1, 2}) == "B1,2,3"
