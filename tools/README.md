# EDHPowerLevel reference implementation

`src/edh_scout/powerlevel.py` is a Python port of the power-level scoring that
[edhpowerlevel.com](https://edhpowerlevel.com) runs in the browser. The site has no API, so the port
was derived by reading their shipped JavaScript bundle.

`edhpl_reference.js` is that scoring transcribed back out of the bundle, expression for expression,
including the two quirks the Python port deliberately reproduces:

- the curve function scales the stop index by `step` but **not** the interpolated fraction;
- a value landing exactly on the last curve stop falls through and returns `0`.

`edhpl_overrides.json` is their per-card override table (price/cmc/impact/commander multipliers),
extracted from the same bundle. The `producer` entries only feed the site's playability stat, which
their own documentation says is not used for power level, so the Python port ignores them.

## Why keep it

It is the ground truth for the port. When EDHPowerLevel retunes its constants — and the module
docstring warns that they do — this is how you check what actually changed rather than re-deriving
the algorithm from scratch.

## Re-verifying the port

`tests/data/powerlevel_golden.json` holds the expected scores for the decks in `tests/data/decks/`,
and `tests/test_powerlevel.py` pins the Python against them. Those golden values came from this
script. To regenerate or re-check them:

```sh
# 1. dump the deck card data the reference needs (uses the committed card fixture)
python tools/dump_reference_input.py > /tmp/decks.json

# 2. score them with the JavaScript reference
node tools/edhpl_reference.js /tmp/decks.json > /tmp/js_scores.json

# 3. compare against the committed golden values
python tools/compare_reference.py /tmp/js_scores.json
```

## Re-deriving after an upstream change

1. Fetch `https://edhpowerlevel.com` and pull the `main-*.js` bundle it references.
2. Search it for `efficiencyLimits` — the `factors` object next to it holds every constant
   (`priceCurve`, `popCurve`, `powerCurve`, `land`, `reserved`, `favorPrice`, `cmcFloor`,
   `cmcCeiling`).
3. Search for `U.impact=(` for the per-card scoring, and for `cmcImpactTotalR` for the aggregation,
   efficiency blend and final curve lookup.
4. Update `edhpl_reference.js` and `edhpl_overrides.json`, then `src/edh_scout/powerlevel.py`, then
   regenerate the golden values above.
