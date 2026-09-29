#!/usr/bin/env python3
"""Compare tools/edhpl_reference.js output against the committed golden values.

    node tools/edhpl_reference.js /tmp/decks.json > /tmp/js_scores.json
    python tools/compare_reference.py /tmp/js_scores.json

Exits non-zero on any mismatch. Use this after touching the reference or the port; the test suite
checks the *Python* against the same golden file, so agreement here means all three agree.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "tests" / "data" / "powerlevel_golden.json"
DECKS = ROOT / "tests" / "data" / "decks"
FIELDS = {"level": "level", "score": "score", "impact": "impact",
          "efficiency": "efficiency", "avgCost": "avg_cmc", "curveCmc": "curve_cmc"}
TOLERANCE = 1e-9


def main() -> int:
    if len(sys.argv) != 2:
        sys.exit(__doc__.strip().splitlines()[0])
    js = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    golden = json.loads(GOLDEN.read_text(encoding="utf-8"))
    names = sorted(p.name for p in DECKS.glob("*.txt"))
    if len(js) != len(names):
        sys.exit(f"reference scored {len(js)} decks but there are {len(names)} in {DECKS}")

    bad = 0
    for name, scored in zip(names, js):
        want = golden.get(name)
        if want is None:
            print(f"MISSING golden entry: {name}")
            bad += 1
            continue
        for js_key, golden_key in FIELDS.items():
            if abs(scored[js_key] - want[golden_key]) > TOLERANCE:
                print(f"MISMATCH {name}.{golden_key}: reference={scored[js_key]!r} "
                      f"golden={want[golden_key]!r}")
                bad += 1
    print(f"FAIL: {bad} mismatch(es)" if bad else f"OK: reference matches golden on all {len(names)} decks")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
