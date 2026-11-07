# edh-scout

Find **well-rounded, fun Commander decklists** for a given commander, ready for a normal commander
night. edh-scout pulls decklists from EDHREC, scores every one the way
[EDHPowerLevel](https://edhpowerlevel.com) does (power level and bracket, computed locally), adds
EDHREC's salt score, a deckbuilding-template check and synergy with the commander, and lets you
browse, compare and export the best fits — e.g. to print proxies.

```
$ edh-scout scout "Jodah, the Unifier" --plain
╭───┬───────┬─────────┬──────┬───────────┬─────────┬─────────────────────────────────────────────────────────────────────────╮
│ # ┆ Power ┆ Bracket ┆ Salt ┆ Structure ┆ Synergy ┆ Themes                                                                  │
╞═══╪═══════╪═════════╪══════╪═══════════╪═════════╪═════════════════════════════════════════════════════════════════════════╡
│ 1 ┆   6.2 ┆ B3      ┆   50 ┆       83% ┆    1.8% ┆ Enchantress, Unnatural, Lands Matter, Cascade, Legends, Ramp, Artifacts │
│ 2 ┆   6.1 ┆ B2      ┆   47 ┆      100% ┆    5.5% ┆ Time Lords, Anthems, Legends                                            │
│ 3 ┆   6.1 ┆ B3      ┆   52 ┆       91% ┆    2.2% ┆ Commander Matters, +1/+1 Counters, Modified Creatures, Humans           │
│ 4 ┆   6.3 ┆ B2      ┆   52 ┆      100% ┆    6.7% ┆ Legends                                                                 │
│ 5 ┆   6.0 ┆ B3      ┆   51 ┆       96% ┆    2.3% ┆ Legends, Historic                                                       │
│ 6 ┆   6.4 ┆ B2      ┆   51 ┆       94% ┆   16.2% ┆ –                                                                       │
╰───┴───────┴─────────┴──────┴───────────┴─────────┴─────────────────────────────────────────────────────────────────────────╯
```

Without `--plain` the same ranking opens in an interactive browser with a detail pane per deck
(power bar against the target band, bracket reasons, salty cards, role counts, synergy, combos) and
the full decklist one keypress away.

## Install

```sh
nix run . -- scout "Krenko, Mob Boss"   # from a checkout
nix build                               # ./result/bin/edh-scout
cargo install --path crates/edh-scout
```

The first run downloads the card and combo databases (see *Data sources*); after that everything
is cached.

## Use

```sh
edh-scout scout "Jodah, the Unifier"                 # interactive browser, target brackets 2–3
edh-scout scout tymna --partner kraum                # partners; partial names work
edh-scout scout krenko -b 3 --max-price 300          # bracket 3 only, budget cap
edh-scout scout krenko --price                       # also show EDHREC's deck prices
edh-scout scout krenko -p 5.5-7                      # explicit EDHPowerLevel band instead
edh-scout scout krenko --rank combined --weights power=0.4,salt=0.4,structure=0.2
edh-scout scout krenko -t Tokens -t Goblins          # only decks EDHREC tagged with a theme
edh-scout scout krenko --plain --top 15 --export out # table on stdout + report.md + decklists
edh-scout check mydeck.txt                           # score a local list (any common export format)
edh-scout sync                                       # refresh prices / Game Changers / combos
```

Names are matched loosely (any order, case, accents and punctuation ignored: `kraum lud`, `ludevics`).
If several commanders match, or none do, a picker opens below the prompt, prefilled with what you
typed: keep typing to narrow it, `↑↓` and `⏎` to choose (each row shows color identity and EDHREC
rank to tell look-alikes apart). Without a terminal you get the list of matches instead.

In the browser: `↑↓` move · `⏎` decklist/details · `J/K` scroll · `r` cycle ranking ·
`f` only decks in the target · `o`/`s`/`p` open on EDHREC / the source site / EDHPowerLevel ·
`e` export the selected list · `E` export `report.md` plus the top lists · `q` quit.

When stdout isn't a terminal (or with `--plain`) you get a table instead.

### Exports (proxies)

Decklists are written as plain `1 Card Name` lines, commander first, nothing else — the format
MPCFill and most proxy printers take, and that Moxfield/Archidekt import cleanly. `--format
sectioned` writes `Commander` / `Deck` sections instead. `report.md` links every deck to EDHREC,
its source page and a prefilled EDHPowerLevel check.

## How decks are scored

| Signal | Meaning |
|---|---|
| **Power** | EDHPowerLevel's 0–10 score (card prices & popularity, scaled by curve efficiency). |
| **Bracket** | EDHPowerLevel's bracket: the higher of its rules floor (extra turns, land denial, Game Changers, early/late 2-card combos) and the bracket the power level implies. |
| **Salt** | EDHREC's deck salt (sum of its cards' salt). The detail view lists the saltiest cards. |
| **Structure** | Closeness to a ~36 land / 10 ramp / 10 draw / 8 removal / 2 wipe template, using Scryfall Tagger's community card tags. |
| **Synergy** | Average EDHREC synergy of the deck's cards with this commander: how on-plan it is. |

**Target.** `-b 2-3` (default) means "EDHPowerLevel would call it bracket 2 or 3", i.e. power
4.7–7.7 using the site's own power→bracket curve, with no rule pushing it higher. `-p` sets an
explicit power band instead.

**Ranking.** `--rank power` (default) puts decks in the target first, then those whose power sits
closest to the middle of the band. `combined` blends power fit, salt, structure and synergy with
`--weights` (default `power=0.5,salt=0.2,structure=0.15,synergy=0.15`); `salt`, `structure` and
`synergy` sort by one signal. Salt and synergy are scored relative to the decks being compared.
Press `r` in the browser to cycle.

**Which decks get fetched.** EDHREC lists tens of thousands of decks for popular commanders with
no quality signal, so the deck table (one request) is filtered first: incomplete lists, decks
declared outside the target, and duplicates are dropped; a salt ceiling is calibrated
from the decks whose owners declared a target bracket (their 90th percentile; with `--price` a
price ceiling too); the fetch budget
(`-n`, default 60) is then spread across themes and salt/price tiers. Decks fetched earlier are
reused for free, so repeated runs grow the pool.

## Data sources and politeness

| Source | Used for | Requests |
|---|---|---|
| EDHREC | deck table, deck pages, commander synergy, combo votes, top-salt list | 1 table + 3 side pages per commander (cached 1–14 days); 1 per new deck (cached forever) |
| Scryfall | `default_cards` bulk (cheapest printing prices, Game Changers, oracle text) and `oracle_tags` bulk (card roles) | 4 per `sync` |
| Commander Spellbook | full combo dump | 1 per `sync` |
| EDHPowerLevel | nothing — scored locally; links go to the site | 0 |

Requests are strictly sequential with a per-host minimum gap (2 s for EDHREC deck pages, 1 s for
EDHREC JSON, 150 ms for Scryfall), `Retry-After` is honoured, and a host that rate-limits twice is
left alone for the rest of the run. Every run ends with a count of the requests it made. `--offline`
never touches the network.

Where things live (XDG):

| Path | Contents | If deleted |
|---|---|---|
| `$XDG_CACHE_HOME/edh-scout` (`~/.cache/…`) | bulk downloads, the card/combo databases built from them, EDHREC pages that expire anyway | re-downloaded by the next `sync` / run |
| `$XDG_STATE_HOME/edh-scout/decks` (`~/.local/state/…`) | every fetched decklist, as `<urlhash>.html` | each deck costs a request again |

Override with `--cache-dir` / `--state-dir` (or `EDH_SCOUT_CACHE` / `EDH_SCOUT_STATE`). On macOS and
Windows the platform's cache and local app-data directories are used instead.

A first `scout` for a commander costs about 64 requests (≈2 minutes at the default pace), but you
don't wait for them: the browser opens as soon as the deck table is in, with the decks already on
disk, and new decks slot into the ranking as they arrive (the header shows the progress; quitting
stops the fetching). Running it again within a day costs no requests at all.

## Development

```sh
nix develop          # cargo, clippy, rustfmt, rust-analyzer, node
cargo test --workspace
cargo clippy --workspace --all-targets
```

The test suite is offline: EDHREC payloads, deck fixtures and golden values live in
`crates/*/tests/data`.

```
crates/
  edhpl/       EDHPowerLevel's power level + bracket, ported from the site (see its README)
  edh-data/    polite cached HTTP, Scryfall/Spellbook databases and sync, EDHREC readers, decks
  edh-scout/   the app: analysis, roles, ranking, candidate selection, exports, CLI and TUI
```
