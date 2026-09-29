"""Command line entry point: `edh-scout sync | scout | check`."""
from __future__ import annotations

import argparse
import random
import sys
import urllib.error
from pathlib import Path

from .cards import CardDB, build_cards_db, build_combo_db
from .config import CACHE, DB_CARDS, DB_COMBOS, SCRYFALL, SPELLBOOK_DUMP, UA, log
from .deck import bracket_spec, commander_slug, fmt_brackets, parse_decklist, slugify
from .edhrec import edhrec_deck, edhrec_deck_table
from .evaluate import evaluate
from .fetch import Http, OfflineMiss
from .report import emit
from .storage import load_gz, save_gz


def cmd_sync(args, http: Http) -> None:
    db = None
    if not args.skip_cards:
        meta = http.json(f"{SCRYFALL}/bulk-data/oracle-cards", ttl=0)
        # Scryfall replaced the plain-JSON download with gzipped JSON Lines (jsonl_download_uri);
        # keep reading download_uri/size too in case either form comes back.
        url = meta.get("jsonl_download_uri") or meta.get("download_uri")
        if not url:
            sys.exit("Scryfall's bulk-data entry has no download URI -- did their API change?")
        size = meta.get("compressed_size") or meta.get("size") or 0
        raw = CACHE / ("scryfall-oracle-cards.jsonl.gz" if url.endswith(".gz") else
                       "scryfall-oracle-cards.json")
        log(f"downloading Scryfall oracle cards (~{size / 1e6:.0f} MB) ...")
        http.download(url, raw)
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
    # EDHREC's deck table carries the bracket the deck's submitter declared (1-5, often absent).
    want = args.declared_bracket
    if want:
        kept = [r for r in rows if r.get("bracket") in want
                or (r.get("bracket") is None and args.include_undeclared)]
        log(f"{len(rows)} decks -> {len(kept)} declared {fmt_brackets(want)}"
            + (" (+ undeclared)" if args.include_undeclared else ""))
        if not kept:
            sys.exit(f"no decks declared {fmt_brackets(want)} for '{slug}' -- most EDHREC decks carry "
                     "no declared bracket; try --include-undeclared or a wider spec")
        rows = kept
    # Bracket 5 *is* the cEDH label, so asking for it explicitly opts in.
    allow_cedh = args.include_cedh or bool(want and 5 in want)
    if not allow_cedh:
        rows = [r for r in rows if r.get("bracket") != 5]
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
        # deckpreview no longer carries a `cedh` flag (the table's bracket 5 stands in, filtered above).
        if deck.meta.get("cedh") and not allow_cedh:
            continue
        deck.meta.update({k: row[k] for k in ("price", "salt", "savedate", "bracket")
                          if row.get(k) is not None})
        deck.commanders = deck.commanders or commanders
        res = evaluate(deck, db, http, args.spellbook, args.power_level)
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
    r = evaluate(deck, CardDB.load(http), http, args.spellbook, args.power_level)
    emit([(deck, r)], args.bracket, Path(args.out or f"edh-scout-check-{deck.ident}"), path.stem, False)


def build_parser() -> argparse.ArgumentParser:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--offline", action="store_true", help="never touch the network; cache/DB only")
    common.add_argument("--user-agent", default=UA)
    evalopts = argparse.ArgumentParser(add_help=False)
    evalopts.add_argument("--bracket", type=int, default=3, choices=range(1, 6), help="target bracket")
    evalopts.add_argument("--spellbook", action="store_true",
                          help="ask Commander Spellbook's /estimate-bracket (online, more accurate)")
    evalopts.add_argument("--no-power-level", dest="power_level", action="store_false",
                          help="skip the 1-10 EDHPowerLevel score (shown by default)")
    evalopts.add_argument("--out", help="output directory")

    p = argparse.ArgumentParser(prog="edh-scout", description="EDHREC deck scouting + bracket triage")
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
    s.add_argument("--declared-bracket", type=bracket_spec, metavar="SPEC",
                   help="only decks whose submitter declared this bracket on EDHREC "
                        "(e.g. 3, 1-3, 2,4); undeclared decks are dropped")
    s.add_argument("--include-undeclared", action="store_true",
                   help="with --declared-bracket, also keep decks that declared no bracket")
    s.add_argument("--include-cedh", action="store_true", help="don't skip decks EDHREC tags as cEDH")
    s.add_argument("--only-fitting", action="store_true", help="only report decks that fit --bracket")

    s = sub.add_parser("check", parents=[common, evalopts], help="evaluate a local decklist file")
    s.add_argument("file")
    s.add_argument("--commander", action="append", help="commander name (repeat for partners)")
    return p


def main(argv=None) -> None:
    args = build_parser().parse_args(argv)
    http = Http(args.offline, args.user_agent)
    try:
        {"sync": cmd_sync, "scout": cmd_scout, "check": cmd_check}[args.cmd](args, http)
    except OfflineMiss as e:
        sys.exit(f"--offline and not cached: {e}")
    except urllib.error.HTTPError as e:
        sys.exit(f"HTTP {e.code} for {e.url}" + (" (try --user-agent)" if e.code == 403 else ""))
    except KeyboardInterrupt:
        sys.exit(130)
