"""Reading EDHREC: the deck table for a commander, and the contents of individual decks.

Both come from the undocumented JSON their own frontend consumes, so the payload shapes are
defensive -- `edhrec_cards` in particular has to cope with a format that has already moved once.
"""
from __future__ import annotations

import re
import time
import urllib.error
from collections import Counter

from .config import CACHE, EDHREC_JSON, EDHREC_SITE
from .deck import Deck, slugify
from .fetch import Http


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


def _entry(e) -> tuple[str, int]:
    """A card entry is ["Sol Ring", 1] now; older payloads used bare names or objects."""
    if isinstance(e, (list, tuple)):
        name, qty = (list(e) + [1])[:2]
    elif isinstance(e, dict):
        name, qty = e.get("name") or e.get("card"), e.get("quantity") or e.get("q") or 1
    else:
        name, qty = e, 1
    return str(name or ""), int(qty or 1)


def edhrec_cards(data: dict) -> Counter:
    """deckpreview keeps the 99 in data['deck']['cards'], grouped by card type:
    {"Creature": [["Birds of Paradise", 1], ...], "Land": [["Plains", 6], ...]}."""
    deck = data.get("deck")
    groups = deck.get("cards") if isinstance(deck, dict) else None
    if isinstance(groups, dict):
        entries = [e for g in groups.values() for e in (g or [])]
    elif isinstance(groups, list):
        entries = groups
    else:                                       # older shape: a flat list of names at the top level
        entries = data.get("cards") or (deck if isinstance(deck, list) else [])
    out = Counter()
    for e in entries:
        name, qty = _entry(e)
        if name:
            out[name] += qty
    return out


def edhrec_commanders(data: dict) -> list[str]:
    cmdrs = [str(c) for c in data.get("commanders") or [] if c]
    deck = data.get("deck") if isinstance(data.get("deck"), dict) else {}
    for key in ("commander_v2", "commander"):
        if cmdrs:
            break
        cmdrs = [n for n, _ in map(_entry, deck.get(key) or []) if n]
    return cmdrs


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
    cmdrs = edhrec_commanders(data)
    cards = edhrec_cards(data)
    for c in cmdrs:
        cards.pop(c, None)
    if not cards:   # loud: an empty list means their payload moved again, not a 1-card deck
        raise ValueError("no cards in the deckpreview payload -- EDHREC's format may have changed")
    meta = {k: data.get(k) for k in ("price", "salt", "cedh", "theme", "tribe", "tags", "savedate")}
    return Deck(commanders=cmdrs, cards=cards, ident=urlhash, source=data.get("url") or "",
                edhrec=f"{EDHREC_SITE}/deckpreview/{urlhash}", meta=meta)
