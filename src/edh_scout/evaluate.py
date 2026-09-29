"""Bracket triage: what is the *lowest* Commander bracket a deck can honestly claim?

The floor comes from hard signals only -- Game Changers, bans, mass land denial, extra turns and
known combos. It cannot see speed, synergy or pilot intent, so it is a floor, never a verdict.
"""
from __future__ import annotations

import hashlib
import json
import urllib.error
from dataclasses import dataclass, field

from .cards import CardDB
from .config import SPELLBOOK_API, TAG_BRACKET, TAG_NAME, log
from .deck import Deck
from .fetch import Http, OfflineMiss
from .powerlevel import PowerLevel, edhpowerlevel


@dataclass
class Result:
    floor: int = 1
    parts: dict = field(default_factory=dict)
    game_changers: list = field(default_factory=list)
    banned: list = field(default_factory=list)
    mld: list = field(default_factory=list)
    extra_turns: list = field(default_factory=list)
    combos: list = field(default_factory=list)       # (tag, [cards], [produces])
    stats: dict = field(default_factory=dict)
    unknown: list = field(default_factory=list)
    engine: str = "offline"
    power: "PowerLevel | None" = None

    def reasons(self) -> list[str]:
        out = []
        if self.banned:
            out.append("BANNED: " + ", ".join(self.banned))
        if self.game_changers:
            out.append(f"{len(self.game_changers)} GC: " + ", ".join(self.game_changers))
        if self.mld:
            out.append(("MLD? " if self.engine == "offline" else "MLD: ") + ", ".join(self.mld))
        if self.extra_turns:
            out.append("extra turns: " + ", ".join(self.extra_turns))
        for tag, cards, prod in self.combos[:3]:
            out.append(f"combo [{TAG_NAME.get(tag, tag)}]: " + " + ".join(cards)
                       + (f" -> {prod[0]}" if prod else ""))
        if len(self.combos) > 3:
            out.append(f"... +{len(self.combos) - 3} more combos")
        return out

    def hint(self, target: int) -> str:
        if self.floor <= target or not self.game_changers:
            return ""
        others = max((v for k, v in self.parts.items() if k != "gc"), default=1)
        if others > target or target < 2:
            return ""
        keep = 3 if target >= 3 else 0
        return f"cut {len(self.game_changers) - keep} Game Changer(s) to fit B{target}"


def deck_stats(deck: Deck, canon: dict, db: CardDB) -> dict:
    lands = nonland = tutors = ramp = 0
    mv_sum = 0.0
    for n, q in deck.cards.items():
        c = db.cards.get(canon.get(n) or "")
        if not c:
            continue
        if c["land"]:
            lands += q
            continue
        nonland += q
        mv_sum += c["mv"] * q
        tutors += q * c["tut"]
        ramp += q * c["ramp"]
    return {"cards": sum(deck.cards.values()) + len(deck.commanders), "lands": lands,
            "avg_mv": round(mv_sum / nonland, 2) if nonland else 0.0, "tutors": tutors, "ramp": ramp}


def apply_spellbook(r: Result, deck: Deck, http: Http) -> None:
    body = {"commanders": [{"card": c, "quantity": 1} for c in deck.commanders],
            "main": [{"card": n, "quantity": q} for n, q in sorted(deck.cards.items())]}
    raw = json.dumps(body, sort_keys=True).encode()
    res = http.json(f"{SPELLBOOK_API}/estimate-bracket", ttl=7 * 86400, data=raw,
                    key="spellbook:" + hashlib.sha256(raw).hexdigest(),
                    headers={"Content-Type": "application/json"})
    cards = res.get("cards", [])

    def pick(flag):
        return sorted(c["card"]["name"] for c in cards if c.get(flag))

    r.game_changers, r.banned = pick("gameChanger"), pick("banned")
    r.mld, r.extra_turns = pick("massLandDenial"), pick("extraTurn")
    r.combos = sorted(
        ((v["combo"].get("bracketTag", "R"),
          [u["card"]["name"] for u in v["combo"].get("uses", [])],
          [p["feature"]["name"] for p in v["combo"].get("produces", [])][:3])
         for v in res.get("combos", [])),
        key=lambda c: (-TAG_BRACKET.get(c[0], 4), len(c[1])))
    r.floor = TAG_BRACKET.get(res.get("bracketTag"), 4)
    r.engine = "spellbook"


def evaluate(deck: Deck, db: CardDB, http: Http, use_spellbook: bool = False,
             power_level: bool = True) -> Result:
    canon = {n: db.resolve(n) for n in [*deck.commanders, *deck.cards]}
    names = {c for c in canon.values() if c}
    info = db.cards.get
    r = Result()
    r.unknown = [] if db.partial else sorted(n for n, c in canon.items() if c is None)
    r.game_changers = sorted(c for c in names if info(c, {}).get("gc"))
    r.banned = sorted(c for c in names if info(c, {}).get("ban"))
    r.mld = sorted(c for c in names if info(c, {}).get("mld"))
    r.extra_turns = sorted(c for c in names if info(c, {}).get("xt"))
    r.combos = [(tag, cards, prod) for _, tag, cards, prod in db.find_combos(names)]
    if not db.partial:
        r.stats = deck_stats(deck, canon, db)
        if power_level:
            r.power = edhpowerlevel(deck, db)
    if use_spellbook:
        try:
            apply_spellbook(r, deck, http)
        except (urllib.error.URLError, OfflineMiss, ValueError) as e:
            log(f"  spellbook unavailable ({e}); using offline estimate")
    r.parts = {
        "banned": 99 if r.banned else 1,
        "gc": 4 if len(r.game_changers) > 3 else 3 if r.game_changers else 1,
        "mld": 4 if r.mld else 1,
        "xturn": 4 if len(r.extra_turns) >= 2 else 2 if r.extra_turns else 1,
        "combo": max((TAG_BRACKET.get(t, 4) for t, *_ in r.combos), default=1),
    }
    if r.engine == "offline":
        r.floor = max(r.parts.values())
    return r
