#!/usr/bin/env python3
"""edh_scout — pull EDHREC decklists for a commander and triage them by Commander bracket.

  python3 edh_scout.py sync                                   # one-time download -> compact local DB
  python3 edh_scout.py scout "Atraxa, Praetors' Voice" -n 12 --bracket 3
  python3 edh_scout.py scout "Tymna the Weaver" --partner "Kraum, Ludevic's Opus" --spellbook
  python3 edh_scout.py check mydeck.txt --bracket 3

Sources (public, keyless -- everything is cached, please keep it polite):
  EDHREC ............. undocumented JSON its own frontend uses (deck tables + deck contents)
  Scryfall ........... oracle-cards bulk: game_changer flag, legality, oracle text
  Commander Spellbook  combo dump (offline) and /estimate-bracket (online, --spellbook)
  EDHPowerLevel ...... no API; the report contains prefilled ?d= links (their documented format)

Stdlib only. Cache/DB live in $XDG_CACHE_HOME/edh-scout (default ~/.cache/edh-scout).
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import random
import re
import sys
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

VERSION = "0.1"
UA = f"edh-scout/{VERSION} (personal deck-testing script)"
CACHE = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "edh-scout"
DB_CARDS = CACHE / "cards.json.gz"
DB_COMBOS = CACHE / "combos.json.gz"

EDHREC_JSON = "https://json.edhrec.com/pages"
EDHREC_SITE = "https://edhrec.com"
SCRYFALL = "https://api.scryfall.com"
SPELLBOOK_API = "https://backend.commanderspellbook.com"
SPELLBOOK_DUMP = "https://json.commanderspellbook.com/variants.json.gz"

# Spellbook bracket tags -> bracket number (the mapping Spellbook itself uses).
TAG_BRACKET = {"E": 1, "C": 2, "O": 2, "P": 3, "S": 3, "R": 4, "B": 99}
TAG_NAME = {"E": "Exhibition", "C": "Core", "O": "Oddball", "P": "Powerful",
            "S": "Spicy", "R": "Ruthless", "B": "Banned"}

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


def log(msg: str) -> None:
    print(msg, file=sys.stderr)


# --- HTTP + cache -------------------------------------------------------------------------------

class OfflineMiss(Exception):
    """Raised when --offline is set and the requested thing isn't cached."""


class Http:
    GAP = {"edhrec.com": 1.0, "json.edhrec.com": 1.0,       # seconds between requests per host
           "backend.commanderspellbook.com": 0.8, "api.scryfall.com": 0.12}

    def __init__(self, offline: bool = False, user_agent: str = UA):
        self.offline, self.ua, self._last = offline, user_agent, {}

    def _open(self, url, data=None, headers=None, retries=4):
        if self.offline:
            raise OfflineMiss(url)
        host = urllib.parse.urlsplit(url).hostname or ""
        hdrs = {"User-Agent": self.ua, "Accept": "application/json, */*;q=0.5", **(headers or {})}
        for attempt in range(retries):
            wait = self._last.get(host, 0) + self.GAP.get(host, 0.5) - time.monotonic()
            if wait > 0:
                time.sleep(wait)
            self._last[host] = time.monotonic()
            try:
                req = urllib.request.Request(url, data=data, headers=hdrs)
                return urllib.request.urlopen(req, timeout=120)
            except urllib.error.HTTPError as e:
                if e.code in (429, 500, 502, 503, 504) and attempt + 1 < retries:
                    time.sleep(float(e.headers.get("Retry-After") or 2 ** (attempt + 1)))
                    continue
                raise
        raise RuntimeError("unreachable")

    def get(self, url, data=None, headers=None) -> bytes:
        with self._open(url, data, headers) as r:
            return r.read()

    def download(self, url, dest: Path) -> None:
        dest.parent.mkdir(parents=True, exist_ok=True)
        tmp = dest.with_name(dest.name + ".part")
        with self._open(url) as r, open(tmp, "wb") as f:
            while chunk := r.read(1 << 20):
                f.write(chunk)
        tmp.replace(dest)

    @staticmethod
    def _path(key: str) -> Path:
        return CACHE / "http" / (hashlib.sha256(key.encode()).hexdigest()[:32] + ".json")

    def peek(self, key: str):
        p = self._path(key)
        return json.loads(p.read_text()) if p.exists() else None

    def json(self, url, *, ttl=None, key=None, data=None, headers=None):
        """GET/POST JSON through a file cache. ttl=None caches forever; --offline reads cache only."""
        path = self._path(key or url)
        if path.exists() and (self.offline or ttl is None or time.time() - path.stat().st_mtime < ttl):
            return json.loads(path.read_text())
        obj = json.loads(gunzip_if_needed(self.get(url, data, headers)))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(obj))
        return obj


def gunzip_if_needed(b: bytes) -> bytes:
    while b[:2] == b"\x1f\x8b":
        b = gzip.decompress(b)
    return b


def open_text(path: Path) -> io.TextIOBase:
    with open(path, "rb") as f:
        gz = f.read(2) == b"\x1f\x8b"
    return io.TextIOWrapper(gzip.open(path, "rb") if gz else open(path, "rb"), encoding="utf-8")


def iter_json_array(fp: io.TextIOBase, key: str | None = None, chunk: int = 1 << 20):
    """Stream the items of a big JSON array (top-level, or under top-level `key`) without
    loading the whole document -- the Spellbook dump is several hundred MB uncompressed."""
    dec, buf, pos = json.JSONDecoder(), "", 0

    def fill() -> bool:
        nonlocal buf, pos
        data = fp.read(chunk)
        if not data:
            return False
        buf, pos = buf[pos:] + data, 0
        return True

    start = re.compile(r'"%s"\s*:\s*\[' % re.escape(key) if key else r"\s*\[")
    while not (m := start.search(buf) if key else start.match(buf)):
        if not fill():
            raise ValueError(f"JSON array {key or '<root>'} not found")
    pos = m.end()
    sep = re.compile(r"[\s,]*")
    while True:
        pos = sep.match(buf, pos).end()
        if pos >= len(buf):
            if not fill():
                return
            continue
        if buf[pos] == "]":
            return
        try:
            obj, end = dec.raw_decode(buf, pos)
        except json.JSONDecodeError:
            if not fill():
                raise
            continue
        yield obj
        pos = end


def save_gz(path: Path, obj) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with gzip.open(path, "wt", encoding="utf-8") as f:
        json.dump(obj, f, separators=(",", ":"))


def load_gz(path: Path):
    with gzip.open(path, "rt", encoding="utf-8") as f:
        return json.load(f)


# --- local databases ----------------------------------------------------------------------------

def build_cards_db(bulk_path: Path) -> dict:
    cards, aliases = {}, {}
    for c in iter_json_array(open_text(bulk_path)):
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
        cards[name] = {
            "gc": bool(c.get("game_changer")),
            "ban": legal == "banned",
            "mld": bool(MLD_RE.search(text)),
            "xt": bool(EXTRA_TURN_RE.search(text)),
            "tut": any(not LANDWORD_RE.search(m.group(0)) for m in TUTOR_RE.finditer(text)),
            "ramp": not land and mv <= 2 and bool(c.get("produced_mana") or LAND_RAMP_RE.search(text)),
            "mv": mv,
            "land": land,
        }
        aliases[name.lower()] = name
        for f in faces:
            aliases.setdefault(f.get("name", "").lower(), name)
    return {"built": time.time(), "cards": cards, "aliases": aliases}


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
    def __init__(self, cards: dict, aliases: dict, combos: list, partial: bool = False):
        self.cards, self.aliases, self.combos, self.partial = cards, aliases, combos, partial
        self._idx = defaultdict(list)
        for i, (_, _, names, _) in enumerate(combos):
            for n in names:
                self._idx[n].append(i)

    @classmethod
    def load(cls, http: Http) -> "CardDB":
        partial = False
        if DB_CARDS.exists():
            d = load_gz(DB_CARDS)
            age = (time.time() - d.get("built", 0)) / 86400
            if age > 30:
                log(f"note: card DB is {age:.0f} days old -- `sync` to pick up Game Changer changes")
            cards, aliases = d["cards"], d["aliases"]
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
        return cls(cards, aliases, combos, partial)

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


# --- decks --------------------------------------------------------------------------------------

@dataclass
class Deck:
    commanders: list[str]
    cards: Counter                      # the 99 (commanders excluded)
    ident: str = "deck"
    source: str = ""
    edhrec: str = ""
    meta: dict = field(default_factory=dict)

    def lines(self) -> list[str]:
        return ["Commander", *(f"1 {c}" for c in self.commanders), "", "Deck",
                *(f"{q} {n}" for n, q in sorted(self.cards.items()))]


DECKLINE_RE = re.compile(
    r"^(?:(\d+)\s*x?\s+)?(.+?)"
    r"(?:\s+\([A-Za-z0-9]{2,6}\)(?:\s+[\w★-]+)?)?"   # (SET) 123
    r"(?:\s+\[[^\]]*\])*"                             # [Archidekt categories]
    r"(?:\s+\*[A-Za-z]+\*)*\s*$")                     # *F* / *CMDR*
SECTIONS = {"commander", "commanders", "deck", "main", "mainboard", "sideboard", "maybeboard", "companion"}


def parse_decklist(text: str, commanders: list[str] | None = None) -> Deck:
    cmdrs, cards, section = list(commanders or []), Counter(), None
    for line in text.splitlines():
        s = line.strip()
        if not s or s.startswith(("//", "#")):
            continue
        if s.lower().rstrip(":") in SECTIONS:
            section = s.lower().rstrip(":")
            continue
        m = DECKLINE_RE.match(s)
        if not m or section in ("sideboard", "maybeboard"):
            continue
        qty, name = int(m.group(1) or 1), m.group(2).strip()
        if section in ("commander", "commanders") or "*CMDR*" in s or "[Commander" in s:
            if name not in cmdrs:
                cmdrs.append(name)
        else:
            cards[name] += qty
    for c in cmdrs:
        cards.pop(c, None)
    return Deck(commanders=cmdrs, cards=cards)


def slugify(name: str) -> str:
    s = unicodedata.normalize("NFKD", name.split(" // ")[0]).encode("ascii", "ignore").decode().lower()
    s = s.replace("'", "").replace("+", "plus-")
    return re.sub(r"[^a-z0-9]+", "-", s).strip("-")


def commander_slug(commanders: list[str]) -> str:
    # Partner pages appear to be ordered alphabetically -- pass --slug if EDHREC disagrees.
    return "-".join(sorted(slugify(c) for c in commanders))


def edhrec_deck_table(http: Http, slug: str, theme=None, budget=None) -> list[dict]:
    path = "/".join(p for p in (slug, theme and slugify(theme), budget) if p)
    return http.json(f"{EDHREC_JSON}/decks/{path}.json", ttl=86400).get("table") or []


def edhrec_build_id(http: Http, refresh: bool = False) -> str:
    f = CACHE / "edhrec-build-id"
    if not refresh and f.exists() and (http.offline or time.time() - f.stat().st_mtime < 6 * 3600):
        return f.read_text().strip()
    html = http.get(EDHREC_SITE + "/").decode("utf-8", "replace")
    m = (re.search(r'"buildId"\s*:\s*"([^"]+)"', html)
         or re.search(r"/_next/static/([A-Za-z0-9_-]{16,}?)/", html))
    if not m:
        raise RuntimeError("couldn't find EDHREC's Next.js buildId -- did their frontend change?")
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text(m.group(1))
    return m.group(1)


def edhrec_deck(http: Http, urlhash: str) -> Deck:
    key = f"edhrec:deckpreview:{urlhash}"      # build-id independent, decks are snapshots: cache forever
    page = http.peek(key)
    if page is None:
        for refresh in (False, True):          # a stale buildId 404s -> refresh once
            bid = edhrec_build_id(http, refresh)
            try:
                page = http.json(f"{EDHREC_SITE}/_next/data/{bid}/deckpreview/{urlhash}.json", key=key)
                break
            except urllib.error.HTTPError as e:
                if e.code != 404 or refresh:
                    raise
    data = (page.get("pageProps") or {}).get("data") or page
    cmdrs = [c for c in data.get("commanders") or [] if c]
    cards = Counter(c for c in data.get("cards") or [] if c and c not in cmdrs)
    meta = {k: data.get(k) for k in ("price", "salt", "cedh", "theme", "tribe", "tags", "savedate")}
    return Deck(commanders=cmdrs, cards=cards, ident=urlhash, source=data.get("url") or "",
                edhrec=f"{EDHREC_SITE}/deckpreview/{urlhash}", meta=meta)


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


# --- evaluation ---------------------------------------------------------------------------------

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


def evaluate(deck: Deck, db: CardDB, http: Http, use_spellbook: bool = False) -> Result:
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


# --- output -------------------------------------------------------------------------------------

def emit(results: list[tuple[Deck, Result]], target: int, out: Path, title: str, only_fit: bool) -> None:
    (out / "decks").mkdir(parents=True, exist_ok=True)
    results = sorted(results, key=lambda dr: (
        dr[1].floor > target, not dr[1].hint(target), dr[1].floor,
        len(dr[1].game_changers), dr[0].meta.get("price") or 0))
    md = [f"# {title}: bracket triage (target B{target})", "",
          f"Generated {time.strftime('%Y-%m-%d %H:%M')} by edh_scout {VERSION}. A floor is the *minimum* "
          "bracket implied by Game Changers, mass land denial, extra turns and known combos; it can't "
          "see speed, synergy or pilot intent. Combo data: [Commander Spellbook](https://commanderspellbook.com).",
          ""]
    print(f"\n{'':1} {'B>=':>3} {'GC':>2} {'cmb':>3} {'$':>6} {'salt':>5} {'MV':>4} {'tut':>3}  deck")
    shown = 0
    for deck, r in results:
        fit, hint = r.floor <= target, r.hint(target)
        if only_fit and not fit:
            continue
        shown += 1
        (out / "decks" / f"{deck.ident}.txt").write_text("\n".join(deck.lines()) + "\n", encoding="utf-8")
        mark = "✓" if fit else "~" if hint else "✗"
        floor = "ban" if r.floor >= 99 else str(r.floor)
        st, m = r.stats, deck.meta
        price = f"{m['price']:.0f}" if isinstance(m.get("price"), (int, float)) else "-"
        salt = f"{m['salt']:.1f}" if isinstance(m.get("salt"), (int, float)) else "-"
        print(f"{mark} {floor:>3} {len(r.game_changers):>2} {len(r.combos):>3} {price:>6} {salt:>5} "
              f"{st.get('avg_mv', '-'):>4} {st.get('tutors', '-'):>3}  {deck.ident}")
        for line in ([hint] if hint else []) + r.reasons():
            print(f"      {line}")

        links = [f"[EDHPowerLevel]({edhpowerlevel_url(deck.lines())})"]
        if deck.edhrec:
            links.insert(0, f"[EDHREC]({deck.edhrec})")
        if deck.source:
            links.insert(1, f"[source]({deck.source})")
        md += [f"## {mark} B≥{floor} · `{deck.ident}`", "",
               "- " + " · ".join(links) + f" · list: `decks/{deck.ident}.txt`",
               f"- ${price} · salt {salt} · {st.get('cards', '?')} cards · {st.get('lands', '?')} lands · "
               f"avg MV {st.get('avg_mv', '?')} · tutors {st.get('tutors', '?')} · cheap ramp {st.get('ramp', '?')}",
               f"- engine: {'Commander Spellbook /estimate-bracket' if r.engine == 'spellbook' else 'offline DB + heuristics'}"]
        md += [f"- **{hint}**"] if hint else []
        md += [f"- {line}" for line in r.reasons()]
        if deck.meta.get("tags"):
            md.append("- tags: " + ", ".join(map(str, deck.meta["tags"])))
        if r.unknown:
            md.append(f"- unrecognised names ({len(r.unknown)}): " + ", ".join(r.unknown[:10]))
        if st and st.get("cards", 100) < 95:
            md.append(f"- ⚠ only {st['cards']} cards in the EDHREC snapshot; list may be incomplete")
        md.append("")
    (out / "report.md").write_text("\n".join(md), encoding="utf-8")
    print(f"\n{shown} deck(s) -> {out}/report.md  (✓ fits B{target}, ~ fixable, ✗ above)")


# --- commands -----------------------------------------------------------------------------------

def cmd_sync(args, http: Http) -> None:
    db = None
    if not args.skip_cards:
        meta = http.json(f"{SCRYFALL}/bulk-data/oracle-cards", ttl=0)
        raw = CACHE / "scryfall-oracle-cards.json"
        log(f"downloading Scryfall oracle cards (~{meta.get('size', 0) / 1e6:.0f} MB) ...")
        http.download(meta["download_uri"], raw)
        db = build_cards_db(raw)
        save_gz(DB_CARDS, db)
        raw.unlink()
        log(f"  {len(db['cards'])} cards, {sum(c['gc'] for c in db['cards'].values())} Game Changers")
    if not args.skip_combos:
        if db is None and DB_CARDS.exists():
            db = load_gz(DB_CARDS)
        aliases = (db or {}).get("aliases", {})
        raw = CACHE / "spellbook-variants.json.gz"
        log("downloading Commander Spellbook combo dump ...")
        http.download(SPELLBOOK_DUMP, raw)
        combos = build_combo_db(raw, resolve=lambda n: aliases.get(n.lower()))
        save_gz(DB_COMBOS, combos)
        raw.unlink()
        log(f"  {len(combos['combos'])} combos ({combos['templated_skipped']} template-based ones "
            "skipped offline; --spellbook covers them)")


def cmd_scout(args, http: Http) -> None:
    commanders = [args.commander, *([args.partner] if args.partner else [])]
    slug = args.slug or commander_slug(commanders)
    rows = [r for r in edhrec_deck_table(http, slug, args.theme, "budget" if args.budget else None)
            if r.get("urlhash")]
    if not rows:
        sys.exit(f"no EDHREC decks for '{slug}' (check spelling or pass --slug)")
    if args.max_price:
        rows = [r for r in rows if (r.get("price") or 0) <= args.max_price]
    if args.sample == "random":
        random.Random(args.seed).shuffle(rows)
    elif args.sample == "cheapest":
        rows.sort(key=lambda r: r.get("price") or 1e9)
    else:
        rows.sort(key=lambda r: str(r.get("savedate") or ""), reverse=True)
    log(f"{len(rows)} candidate decks for {slug}; evaluating up to {args.n}")

    db = CardDB.load(http)
    results = []
    for row in rows:
        if len(results) >= args.n:
            break
        try:
            deck = edhrec_deck(http, row["urlhash"])
        except OfflineMiss:
            continue
        except (urllib.error.URLError, RuntimeError, ValueError, KeyError) as e:
            log(f"  skip {row['urlhash']}: {e}")
            continue
        if deck.meta.get("cedh") and not args.include_cedh:
            continue
        deck.meta.update({k: row[k] for k in ("price", "salt", "savedate") if row.get(k) is not None})
        deck.commanders = deck.commanders or commanders
        res = evaluate(deck, db, http, args.spellbook)
        results.append((deck, res))
        log(f"  [{len(results)}/{args.n}] {deck.ident}: B>={res.floor} ({len(res.game_changers)} GC)")
    if not results:
        sys.exit("nothing evaluated" + (" (offline: no cached decks)" if http.offline else ""))
    emit(results, args.bracket, Path(args.out or f"edh-scout-{slug}"), " + ".join(commanders),
         args.only_fitting)


def cmd_check(args, http: Http) -> None:
    path = Path(args.file)
    deck = parse_decklist(path.read_text(encoding="utf-8"), args.commander)
    deck.ident = slugify(path.stem) or "deck"
    if not deck.commanders:
        log("warning: no commander found (use --commander, a 'Commander' section, or *CMDR*)")
    r = evaluate(deck, CardDB.load(http), http, args.spellbook)
    emit([(deck, r)], args.bracket, Path(args.out or f"edh-scout-check-{deck.ident}"), path.stem, False)


def main(argv=None) -> None:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--offline", action="store_true", help="never touch the network; cache/DB only")
    common.add_argument("--user-agent", default=UA)
    evalopts = argparse.ArgumentParser(add_help=False)
    evalopts.add_argument("--bracket", type=int, default=3, choices=range(1, 6), help="target bracket")
    evalopts.add_argument("--spellbook", action="store_true",
                          help="ask Commander Spellbook's /estimate-bracket (online, more accurate)")
    evalopts.add_argument("--out", help="output directory")

    p = argparse.ArgumentParser(prog="edh_scout", description="EDHREC deck scouting + bracket triage")
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("sync", parents=[common], help="download/refresh local card + combo databases")
    s.add_argument("--skip-cards", action="store_true")
    s.add_argument("--skip-combos", action="store_true")

    s = sub.add_parser("scout", parents=[common, evalopts], help="pull and evaluate EDHREC decks")
    s.add_argument("commander")
    s.add_argument("--partner", help="second commander (partner/background/etc.)")
    s.add_argument("--slug", help="override the EDHREC commander slug")
    s.add_argument("-n", type=int, default=10, help="number of decks to evaluate (default 10)")
    s.add_argument("--sample", choices=("recent", "random", "cheapest"), default="recent")
    s.add_argument("--seed", type=int, help="seed for --sample random")
    s.add_argument("--max-price", type=float, help="skip decks above this EDHREC price (USD)")
    s.add_argument("--theme", help="EDHREC theme, e.g. 'counters' or 'infect'")
    s.add_argument("--budget", action="store_true", help="use EDHREC's budget deck subset")
    s.add_argument("--include-cedh", action="store_true", help="don't skip decks EDHREC tags as cEDH")
    s.add_argument("--only-fitting", action="store_true", help="only report decks that fit --bracket")

    s = sub.add_parser("check", parents=[common, evalopts], help="evaluate a local decklist file")
    s.add_argument("file")
    s.add_argument("--commander", action="append", help="commander name (repeat for partners)")

    args = p.parse_args(argv)
    http = Http(args.offline, args.user_agent)
    try:
        {"sync": cmd_sync, "scout": cmd_scout, "check": cmd_check}[args.cmd](args, http)
    except OfflineMiss as e:
        sys.exit(f"--offline and not cached: {e}")
    except urllib.error.HTTPError as e:
        sys.exit(f"HTTP {e.code} for {e.url}" + (" (try --user-agent)" if e.code == 403 else ""))
    except KeyboardInterrupt:
        sys.exit(130)


if __name__ == "__main__":
    main()
