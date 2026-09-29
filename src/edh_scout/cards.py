"""The local card and combo databases built by `sync`, plus the offline text heuristics."""
from __future__ import annotations

import re
import time
import urllib.parse
from collections import defaultdict
from pathlib import Path

from .config import CARDS_SCHEMA, DB_CARDS, DB_COMBOS, SCRYFALL, TAG_BRACKET, log
from .fetch import Http
from .storage import iter_bulk_cards, iter_json_array, load_gz, open_text

# --- offline heuristics (only used when Spellbook's curated flags aren't available) ---------------
EXTRA_TURN_RE = re.compile(r"\btakes? an? extra turns?\b|\bextra turn after this one\b", re.I)
MLD_RE = re.compile(
    r"\b(?:destroy|exile) all (?:[\w,]+ )*?lands\b"
    r"|\b(?:each player|each opponent|all players) (?:sacrifices?|returns?) (?:all|x|\w+) lands\b"
    r"|\breturn all lands\b"
    r"|\blands don't untap\b|\blands do not untap\b"
    r"|\bplayers can't untap more than\b"
    r"|\bnonbasic lands are (?:mountains|islands|swamps|plains|forests)\b", re.I)
TUTOR_RE = re.compile(r"search your library for [^.]*?\bcards?\b", re.I)
LANDWORD_RE = re.compile(r"\b(?:lands?|plains|islands?|swamps?|mountains?|forests?)\b", re.I)
LAND_RAMP_RE = re.compile(
    r"search your library for [^.]*?\b(?:lands?|plains|islands?|swamps?|mountains?|forests?)\b"
    r"[^.]*?onto the battlefield", re.I)
SKIP_LAYOUTS = {"token", "double_faced_token", "emblem", "art_series", "planar", "scheme", "vanguard"}


def build_cards_db(bulk_path: Path) -> dict:
    cards, aliases = {}, {}
    for c in iter_bulk_cards(bulk_path):
        if c.get("layout") in SKIP_LAYOUTS or c.get("set_type") == "memorabilia":
            continue
        name = c["name"]
        legal = (c.get("legalities") or {}).get("commander", "not_legal")
        if name in cards and legal == "not_legal":
            continue
        faces = c.get("card_faces") or []
        text = c.get("oracle_text") or "\n".join(f.get("oracle_text", "") for f in faces)
        type_line = c.get("type_line") or " // ".join(f.get("type_line", "") for f in faces)
        land = "Land" in type_line.split(" // ")[0]
        mv = float(c.get("cmc") or 0)
        prices = c.get("prices") or {}
        usd = prices.get("usd") or prices.get("usd_foil") or prices.get("usd_etched")
        # colors is absent (not empty) on most double-faced cards -- keep that distinction, the
        # EDHPowerLevel maths treats "no colors key" and "colourless" differently.
        colors = c.get("colors")
        if colors is None and faces:
            colors = faces[0].get("colors")
        cards[name] = {
            "gc": bool(c.get("game_changer")),
            "ban": legal == "banned",
            "mld": bool(MLD_RE.search(text)),
            "xt": bool(EXTRA_TURN_RE.search(text)),
            "tut": any(not LANDWORD_RE.search(m.group(0)) for m in TUTOR_RE.finditer(text)),
            "ramp": not land and mv <= 2 and bool(c.get("produced_mana") or LAND_RAMP_RE.search(text)),
            "mv": mv,
            "land": land,
            "usd": float(usd) if usd else 0.0,
            "rank": c.get("edhrec_rank"),
            "res": bool(c.get("reserved")),
            "col": colors,
            "dfc": c.get("layout") == "modal_dfc",
        }
        aliases[name.lower()] = name
        for f in faces:
            aliases.setdefault(f.get("name", "").lower(), name)
    return {"built": time.time(), "schema": CARDS_SCHEMA, "cards": cards, "aliases": aliases}


def build_combo_db(dump_path: Path, resolve=lambda n: None) -> dict:
    combos, templated = [], 0
    for v in iter_json_array(open_text(dump_path), key="variants"):
        if v.get("status") not in ("OK", "E"):
            continue
        if v.get("requires"):  # needs "any sac outlet"-style templates; only the online API resolves those
            templated += 1
            continue
        names = sorted({resolve(u["card"]["name"]) or u["card"]["name"] for u in v.get("uses", [])})
        produces = [p["feature"]["name"] for p in v.get("produces", [])][:3]
        tag = v.get("bracketTag") or v.get("bracket_tag") or "R"
        combos.append([v.get("id"), tag, names, produces])
    return {"built": time.time(), "combos": combos, "templated_skipped": templated}


class CardDB:
    def __init__(self, cards: dict, aliases: dict, combos: list, partial: bool = False,
                 schema: int = 0):
        self.cards, self.aliases, self.combos, self.partial = cards, aliases, combos, partial
        self.schema = schema
        self.has_prices = schema >= 2      # price/rank/colour fields, i.e. power levels are scorable
        self._idx = defaultdict(list)
        for i, (_, _, names, _) in enumerate(combos):
            for n in names:
                self._idx[n].append(i)

    @classmethod
    def load(cls, http: Http) -> "CardDB":
        partial, schema = False, 0
        if DB_CARDS.exists():
            d = load_gz(DB_CARDS)
            age = (time.time() - d.get("built", 0)) / 86400
            if age > 30:
                log(f"note: card DB is {age:.0f} days old -- `sync` to pick up Game Changer changes")
            cards, aliases, schema = d["cards"], d["aliases"], d.get("schema", 1)
            if schema < CARDS_SCHEMA:
                log("note: card DB predates power-level scoring -- `sync` to enable it")
        else:
            log("no local card DB (run `sync`); using Scryfall is:gamechanger only -- stats disabled")
            partial, cards, aliases = True, {}, {}
            for n in gamechangers_from_scryfall(http):
                cards[n] = {"gc": True}
                aliases[n.lower()] = n
                for face in n.split(" // "):
                    aliases.setdefault(face.lower(), n)
        combos = []
        if DB_COMBOS.exists():
            combos = load_gz(DB_COMBOS)["combos"]
        else:
            log("no local combo DB (run `sync`); combos only via --spellbook")
        return cls(cards, aliases, combos, partial, schema)

    def resolve(self, name: str) -> str | None:
        key = name.strip().lower()
        return self.aliases.get(key) or self.aliases.get(key.split(" // ")[0])

    def find_combos(self, names: set[str]) -> list:
        cand = set()
        for n in names:
            cand.update(self._idx.get(n, ()))
        hits = [self.combos[i] for i in cand if all(x in names for x in self.combos[i][2])]
        hits.sort(key=lambda c: (-TAG_BRACKET.get(c[1], 4), len(c[2])))
        return hits


def gamechangers_from_scryfall(http: Http) -> list[str]:
    url = f"{SCRYFALL}/cards/search?" + urllib.parse.urlencode({"q": "is:gamechanger", "unique": "cards"})
    names = []
    while url:
        page = http.json(url, ttl=7 * 86400)
        names += [c["name"] for c in page.get("data", [])]
        url = page.get("next_page") if page.get("has_more") else None
    return names
