"""EDHPowerLevel's 1-10 power level, scored locally.

A port of the scoring EDHPowerLevel runs in the browser, so a deck gets a number without a round
trip. Their site has no API, but the maths is plain client-side JS over two per-card inputs --
market price and EDHREC rank -- both of which Scryfall's bulk data already gives us in `sync`.

Caveats worth knowing when the number disagrees with the site:
  * prices move, and we use the oracle-card printing's USD where the site uses its own snapshot;
  * edhrec_rank drifts between syncs;
  * they tune the constants below without notice -- re-check if results start looking off.

`tools/edhpl_reference.js` is a transcription of their scoring straight out of the shipped bundle;
`tests/test_powerlevel.py` pins this module against values derived from it.
"""
from __future__ import annotations

import re
import urllib.parse
from dataclasses import dataclass, field

from .cards import CardDB
from .deck import Deck

EDHPL_LAND = 0.6                 # lands contribute a fraction of their impact
EDHPL_RESERVED = 0.2             # reserved-list cards get their price discounted first
EDHPL_FAVOR_PRICE = 0.25         # price is weighted up, popularity down, by this much
EDHPL_CMC_FLOOR, EDHPL_CMC_CEILING = 1.75, 6.0
EDHPL_EFFICIENCY_LIMITS = (0.65, 1.1)
EDHPL_PRICE_CURVE = [0, 0.5, 1.5, 3.5, 6, 10, 15, 25, 40, 65, 100]
EDHPL_POP_CURVE = [0, 8500, 13600, 17100, 19800, 21900, 23700, 25300, 26200, 26700, 27000]
EDHPL_POWER_CURVE = [0, 250, 320, 350, 380, 420, 470, 560, 760, 890, 1000]

EDHPL_BASICS = {"Mountain", "Forest", "Island", "Swamp", "Plains", "Wastes"}
EDHPL_BASICS |= {f"Snow-Covered {b}" for b in EDHPL_BASICS}

# Price multipliers (applied before the price curve).
EDHPL_PRICE_MULT = {
    "Mystic Remora": 2,
    "Rest in Peace": 2.5,
    "The One Ring": 0.8,
    "Sylvan Library": 0.6,
    "Ragavan, Nimble Pilferer": 0.4,
    "Sol Ring": 8,
}

# CMC replacements (absolute) -- free spells and cost-reducers priced at what they really cost.
EDHPL_CMC = {
    "Fierce Guardianship": 0, "Deflecting Swat": 0, "Deadly Rollick": 0, "Flawless Maneuver": 0,
    "Obscuring Haze": 0, "Flare of Denial": 0, "Flare of Fortitude": 0, "Flare of Duplication": 0,
    "Flare of Malice": 0, "Flare of Cultivation": 0, "Endurance": 0, "Solitude": 0, "Grief": 0,
    "Subtlety": 0, "Fury": 0, "Force of Vigor": 0, "Force of Negation": 0, "Force of Despair": 0,
    "Force of Virtue": 0, "Force of Rage": 0, "Force of Will": 0, "Misdirection": 0, "Submerge": 0,
    "Snuff Out": 0, "Daze": 0, "Foil": 0, "Gush": 0, "Shriekmaw": 2, "Blasphemous Act": 3,
    "The Great Henge": 5, "Vandalblast": 3, "Cyclonic Rift": 5, "Everflowing Chalice": 2,
    "Dig Through Time": 4, "Temporal Trespass": 7, "Treasure Cruise": 3,
    "Emrakul, the Promised End": 7,
}

# Flat impact multipliers, and ones that only apply while the card is the commander.
EDHPL_IMPACT_MULT = {"Cataclysm": 1.7, "Jokulhaups": 1.7, "Boom // Bust": 1.7, "Armageddon": 1.4}
EDHPL_CMDR_MULT = {
    "Vivi Ornitier": 3, "Korvold, Fae-Cursed King": 3, "Chulane, Teller of Tales": 3,
    "Yuriko, the Tiger's Shadow": 3, "Shirei, Shizo's Caretaker": 2.5, "Orvar, the All-Form": 3,
    "Magda, Brazen Outlaw": 3.5, "Tergrid, God of Fright // Tergrid's Lantern": 2.5,
    "Winota, Joiner of Forces": 3, "Sisay, Weatherlight Captain": 4, "Urza, Lord High Artificer": 3,
    "Kinnan, Bonder Prodigy": 4, "Yedora, Grave Gardener": 2, "Kraum, Ludevic's Opus": 2,
    "Thrasios, Triton Hero": 3.5, "Tymna the Weaver": 2, "Vial Smasher the Fierce": 2,
}


def edhpl_curve(value: float, curve: list, step: float = 1.0) -> float:
    """Map `value` onto a curve's index positions, piecewise-linearly.

    Ported as-is from EDHPowerLevel, quirks included: only the index is scaled by `step`, not the
    interpolated fraction, and a value sitting exactly on the last stop falls through to 0."""
    if value <= curve[0]:
        return 0.0
    if value > curve[-1]:
        return (len(curve) - 1) * step
    for i in range(len(curve) - 1):
        if curve[i] <= value < curve[i + 1]:
            return i * step + (value - curve[i]) / (curve[i + 1] - curve[i])
    return 0.0


@dataclass
class PowerLevel:
    level: float = 0.0          # the 1-10 number ("10+" once it maxes out)
    score: float = 0.0          # impact scaled by efficiency; what the power curve is read against
    impact: float = 0.0
    efficiency: float = 0.0     # interpolated between 0.65 and 1.1, uncapped past a very cheap curve
    avg_cmc: float = 0.0
    curve_cmc: int = 0          # cmc by which 65% of the deck's coloured impact has landed
    missing: list = field(default_factory=list)

    def label(self) -> str:
        return "10+" if self.level >= 10 else f"{self.level:.1f}"


def _bump(buckets: list, index: int, amount: float) -> None:
    while len(buckets) <= index:
        buckets.append(0.0)
    buckets[index] += amount


def edhpowerlevel(deck: Deck, db: CardDB) -> PowerLevel | None:
    """Score a deck the way edhpowerlevel.com does. None if the DB predates the fields it needs."""
    if not db.has_prices:
        return None
    entries = [(c, 1, True) for c in deck.commanders] + [(n, q, False) for n, q in deck.cards.items()]
    by_cmc: list = []                                   # every card, lands folded into index 0
    by_color = {k: [] for k in "RWGUBC"}                # coloured impact only, drives the 65% point
    total_impact = total_cmc = 0.0
    nonlands = 0
    pl = PowerLevel()
    for name, qty, is_commander in entries:
        canon = db.resolve(name)
        card = db.cards.get(canon or "")
        if not card:
            pl.missing.append(name)
            continue
        price = card["usd"] * EDHPL_PRICE_MULT.get(canon, 1)
        cmc = float(EDHPL_CMC.get(canon, card["mv"]))
        if card["res"]:
            price *= EDHPL_RESERVED
        # A missing rank means "nobody plays it", which is exactly what rank 27000 scores: nothing.
        rank = card["rank"] if card["rank"] is not None else EDHPL_POP_CURVE[-1]
        impact = qty * (edhpl_curve(price, EDHPL_PRICE_CURVE, 1 + EDHPL_FAVOR_PRICE)
                        + edhpl_curve(EDHPL_POP_CURVE[-1] - rank, EDHPL_POP_CURVE,
                                      1 - EDHPL_FAVOR_PRICE))
        impact *= EDHPL_IMPACT_MULT.get(canon, 1)
        if is_commander:
            impact *= EDHPL_CMDR_MULT.get(canon, 1)
        is_land = card["land"] or card["dfc"]
        if is_land:
            impact *= EDHPL_LAND
            cmc = 0.0
        if canon in EDHPL_BASICS:
            impact = 2.0 * qty
        if not card["dfc"]:
            total_cmc += cmc * qty
        total_impact += impact
        if is_land:
            _bump(by_cmc, 0, impact)
            continue
        nonlands += qty
        _bump(by_cmc, int(cmc), impact)
        colors = card["col"]
        if colors is None:          # no colors field at all -- counted in the total, not per-colour
            continue
        for key in colors or "C":
            _bump(by_color[key], int(cmc), impact / (len(colors) or 1))

    colored = [c + [0.0] * (len(by_cmc) - len(c)) for c in by_color.values()]
    colored_total = sum(map(sum, colored))
    running = 0.0
    for i in range(len(by_cmc)):
        running += sum(c[i] for c in colored)
        if running > colored_total * 0.65:
            pl.curve_cmc = i
            break
    pl.avg_cmc = round(total_cmc / nonlands, 2) if nonlands else 0.0
    # Efficiency rewards a deck for doing its work early: blend the average cost with the point the
    # deck's impact actually arrives, then place that between the cmc floor and ceiling.
    midpoint = (pl.avg_cmc + pl.curve_cmc) / 2
    ratio = (EDHPL_CMC_CEILING - midpoint) / (EDHPL_CMC_CEILING - EDHPL_CMC_FLOOR)
    lo, hi = EDHPL_EFFICIENCY_LIMITS
    pl.efficiency = lo + (hi - lo) * ratio
    pl.impact = total_impact
    pl.score = total_impact * pl.efficiency
    pl.level = edhpl_curve(pl.score, EDHPL_POWER_CURVE)
    return pl


def edhpowerlevel_url(lines: list[str]) -> str:
    """Python port of EDHPowerLevelEncode() from https://edhpowerlevel.com/button/."""
    out = []
    for c in lines:
        c = c.split("(")[0]
        c = re.sub(r" *\[[^)]*\] *", " ", c)
        c = re.sub(r" *<[^)]*> *", " ", c)
        out.append(c.strip())
    enc = urllib.parse.quote("~".join(out), safe="-_.!~*'()").replace("%20", "+")
    return "https://edhpowerlevel.com?d=" + enc + "~Z~"
