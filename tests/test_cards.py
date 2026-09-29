"""Building the card DB from Scryfall records, and looking things up in it."""
from __future__ import annotations

import gzip
import json

import pytest

from edh_scout.cards import CardDB, build_cards_db, build_combo_db
from edh_scout.storage import save_gz


def scryfall(**overrides) -> dict:
    """A minimal Scryfall oracle-card record; override just the field under test."""
    base = {
        "name": "Test Card",
        "layout": "normal",
        "cmc": 2.0,
        "type_line": "Creature — Human Wizard",
        "oracle_text": "",
        "legalities": {"commander": "legal"},
        "prices": {"usd": "1.50"},
        "edhrec_rank": 500,
        "colors": ["U"],
        "reserved": False,
    }
    base.update(overrides)
    return base


def build(tmp_path, *records) -> dict:
    path = tmp_path / "bulk.jsonl"
    path.write_text("\n".join(json.dumps(r) for r in records), encoding="utf-8")
    return build_cards_db(path)


# --- field extraction ----------------------------------------------------------------------------

def test_basic_fields(tmp_path):
    db = build(tmp_path, scryfall())
    card = db["cards"]["Test Card"]
    assert card["mv"] == 2.0
    assert card["usd"] == 1.5
    assert card["rank"] == 500
    assert card["col"] == ["U"]
    assert card["land"] is False
    assert card["dfc"] is False
    assert card["res"] is False


def test_schema_is_stamped(tmp_path):
    from edh_scout.config import CARDS_SCHEMA
    assert build(tmp_path, scryfall())["schema"] == CARDS_SCHEMA


def test_price_falls_back_through_foil_then_etched(tmp_path):
    db = build(tmp_path,
               scryfall(name="Plain", prices={"usd": "2.00", "usd_foil": "9"}),
               scryfall(name="FoilOnly", prices={"usd": None, "usd_foil": "9.00"}),
               scryfall(name="EtchedOnly", prices={"usd": None, "usd_foil": None, "usd_etched": "4"}),
               scryfall(name="Priceless", prices={}))
    assert db["cards"]["Plain"]["usd"] == 2.0
    assert db["cards"]["FoilOnly"]["usd"] == 9.0
    assert db["cards"]["EtchedOnly"]["usd"] == 4.0
    assert db["cards"]["Priceless"]["usd"] == 0.0


def test_missing_rank_is_kept_as_none(tmp_path):
    db = build(tmp_path, scryfall(edhrec_rank=None))
    assert db["cards"]["Test Card"]["rank"] is None


def test_lands_are_detected_from_the_front_face(tmp_path):
    db = build(tmp_path,
               scryfall(name="Plainsy", type_line="Land"),
               scryfall(name="Creatureland", type_line="Land Creature — Elemental"),
               scryfall(name="Backside", type_line="Creature — Bear // Land"))
    assert db["cards"]["Plainsy"]["land"] is True
    assert db["cards"]["Creatureland"]["land"] is True
    assert db["cards"]["Backside"]["land"] is False


def test_modal_dfc_is_flagged(tmp_path):
    db = build(tmp_path, scryfall(name="Modal", layout="modal_dfc",
                                  type_line="Sorcery // Land", colors=["B"]))
    assert db["cards"]["Modal"]["dfc"] is True


def test_absent_colors_falls_back_to_the_front_face(tmp_path):
    """Double-faced cards omit top-level `colors`; the front face carries them."""
    record = scryfall(name="Transformer", layout="transform", colors=None,
                      card_faces=[{"name": "Front", "colors": ["R"], "oracle_text": "",
                                   "type_line": "Creature — Werewolf"},
                                  {"name": "Back", "colors": ["R"], "oracle_text": "",
                                   "type_line": "Creature — Werewolf"}])
    db = build(tmp_path, record)
    assert db["cards"]["Transformer"]["col"] == ["R"]


def test_colourless_keeps_an_empty_list_not_none(tmp_path):
    """The power-level maths treats "no colors key" and "colourless" differently -- keep them apart."""
    db = build(tmp_path, scryfall(name="Artifact", colors=[]))
    assert db["cards"]["Artifact"]["col"] == []


def test_reserved_list_flag(tmp_path):
    db = build(tmp_path, scryfall(name="Old Thing", reserved=True))
    assert db["cards"]["Old Thing"]["res"] is True


def test_banned_and_game_changer_flags(tmp_path):
    db = build(tmp_path,
               scryfall(name="Banned Thing", legalities={"commander": "banned"}),
               scryfall(name="Changer", game_changer=True))
    assert db["cards"]["Banned Thing"]["ban"] is True
    assert db["cards"]["Changer"]["gc"] is True


def test_skipped_layouts_and_memorabilia_are_excluded(tmp_path):
    db = build(tmp_path,
               scryfall(name="A Token", layout="token"),
               scryfall(name="An Emblem", layout="emblem"),
               scryfall(name="Gold Border", set_type="memorabilia"),
               scryfall(name="Real Card"))
    assert set(db["cards"]) == {"Real Card"}


def test_faces_become_aliases(tmp_path):
    record = scryfall(name="Front // Back", layout="transform", colors=["G"],
                      card_faces=[{"name": "Front", "colors": ["G"], "oracle_text": "",
                                   "type_line": "Creature"},
                                  {"name": "Back", "colors": ["G"], "oracle_text": "",
                                   "type_line": "Creature"}])
    db = build(tmp_path, record)
    assert db["aliases"]["front"] == "Front // Back"
    assert db["aliases"]["back"] == "Front // Back"


# --- text heuristics -----------------------------------------------------------------------------

def test_extra_turn_and_mld_heuristics(tmp_path):
    db = build(tmp_path,
               scryfall(name="Time Walk", oracle_text="Target player takes an extra turn after this one."),
               scryfall(name="Armageddon-like", oracle_text="Destroy all lands."),
               scryfall(name="Harmless", oracle_text="Draw a card."))
    assert db["cards"]["Time Walk"]["xt"] is True
    assert db["cards"]["Armageddon-like"]["mld"] is True
    assert db["cards"]["Harmless"]["xt"] is False
    assert db["cards"]["Harmless"]["mld"] is False


def test_tutor_heuristic_ignores_land_searches(tmp_path):
    db = build(tmp_path,
               scryfall(name="Demonic", oracle_text="Search your library for a card, then shuffle."),
               scryfall(name="Rampant", oracle_text="Search your library for a basic land card."))
    assert db["cards"]["Demonic"]["tut"] is True
    assert db["cards"]["Rampant"]["tut"] is False


def test_ramp_heuristic_needs_a_cheap_nonland(tmp_path):
    db = build(tmp_path,
               scryfall(name="Signet", cmc=2.0, type_line="Artifact", produced_mana=["W", "U"]),
               scryfall(name="Expensive Rock", cmc=5.0, type_line="Artifact", produced_mana=["W"]),
               scryfall(name="A Land", type_line="Land", cmc=0.0, produced_mana=["G"]))
    assert db["cards"]["Signet"]["ramp"] is True
    assert db["cards"]["Expensive Rock"]["ramp"] is False
    assert db["cards"]["A Land"]["ramp"] is False


# --- CardDB lookups ------------------------------------------------------------------------------

def test_resolve_is_case_insensitive_and_handles_faces(db):
    assert db.resolve("sol ring") == "Sol Ring"
    assert db.resolve("  SOL RING  ") == "Sol Ring"
    assert db.resolve("Definitely Not A Card") is None


def test_has_prices_tracks_the_schema(fixture_cards):
    cards, aliases = fixture_cards["cards"], fixture_cards["aliases"]
    assert CardDB(cards, aliases, [], schema=2).has_prices is True
    assert CardDB(cards, aliases, [], schema=1).has_prices is False
    assert CardDB(cards, aliases, [], schema=0).has_prices is False


def test_find_combos_requires_every_piece_present():
    combos = [
        ["c1", "R", ["Alpha", "Beta"], ["Infinite mana"]],
        ["c2", "S", ["Alpha", "Gamma"], ["Win the game"]],
    ]
    db = CardDB({}, {}, combos, schema=2)
    assert [c[0] for c in db.find_combos({"Alpha", "Beta"})] == ["c1"]
    assert db.find_combos({"Alpha"}) == []
    assert len(db.find_combos({"Alpha", "Beta", "Gamma"})) == 2


def test_find_combos_sorts_most_severe_first():
    combos = [
        ["mild", "E", ["Alpha", "Beta"], []],       # Exhibition -> bracket 1
        ["harsh", "R", ["Alpha", "Beta"], []],      # Ruthless   -> bracket 4
    ]
    db = CardDB({}, {}, combos, schema=2)
    assert [c[0] for c in db.find_combos({"Alpha", "Beta"})] == ["harsh", "mild"]


# --- combo DB ------------------------------------------------------------------------------------

def variant(**overrides) -> dict:
    base = {"id": "v1", "status": "OK", "bracketTag": "S",
            "uses": [{"card": {"name": "Alpha"}}, {"card": {"name": "Beta"}}],
            "produces": [{"feature": {"name": "Infinite mana"}}]}
    base.update(overrides)
    return base


def write_variants(tmp_path, *variants):
    path = tmp_path / "variants.json.gz"
    with gzip.open(path, "wt", encoding="utf-8") as f:
        json.dump({"variants": list(variants)}, f)
    return path


def test_build_combo_db_keeps_usable_variants(tmp_path):
    out = build_combo_db(write_variants(tmp_path, variant()))
    assert out["combos"] == [["v1", "S", ["Alpha", "Beta"], ["Infinite mana"]]]


def test_build_combo_db_skips_templated_and_unusable(tmp_path):
    out = build_combo_db(write_variants(
        tmp_path,
        variant(id="ok"),
        variant(id="templated", requires=[{"template": {"name": "a sac outlet"}}]),
        variant(id="draft", status="D"),
    ))
    assert [c[0] for c in out["combos"]] == ["ok"]
    assert out["templated_skipped"] == 1


def test_build_combo_db_canonicalises_names(tmp_path):
    out = build_combo_db(write_variants(tmp_path, variant()),
                         resolve=lambda n: {"Alpha": "Alpha Prime"}.get(n))
    assert out["combos"][0][2] == ["Alpha Prime", "Beta"]


def test_combo_tag_defaults_to_ruthless(tmp_path):
    out = build_combo_db(write_variants(tmp_path, variant(bracketTag=None)))
    assert out["combos"][0][1] == "R"


# --- round trip through the on-disk format -------------------------------------------------------

def test_cards_db_survives_a_gzip_round_trip(tmp_path):
    built = build(tmp_path, scryfall())
    path = tmp_path / "cards.json.gz"
    save_gz(path, built)
    from edh_scout.storage import load_gz
    assert load_gz(path) == built


@pytest.mark.parametrize("schema,expected", [(1, False), (2, True)])
def test_power_levels_gated_on_schema(fixture_cards, schema, expected):
    db = CardDB(fixture_cards["cards"], fixture_cards["aliases"], [], schema=schema)
    assert db.has_prices is expected
