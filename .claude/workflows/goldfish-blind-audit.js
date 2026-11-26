export const meta = {
  name: 'goldfish-blind-audit',
  description: 'Blind-read the ~1,500 most played mana and draw cards and triage where the goldfish reads them differently',
  whenToUse: 'After a Forge release or a change to the goldfish card readers (crates/edh-goldfish), to find misreads. About 850K tokens. Ends with a drafted report: write it to the path it returns (notes/goldfish-audit/findings-DATE.md), then do the follow-ups it lists.',
  phases: [
    { title: 'Census', detail: 'sync if needed, build the census into a fresh run folder, split the population into batches' },
    { title: 'Read', detail: 'blind readers, one per batch (Sonnet)', model: 'sonnet' },
    { title: 'Triage', detail: 'compare with the census, classify every mana disagreement and a draw sample' },
    { title: 'Check', detail: 'recount from the raw files, verify each real misread against the code' },
    { title: 'Report', detail: 'draft the findings file for notes/goldfish-audit' },
  ],
}

// args (all optional): { base: where run folders go, batches: number of readers,
// pilot: cards in a single small batch instead (to try the brief), target: CARGO_TARGET_DIR }
// Each run writes to a folder of its own under `base`, named by the census step for the time it
// started, so a later run can't overwrite one whose findings haven't been checked yet.
const A = args || {}
const BASE = A.base || '/home/agent/goldfish-audit'
const K = A.pilot ? 1 : (A.batches || 7)
const TARGET = A.target || '/home/agent/edh-target'
const NOTES = '/work/notes/goldfish-audit'

phase('Census')
const prep = await agent(
  `Prepare a blind audit run of the mana goldfish in /work. Run these, from /work, and report.

0. If ~/.cache/edh-scout/cards.json.gz or ~/.cache/edh-scout/forge-scripts.json.gz is missing (a
   fresh machine), run: CARGO_TARGET_DIR=${TARGET} nix develop -c cargo run --release -q -p edh-scout -- sync
1. RUN=${BASE}/$(date -u +%Y-%m-%d-%H%M); mkdir -p $RUN/batches; echo $RUN
   Use that folder as RUN below, and return it as \`run\` and its date (YYYY-MM-DD) as \`date\`.
2. CENSUS_OUT=$RUN/census CARGO_TARGET_DIR=${TARGET} nix develop -c cargo test --release -p edh-goldfish census -- --ignored --nocapture
   (it prints "N cards read from forge-… into …"; if it fails, return the error in \`error\`).
3. ${A.pilot
      ? `python3 -I ${NOTES}/select.py $RUN/census $RUN/batches --pilot ${A.pilot} 7 && mv $RUN/batches/input.jsonl $RUN/batches/batch1.jsonl`
      : `python3 -I ${NOTES}/select.py $RUN/census $RUN/batches --batches ${K}`}
4. wc -l $RUN/batches/batch*.jsonl
5. git -C /work log --oneline -1 and the version in /work/Cargo.toml ([workspace.package]): return
   them as \`code\` ("<commit> <subject>, version X.Y.Z", plus "with uncommitted changes" if
   git status shows modified files under crates/).

Return the run folder, its date, the Forge source the census names, the number of census cards, the code it read with, and the batch files with their line counts.`,
  {
    label: 'census',
    effort: 'low',
    schema: {
      type: 'object',
      properties: {
        run: { type: 'string' },
        date: { type: 'string' },
        code: { type: 'string' },
        forge: { type: 'string' },
        census_cards: { type: 'number' },
        batches: { type: 'array', items: { type: 'object', properties: { path: { type: 'string' }, cards: { type: 'number' } }, required: ['path', 'cards'] } },
        error: { type: 'string' },
      },
      required: ['batches'],
    },
  },
)
if (!prep || prep.error || !prep.run || !prep.batches.length) {
  // The census agent returns its failure rather than failing, so resuming this run replays it:
  // start a fresh run once the cause is fixed.
  return { failed: 'census', detail: prep, next: 'fix the cause, then start a fresh run (resuming replays this failure)' }
}
const OUT = prep.run
log(`${prep.census_cards} cards in the census (${prep.forge}, ${prep.code}); ${prep.batches.length} batches in ${OUT}`)

phase('Read')
const reads = await parallel(prep.batches.map((b, i) => () => agent(
  `You are an independent reader of Magic: The Gathering cards for an audit. Read the brief at ${NOTES}/brief.md carefully (including its rulings), then read the cards in ${b.path} (one JSON object per line: name, type_line, layout, mana_cost per face, oracle text per face, produced = Scryfall's list of mana kinds it can make, roles = Scryfall Tagger's roles). There are ${b.cards} cards.

For every card, write how the goldfish should read it, in the brief's JSON schema, one object per line, in input order. Work in chunks of about 50 cards: write each chunk to ${OUT}/batches/out${i + 1}-partN.jsonl (N = 1, 2, …), then concatenate the parts in order into ${OUT}/batches/out${i + 1}.jsonl, and check with python3 that every line parses as JSON and the line count equals ${b.cards}.

This is a blind check: open ONLY the brief, your input file and your own output files. Do not read source code, census files or anything else under /home/agent or /work, and do not search the web. Where a card is ambiguous under the brief, decide anyway and fill \`unsure\`.

Return how many cards you wrote and up to 5 places the brief was unclear or didn't cover a card.`,
  {
    label: `read ${i + 1}`,
    phase: 'Read',
    model: 'sonnet',
    schema: {
      type: 'object',
      properties: {
        written: { type: 'number' },
        output: { type: 'string' },
        unclear: { type: 'array', items: { type: 'string' } },
      },
      required: ['written', 'output'],
    },
  },
)))
const done = reads.filter(Boolean)
const missing = prep.batches.length - done.length
if (missing) log(`${missing} reader(s) failed; their batches aren't compared`)

phase('Triage')
const outs = done.map(r => r.output).join(' ')
const triage = await agent(
  `Triage a blind audit of the mana goldfish (/work/crates/edh-goldfish). Background: ${NOTES}/README.md and the earlier runs' findings (${NOTES}/findings*.md, newest last), and TODO.md. Blind agents read cards from oracle text and ${NOTES}/brief.md; the census at ${OUT}/census holds the goldfish's readings (cards.jsonl: per card its oracle text, Forge script, reading, fidelity).

1. For each blind output (${outs}): python3 -I ${NOTES}/compare.py ${OUT}/census <output> > <output>.cmp, then python3 -I ${NOTES}/split.py <output>.cmp.
2. For EVERY mana-side disagreement: read the card's script and reading in cards.jsonl and, where needed, crates/edh-goldfish/src/forge_profile.rs, and decide: "real" (the goldfish misreads the card against its own model), "agent" (the blind reader slipped or misread the brief: e.g. Commander always has two or more opponents), or "simplification" (an accepted simplification, listed in the brief or TODO.md's Maybe list). Group real ones into classes by cause (one Forge script shape or one reader rule).
3. Draw side: set aside disagreements that are only a format slip (the same value written another way), then sample at least 20 of the rest, most popular first (rank in the report), and classify them the same way; estimate the share that's real. Say exactly which cards the sample covers (its rank bounds) and list every draw disagreement outside it, with its rank: those are untriaged.
4. Note brief rulings that would have avoided "agent" disagreements.
5. Say where a card's class here differs from an earlier run's findings.
Only state a cause you checked in the script or the code; mark anything else as a guess. Don't change any file under /work, and don't write a report file: return everything (the workflow drafts the report from it).
Readers' notes on unclear spots in the brief: ${JSON.stringify(done.flatMap(r => r.unclear || []))}`,
  {
    label: 'triage',
    schema: {
      type: 'object',
      properties: {
        cards: { type: 'number' },
        disagreements: { type: 'number' },
        classes: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              side: { type: 'string', enum: ['mana', 'draw'] },
              cause: { type: 'string' },
              examples: { type: 'array', items: { type: 'string' } },
              best_rank: { type: 'number' },
            },
            required: ['side', 'cause', 'examples'],
          },
        },
        draw_real_share: { type: 'number' },
        per_batch: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              output: { type: 'string' }, disagreements: { type: 'number' }, left_out_only: { type: 'number' },
              mana: { type: 'number' }, draw: { type: 'number' }, format_slips: { type: 'number' },
            },
            required: ['output', 'disagreements', 'left_out_only', 'mana', 'draw'],
          },
        },
        cards: {
          type: 'array',
          description: 'every triaged card',
          items: {
            type: 'object',
            properties: {
              name: { type: 'string' }, rank: { type: 'number' }, side: { type: 'string', enum: ['mana', 'draw'] },
              verdict: { type: 'string', enum: ['real', 'agent', 'simplification', 'format slip'] },
              why: { type: 'string' },
            },
            required: ['name', 'side', 'verdict', 'why'],
          },
        },
        draw_sample: { type: 'string', description: 'which draw disagreements were triaged, by rank bounds' },
        untriaged: { type: 'array', items: { type: 'string' }, description: '"Name (rank)" for every disagreement not triaged, left_out-only ones aside' },
        conflicts_with_earlier_runs: { type: 'array', items: { type: 'string' } },
        brief_rulings: { type: 'array', items: { type: 'string' } },
        findings: { type: 'string' },
      },
      required: ['disagreements', 'per_batch', 'cards', 'classes', 'draw_sample', 'untriaged', 'findings'],
    },
  },
)
if (!triage) return { census: prep, readers: done.length, missing, failed: 'triage' }

phase('Check')
const check = await agent(
  `Check a blind audit's triage before it's written up. You didn't do the triage: be skeptical of it. Run folder: ${OUT} (census/cards.jsonl, batches/out*.jsonl and the triage's .cmp and .split reports beside them). Notes and scripts: ${NOTES}.

The triage's result:
${JSON.stringify({ per_batch: triage.per_batch, disagreements: triage.disagreements, draw_sample: triage.draw_sample, untriaged: triage.untriaged, classes: triage.classes, cards: triage.cards, conflicts_with_earlier_runs: triage.conflicts_with_earlier_runs })}

1. Recount: run python3 -I ${NOTES}/compare.py ${OUT}/census <each out*.jsonl> yourself (to your own file names, not over the triage's) and split.py, and check every per-batch count, the totals, the left_out-only count and the format slips.
2. Check the draw sample: are the sampled cards the most played of the non-slip draw disagreements, as its bounds say, and is every card outside it in \`untriaged\`? Is any card called real actually outside the sample (say so)?
3. For every card called real, check the stated cause against the card's script (census cards.jsonl) and /work/crates/edh-goldfish/src/forge_profile.rs. A cause that doesn't hold, or that's only a guess ("probably"), is a correction. Check that each class's cards share the cause.
4. Check the triage's claims about earlier runs and TODO.md against ${NOTES}/findings*.md and /work/TODO.md.
Don't change any file under /work. Return every correction, each with what the triage said and what's true.`,
  {
    label: 'check',
    phase: 'Check',
    schema: {
      type: 'object',
      properties: {
        counts_ok: { type: 'boolean' },
        recount: { type: 'string', description: 'per-batch and total counts as you found them' },
        corrections: { type: 'array', items: { type: 'string' } },
        unverified: { type: 'array', items: { type: 'string' }, description: 'claims you could not check either way' },
      },
      required: ['counts_ok', 'recount', 'corrections'],
    },
  },
)

phase('Report')
const path = `${NOTES}/findings-${prep.date}.md`
const report = await agent(
  `Draft the findings file for a blind audit of the mana goldfish, to be saved as ${path} by whoever runs this workflow (you can't write it: return it). Read ${NOTES}/findings-2026-11-23.md for the house style (plain, specific, card names with EDHREC ranks, code named by file and function) and ${NOTES}/README.md.

The run: ${JSON.stringify({ run: OUT, date: prep.date, code: prep.code, forge: prep.forge, census_cards: prep.census_cards, batches: prep.batches, readers: done.length, missing_readers: missing })}
The triage: ${JSON.stringify(triage)}
The check of the triage (apply its corrections; where it couldn't verify something, say so in the text): ${JSON.stringify(check)}

Use this outline:
1. Title "# Blind-reading audit, <date>: triage", then a short paragraph: population, batches, Forge source (and whether it matches the flake's pinned forge-cardsfolder version: grep it in /work/flake.nix), the code read, the run folder, and the previous run's totals for comparison.
2. Counts: disagreements, left_out-only, mana, draw, format slips; per-batch counts in a table; whether the check's recount matched.
3. Real misreads, mana side then draw side, grouped by cause, with cards and ranks and the code involved.
4. Accepted simplifications, and reader mistakes grouped by the brief rule that would have avoided them.
5. The draw sample's bounds, and every untriaged card with its rank.
6. Where this run's verdicts differ from earlier runs'.
7. Rulings for brief.md, saying which are settled and which need a decision.
8. What the check corrected or couldn't verify.
Don't change any file. Also return the TODO.md items the findings call for (each in TODO.md's form: bold title, files, size, why), naming any existing TODO.md item each one extends.`,
  {
    label: 'report',
    phase: 'Report',
    schema: {
      type: 'object',
      properties: {
        markdown: { type: 'string' },
        todo: { type: 'array', items: { type: 'string' } },
      },
      required: ['markdown', 'todo'],
    },
  },
)

return {
  report: report && { path, markdown: report.markdown, todo: report.todo },
  next: `Write report.markdown to ${path}, check it against the run folder, add a line for it to ${NOTES}/README.md, and file report.todo in TODO.md.`,
  census: prep,
  readers: done.length,
  missing,
  triage,
  check,
}
