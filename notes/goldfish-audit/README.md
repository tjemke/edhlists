# Blind audit of the mana goldfish's card readings

How the goldfish reads the most played mana and draw cards, checked by agents who read only the
oracle text and `brief.md`, never the code or its readings. Run it again after a Forge release or
a reader change: each run so far found misreads in cards decks are built around.

- `brief.md`: what the goldfish models, its accepted simplifications, and rulings from earlier runs.
  **Update it whenever the model changes on purpose**, or the next run flags the change.
- `select.py CENSUS OUT --batches K` (or `--pilot N SEED`): the population (mana cards ranked
  ≤ 3,800, draw-only cards ≤ 2,000; ~1,500) as blind inputs.
- `compare.py CENSUS OUT.jsonl…`: field-by-field disagreements with the census; `split.py` splits a
  report into mana and draw sides.
- `census_diff.py OLD NEW`: every reading a code change moves, across the whole pool.
- `draw_agree.py OLD NEW OUT.jsonl…`: how often two census runs agree with the agents on draw.
- `findings.md`: what the 2026-11-22 run and the closing deck round found, and what was fixed.
- `findings-2026-11-23.md`: the second run (forge-2.0.16), and what 0.3.9 fixed.
- `findings-2026-11-25.md`: the third run (forge-2.0.16, after 0.3.9 and the second audit's fixes).

The census itself: `CENSUS_OUT=DIR cargo test --release -p edh-goldfish census -- --ignored`
(`crates/edh-goldfish/src/census.rs`). Its output is derived from Forge's GPL scripts: keep it out
of the repo. The whole run is the saved workflow `goldfish-blind-audit`
(`.claude/workflows/goldfish-blind-audit.js`): census (syncing first on a fresh machine), 7 Sonnet
readers, a triage, a check that recounts from the raw files and verifies each misread against the
code, and a drafted report; about 850K tokens. Each run writes to a folder of its own,
`~/goldfish-audit/<date-time>`, outside the repo.

**After a run, write the report.** Subagents can't write report files, so the workflow returns the
draft (`report.markdown`) and its path (`findings-<date>.md` here). Save it, check its counts and
untriaged list against the run folder, add a line for it to the list above, and file `report.todo`
in TODO.md. The run folder doesn't outlive the machine, so the findings file is all that's kept.
