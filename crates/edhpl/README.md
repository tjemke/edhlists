# edhpl

A Rust port of [EDHPowerLevel](https://edhpowerlevel.com)'s scoring: the 0–10 **power level**, the
**Commander bracket** (rules floor + power-implied bracket), and the prefilled link back to the site.
The site has no API; its scoring is client-side JavaScript, so this crate reimplements it.

Pure computation, no I/O. Inputs use the site's own shapes (`DeckCard`, `Combo`), so the JavaScript
reference and the Rust tests read the very same fixture.

Ported from bundle `main-C1lbqCDd.js`, checked 2026-11-06.

## What the site computes

- **Power level.** Each card's impact = price-curve position (×1.25) + popularity-curve position
  from its EDHREC rank (×0.75), with per-card overrides (`data/overrides.json`), reserved-list
  discount, lands at 60%, basics at a flat 2. The total is scaled by an efficiency factor (average
  cmc blended with the cmc by which 65% of the coloured impact lands) and read off a power curve.
- **Bracket.** `max(rules floor, ceil(curve(level, bracketCurve)))`, where `bracketCurve` is
  `[0, 4.7, 6.7, 7.7, 9.25, 10]` — so B2 ≈ (4.7, 6.7], B3 ≈ (6.7, 7.7]. The rules floor counts
  extra-turn cards (and a "chainable" list), mass land denial (regex + list), Game Changers (more
  than 3 → B4), and Commander Spellbook 2-card combos: *early* (≤ 7 mana including battlefield
  pieces → B4) or *late* (→ B3).

Prices matter: the site prices a card at its **cheapest printing**. `edh-data` builds the card DB
from Scryfall's `default_cards` bulk for exactly that reason; with it, per-card impacts match the
site to within day-to-day price drift (checked against two live decks: 6.1 vs 6.01, 6.0 vs 6.00).

Quirks reproduced on purpose (marked "quirk" in the code): the curve scales only the stop index by
`step`; a value exactly on the last stop returns 0; a power level of exactly 10 maps to "bracket 0";
a combo needing any Spellbook template never counts as a 2-card combo; the land-denial regex anchors
`^` to the start of a face's text; the tutor rule is defined but never fed matches.

## Verifying the port

`reference/edhpl_reference.js` is the site's code transcribed from the bundle (everything between
the "verbatim" markers is copied as-is). `tests/golden.rs` checks the Rust against its output:

```sh
# rebuild the fixture from today's card/combo data (needs a synced cache), then the goldens
cargo run --example build_edhpl_fixture
node crates/edhpl/reference/edhpl_reference.js crates/edhpl/tests/data/decks.json \
    > crates/edhpl/tests/data/golden.json
node crates/edhpl/reference/build_curve_fixture.js
cargo test -p edhpl
```

`tests/data/decks/*.txt` are the input decklists; `bracket_bait.txt` exists to exercise every
bracket rule.

## Re-deriving after an upstream change

1. Fetch `https://edhpowerlevel.com`, note the `main-*.js` bundles it loads. If `main-C1lbqCDd.js`
   is still there, nothing changed (bundle names are content hashes).
2. In the new bundle, search for `efficiencyLimits` (the `factors` object: every curve and
   constant, including `bracketCurve`), `U.impact=(` (per-card scoring), `cmcImpactTotalR`
   (aggregation), `bracketHelper` (bracket rules, card lists, maxes) and `producers:` (the combo
   feature filter). The override table is the `Ba` array.
3. Update `reference/edhpl_reference.js` and `data/overrides.json`, then the Rust, then regenerate
   the goldens as above.
