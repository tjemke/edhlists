# edh-scout

Pull EDHREC decklists for a commander and triage them: what is the lowest **Commander bracket** each
deck can honestly claim, and what **power level** does it score?

```
  B>=   PL GC cmb      $  salt   MV tut  deck
✓   3  7.5  3   2   5060  61.4 2.94   0  DLSY0ucAq2iP44GSfNiOIw
      PL 7.5/10: impact 688 × efficiency 0.97 = score 666
      3 GC: Aura Shards, Necropotence, Notion Thief
      extra turns: Ichormoon Gauntlet
      combo [Spicy]: Vorinclex, Monstrous Raider + Vraska, Betrayal's Sting -> Target opponent loses
~   4  7.8  9   3   1061  79.1 3.67   9  f2fyso5moXX2yHyDUBTqtw
      cut 6 Game Changer(s) to fit B3
```

`✓` fits the target bracket, `~` fits after cuts, `✗` is above it.

## Install

```sh
pip install -e .
```

No runtime dependencies — standard library only, and intended to stay that way.

## Use

```sh
edh-scout sync                                    # one-time download -> compact local DB
edh-scout scout "Atraxa, Praetors' Voice" -n 12 --bracket 3
edh-scout scout "Tymna the Weaver" --partner "Kraum, Ludevic's Opus" --spellbook
edh-scout scout "Jodah, the Unifier" --declared-bracket 2-3   # trust EDHREC's own label
edh-scout check mydeck.txt --bracket 3
```

`python -m edh_scout ...` works identically.

`sync` first — it builds the local card and combo databases everything else reads. Re-run it every
few weeks to pick up Game Changer list changes and price movement.

Each run writes a directory containing `report.md` (one linkable section per deck) and `decks/*.txt`
(the decklists themselves, ready to paste elsewhere).

`check` reads most decklist formats: plain `1 Sol Ring`, Archidekt categories, `(SET) 123` suffixes,
`*CMDR*` markers, and `Commander` / `Deck` / `Sideboard` section headers.

### Useful flags

| Flag | Effect |
|---|---|
| `--bracket N` | Target bracket to triage against (default 3) |
| `--spellbook` | Ask Commander Spellbook's `/estimate-bracket` — slower, online, more accurate |
| `--no-power-level` | Skip the 1-10 score |
| `--declared-bracket SPEC` | Only decks whose submitter declared this bracket (`3`, `1-3`, `2,4`) |
| `--offline` | Never touch the network; cache and local DB only |
| `--sample recent\|random\|cheapest` | How to pick which decks to evaluate |

## What the two numbers mean

**Bracket floor (`B>=`)** is the *minimum* bracket a deck can claim, from hard signals only: Game
Changers, banned cards, mass land denial, extra turns and known combos. It cannot see speed, synergy
or pilot intent, so it is a floor, never a verdict. A deck can sit far above its floor.

**Power level (`PL`)** is [EDHPowerLevel](https://edhpowerlevel.com)'s 1-10 scale. Their site has no
API, so `src/edh_scout/powerlevel.py` reimplements the scoring they run in the browser: each card's
impact comes from its market price and EDHREC rank, the total is scaled by an efficiency factor
derived from the mana curve, and the result is read off a power curve.

Because the inputs are Scryfall's price and rank rather than EDHPowerLevel's own snapshot, the
number can drift a little from the site. Every deck in `report.md` carries a prefilled
`edhpowerlevel.com?d=` link, so checking is one click. See `tools/README.md` for how the port is
verified and how to re-derive it if they retune their constants.

## Data sources

All public and keyless. Everything is cached on disk; please keep it polite.

| Source | Used for |
|---|---|
| EDHREC | The undocumented JSON its own frontend uses: deck tables and deck contents |
| Scryfall | `oracle-cards` bulk: Game Changer flag, legality, oracle text, price, EDHREC rank |
| Commander Spellbook | Combo dump (offline) and `/estimate-bracket` (online, `--spellbook`) |
| EDHPowerLevel | No API; power level is scored locally, with prefilled links back to the site |

Cache and databases live in `$XDG_CACHE_HOME/edh-scout` (default `~/.cache/edh-scout`).

## Development

```sh
pip install -e ".[dev]"
pytest
```

The test suite is fully offline: card data is pinned in `tests/data/cards.json`, and the power-level
expectations in `tests/data/powerlevel_golden.json` and `tests/data/curve_golden.json` are derived
from a JavaScript transcription of EDHPowerLevel's own scoring (`tools/`). Regenerate fixtures with
`tools/build_card_fixture.py` and `tools/build_curve_fixture.js`.

### Layout

```
src/edh_scout/
  config.py       paths, endpoints, shared constants
  storage.py      gzip + streaming JSON readers for the bulk dumps
  fetch.py        polite, file-cached HTTP
  cards.py        card/combo DB building and lookup
  deck.py         the Deck model, decklist parsing, slugs
  edhrec.py       reading EDHREC's deck tables and deck contents
  powerlevel.py   the EDHPowerLevel port
  evaluate.py     bracket triage
  report.py       stdout table + report.md
  cli.py          argument parsing and the three subcommands
```
