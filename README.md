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
edh-scout scout -T planeswalkers=10+                 # superfriends decks, across commanders
edh-scout scout -T spells=30+ -b 3                   # spell-heavy decks at bracket 3
edh-scout scout -T creatures=..5                     # (nearly) creatureless decks
edh-scout scout jodah -T planeswalkers=10+           # one commander's decks, narrowed by type
edh-scout scout -T equipment=10+                     # equipment decks: any type-line word works
edh-scout scout jodah -T legendary=30+ -T ramp=12+   # legends-heavy Jodah lists with real ramp
edh-scout scout -T 'otag:sweeper=4+' -b 3            # Scryfall searches count too (Tagger tags here)
edh-scout scout krenko -p 5.5-7                      # explicit EDHPowerLevel band instead
edh-scout scout krenko --rank combined --weights fit=0.4,salt=0.4,structure=0.2
edh-scout scout krenko -t Tokens -t Goblins          # only decks EDHREC tagged with a theme
edh-scout scout krenko --plain --top 15 --export out # table on stdout + report.md + decklists
edh-scout check mydeck.txt                           # score a local list (any common export format)
edh-scout check mydeck.txt -T equipment -T ramp=10+  # ... and count what it runs
edh-scout sync                                       # refresh prices / Game Changers / combos
```

Names are matched loosely (any order, case, accents and punctuation ignored: `kraum lud`, `ludevics`).
If several commanders match, or none do, a picker opens below the prompt, prefilled with what you
typed: keep typing to narrow it, `↑↓` and `⏎` to choose (each row shows color identity and EDHREC
rank to tell look-alikes apart). Without a terminal you get the list of matches instead.

### Searching by what decks run

`-T WHAT=RANGE` filters on how many cards of a kind a deck runs. Ranges: `10+`, `..5`, `25-40` or an
exact number. Repeat `-T` to combine filters; all must match, and a card can count towards several.
Without a range (`-T equipment`, `-T 'otag:sweeper'`) a decklist count only adds a column, it
doesn't filter. `check` takes the same `-T` to count a local list, marking counts outside a range.

| `WHAT` | Counts | Checked |
|---|---|---|
| `creatures`, `instants`, `sorceries`, `spells` (instants + sorceries), `artifacts`, `enchantments`, `planeswalkers`, `battles`, `lands` | EDHREC's per-deck counts | from the deck table: free, before any deck is fetched |
| `ramp`, `draw`, `removal`, `wipes` | cards in that role, exactly as the Structure line counts them | in the decklist |
| any other word: `equipment`, `legendary`, `saga`, `aura`, `vehicle`, `goblin`, `snow`, … | cards with that word on their type line (`t:WORD`; plurals work too) | in the decklist |
| a Scryfall search: `'o:"create a treasure"'`, `'otag:sweeper'`, `'t:equipment mv<=2'` | cards it matches | in the decklist |

```sh
edh-scout scout -T equipment=10+ -T 'otag:removal=..6'   # equipment decks that don't lean on removal
edh-scout scout "tom bombadil" -T saga=12+                # the most saga-heavy Tom lists
edh-scout scout -T 'o:"create a treasure"=8+' -b 2        # treasure decks at bracket 2
edh-scout scout krenko -T '(t:goblin -t:creature)=3+'     # Krenko lists with goblin support spells
```

Decklist counts cover the 99, like EDHREC's: commanders don't count, quantities do (`t:basic=30+`
sees 30 Mountains). EDHREC files each card under one main type, so `artifacts=10+` and
`t:artifact=10+` (everything with Artifact on its type line, artifact creatures included) can
differ. Cards missing from the card database don't count.

Searches run locally against the card database (no requests, `--offline` works) and understand
this part of [Scryfall's syntax](https://scryfall.com/docs/syntax):

| Syntax | Matches |
|---|---|
| `t:` `type:` | type line contains the text (either face; partial words fine) |
| `o:` `oracle:` | rules text without reminder text; `~` is the card itself (its name, "this creature", …) |
| `fo:` | rules text including reminder text |
| `t:/…/`, `o:/…/`, `name:/…/` | a case-insensitive regular expression |
| `kw:` `keyword:` | keyword abilities (`kw:flying`) |
| `mv`, `cmc`, `usd` | numbers with `: = != < <= > >=` (`mv<=2`; `usd` is the cheapest printing) |
| `c`, `id` | colour and colour identity: `c:rg` (at least red and green), `id:esper` (fits Esper), `c:m`, `c:c` |
| `is:` / `not:` | `gamechanger`, `commander`, `reserved`, `historic`, `permanent`, `mdfc`, `dfc` |
| `otag:` `function:` | a [Scryfall Tagger](https://tagger.scryfall.com) tag and its sub-tags (`otag:ramp`, `otag:mana-rock`, `otag:sweeper`, `otag:tutor`) |
| `name:`, bare words, `"…"`, `!"…"` | card names; `!` for one exact card |
| `or`, `-`, `( )` | terms side by side must all match; `or`, negation, grouping |

Other keys (set, rarity, artist, …) are an error, never silently ignored. So is a filter that
matches no card at all: usually a typo. The range is whatever follows the *last* `=`, unless that
`=` belongs to the search: `mv=3` alone counts mana value 3 cards, `mv=3=10+` filters on them.

**Fetching.** The deck table doesn't say what's in a deck, so decklist filters are checked as each
deck arrives: decks that fail are left out and counted (`12 didn't match` in the header). They stay
in the deck store, so any later search reads them for free. `-n` still counts decks fetched, not
matches; the browser keeps fetching past it, and a plain run tells you when running again would
help. In the browser, each decklist filter gets a count column (except one that lets a single count
through, like `sagas=0`, since every deck shown has it), the detail pane lists the cards it
counted, and the decklist marks them with `▸`.

The deck table still helps pick which decks to fetch for a minimum. Matching cards only sit in some
of its type columns (equipment are counted as artifacts, or creatures for the few that are both),
so a deck whose columns can't add up to the minimum is skipped, and within each theme and tier the
decks with the most cards where matches usually sit come first (each column weighted by how much of
what decks play there matches, by EDHREC popularity). That works best for kinds that live in one
column (equipment, auras, sagas, instants); ramp, which spreads over every type, gains less.

**With a commander**, `-T` narrows that commander's decks. **Without one**, it searches across
commanders: the filters pick EDHREC themes and their top commanders are scanned. Table counts map to
`planeswalkers`, `spellslinger`, `creatureless`, `artifacts`, `enchantress` and `lands-matter`;
decklist filters to the theme named after what they count (`equipment` → Equipment, `saga` → Sagas,
`legendary` → Legends, `ramp` → Ramp, `o:treasure` → Treasure), looked up in EDHREC's theme list.
Commanders whose own rules text mentions what a decklist filter counts (Sram's "Aura, Equipment, or
Vehicle spell" for `equipment=10+`) fill the slots the themes leave, most popular first, found in
the card database for free. When neither fits (e.g. `creatures=40+`), EDHREC's most-built
commanders are scanned instead.
`--commanders N` (default 30) caps how many deck tables are scanned. The browser opens right away
and fills as each commander's table and decks come in; commander and colour (WUBRG) columns show
whose deck is whose.

**Variety.** Commanders get room in proportion to the square root of how many eligible decks EDHREC
lists for them, so the most popular commander for a kind of deck shows up most, but not to the
exclusion of the rest. Decks you already have count towards that share: a commander with 40 stored
decks waits while the others are fetched up to their share. The ranking interleaves commanders the
same way, each giving its best remaining deck in turn (decks in the target still come first). `m`
in the browser, or `--no-mix`, ranks by score alone instead. Cost: one request for the theme list (when a decklist filter asks for a
minimum) and one per theme (both cached for a week), one per deck table scanned (cached for a day),
two per commander that ends up with decks (synergy and combo votes, cached for a week), plus the
usual deck budget.

In the browser: `↑↓` move · `⏎` decklist/details · `J/K` scroll · `r` cycle ranking ·
`f` only decks in the target · `m` mix commanders or rank by score alone (across commanders) ·
`o`/`s`/`p` open on EDHREC / the source site / EDHPowerLevel ·
`e` export the selected list · `E` export `report.md` plus the top lists · `q` quit.

When stdout isn't a terminal (or with `--plain`) you get a table instead.

Once the `-n` budget is fetched, the browser keeps fetching the remaining eligible decks, in the
same order and at the same pace, until you quit (the header shows `fetching N/total`). Turn that
off in the config file; plain output always stops at the budget.

### Config file

Optional, at `$XDG_CONFIG_HOME/edh-scout/config.toml` (`~/.config/…`; override with `--config` or
`EDH_SCOUT_CONFIG`). Unknown keys are an error, so typos don't go unnoticed.

```toml
[tui]
# Keep fetching eligible decks past the -n budget while the browser is open (default: true).
keep_fetching = true
```

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

**Ranking.** Every ranking puts decks in the target first, then the rest. `--rank fit` (default)
sorts by how close the power level is to the middle of the band: 100% at the middle, 50% at the
edges, 0% one point outside (shown as "fit" next to the power bar). `power` sorts by power level,
highest first. `combined` blends fit, salt, structure and synergy with `--weights` (default
`fit=0.5,salt=0.2,structure=0.15,synergy=0.15`); `salt`, `structure` and `synergy` sort by one
signal. Salt and synergy are scored relative to the decks being compared.
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
| EDHREC | deck tables, deck pages, commander synergy, combo votes, top-salt list, theme list and pages | 1 table + 2 side pages per commander and 1 salt list (cached 1–14 days); 1 per new deck (kept for good); 1 per theme (+1 for the theme list) when searching across commanders |
| Scryfall | `default_cards` bulk (cheapest printing prices, Game Changers, oracle text) and `oracle_tags` bulk (card roles, `otag:` searches) | 4 per `sync` |
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
| `$XDG_CONFIG_HOME/edh-scout/config.toml` (`~/.config/…`) | optional settings, see [Config file](#config-file) | defaults apply |

Override with `--cache-dir` / `--state-dir` (or `EDH_SCOUT_CACHE` / `EDH_SCOUT_STATE`). On macOS and
Windows the platform's cache and local app-data directories are used instead.

A first `scout` for a commander costs about 64 requests (≈2 minutes at the default pace), but you
don't wait for them: the browser opens as soon as the deck table is in, with the decks already on
disk, and new decks slot into the ranking as they arrive (the header shows the progress; quitting
stops the fetching). Left open, it keeps fetching past the budget at one deck per 2 s. Running it again within a day costs no requests at all.

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
