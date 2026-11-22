export const meta = {
  name: 'goldfish-blind-audit',
  description: 'Blind-read the ~1,500 most played mana and draw cards and triage where the goldfish reads them differently',
  whenToUse: 'After a Forge release or a change to the goldfish card readers (crates/edh-goldfish), to find misreads. About 650K tokens.',
  phases: [
    { title: 'Census', detail: 'build the census, split the population into batches' },
    { title: 'Read', detail: 'blind readers, one per batch (Sonnet)', model: 'sonnet' },
    { title: 'Triage', detail: 'compare with the census, classify every mana disagreement' },
  ],
}

// args (all optional): { out: scratch dir for this run, batches: number of readers,
// pilot: cards in a single small batch instead (to try the brief), target: CARGO_TARGET_DIR }
const A = args || {}
const OUT = A.out || '/home/agent/goldfish-audit'
const K = A.pilot ? 1 : (A.batches || 7)
const TARGET = A.target || '/home/agent/edh-target'
const NOTES = '/work/notes/goldfish-audit'

phase('Census')
const prep = await agent(
  `Prepare a blind audit run of the mana goldfish in /work. Run these, from /work, and report.

1. mkdir -p ${OUT}/batches
2. CENSUS_OUT=${OUT}/census CARGO_TARGET_DIR=${TARGET} nix develop -c cargo test --release -p edh-goldfish census -- --ignored --nocapture
   (it prints "N cards read from forge-… into …"; if it fails, return the error in \`error\`).
3. ${A.pilot
      ? `python3 -I ${NOTES}/select.py ${OUT}/census ${OUT}/batches --pilot ${A.pilot} 7 && mv ${OUT}/batches/input.jsonl ${OUT}/batches/batch1.jsonl`
      : `python3 -I ${NOTES}/select.py ${OUT}/census ${OUT}/batches --batches ${K}`}
4. wc -l ${OUT}/batches/batch*.jsonl

Return the Forge source the census names, the number of census cards, and the batch files with their line counts.`,
  {
    label: 'census',
    effort: 'low',
    schema: {
      type: 'object',
      properties: {
        forge: { type: 'string' },
        census_cards: { type: 'number' },
        batches: { type: 'array', items: { type: 'object', properties: { path: { type: 'string' }, cards: { type: 'number' } }, required: ['path', 'cards'] } },
        error: { type: 'string' },
      },
      required: ['batches'],
    },
  },
)
if (!prep || prep.error || !prep.batches.length) {
  return { failed: 'census', detail: prep }
}
log(`${prep.census_cards} cards in the census (${prep.forge}); ${prep.batches.length} batches`)

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
  `Triage a blind audit of the mana goldfish (/work/crates/edh-goldfish). Background: ${NOTES}/README.md and ${NOTES}/findings.md (the previous run). Blind agents read cards from oracle text and ${NOTES}/brief.md; the census at ${OUT}/census holds the goldfish's readings (cards.jsonl: per card its oracle text, Forge script, reading, fidelity).

1. For each blind output (${outs}): python3 -I ${NOTES}/compare.py ${OUT}/census <output> > <output>.cmp, then python3 -I ${NOTES}/split.py <output>.cmp.
2. For EVERY mana-side disagreement: read the card's script and reading in cards.jsonl and, where needed, crates/edh-goldfish/src/forge_profile.rs, and decide: "real" (the goldfish misreads the card against its own model), "agent" (the blind reader slipped or misread the brief: e.g. Commander always has two or more opponents), or "simplification" (an accepted simplification, listed in the brief or TODO.md's Maybe list). Group real ones into classes by cause (one Forge script shape or one reader rule).
3. Draw side: sample at least 20 disagreements, most popular first (rank in the report), and classify them the same way; estimate the share that's real.
4. Note brief rulings that would have avoided "agent" disagreements.
Write the result to ${OUT}/findings.md (classes with example cards and ranks, then the counts). Don't change any file under /work.
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
        brief_rulings: { type: 'array', items: { type: 'string' } },
        findings: { type: 'string' },
      },
      required: ['disagreements', 'classes', 'findings'],
    },
  },
)
return { census: prep, readers: done.length, missing, triage }
