# TODO

Possible next steps for the `-T` decklist filters. Phases 1–2 are done (the seven commits after
`d172f0e`). Nothing below is started.

## Worth doing

- [ ] **Load the databases faster.** `check` on a local list takes ~0.49 s, nearly all of it
  loading the two databases, which now load side by side, so the combo database alone sets the
  pace (`ComboDb::load`, ~0.4 s, 120k combos; `CardDb::load` ~0.15 s alongside). Options:
  `read_json_gz` (`edh-data/src/storage.rs`) parses through `from_reader`, and reading into a
  buffer first is usually faster; or the combos could load in the background, since only scoring
  needs them. *Small each; measure first.*

- [ ] **Change filters live in the browser.** Keep every fetched deck in the pool (today
  `Pool::add` in `scout.rs` drops decks that fail a card filter) and let a key add, edit or remove
  a `-T` filter without new requests; a toggle shows failing decks dimmed. The commander picker
  (`picker.rs`) already has typing-and-narrowing code to reuse. Fetching keeps the order it started
  with. *Value high, cost medium–large.*
- [ ] **Rank by a filter's count.** An extra `r` mode in `rank.rs`: most equipment first, decks in
  the target still first; left out of the cycle when there are no card filters. *Small.*
- [ ] **Don't let a one-card filter flatten the fetch order.** `select` (`select.rs`) orders each
  tier by `likelihood`, the *minimum* over the filters' `Prior::score`, in tenths. A single-card
  filter (`name:aetherspark=1`) scores ~0.01 on every deck (one card's Zipf weight over the whole
  artifact column), so the minimum floors to 0 everywhere and the other filters' ordering
  (`equipment=10+`, which tracks the artifact count) is thrown away. Combine the scores so a
  filter that is flat across decks doesn't hide one that isn't: rank by the product of
  per-filter pass estimates, or leave out filters whose score barely varies within the table.
  Seen with `scout -T planeswalkers=15+ -T equipment=10+ -T name:aetherspark=1`. *Small.*
- [ ] **Browser header overflow.** Long filter descriptions push the run summary off the header
  ("5 didn't m" at 200 columns); shorten or wrap it in `tui.rs` `draw_header`. *Small.*
- [ ] **Keep a commander's favorites past the deck-table checks.** A favorite only shows up in
  `scout` while `select` (`select.rs`) still finds it eligible, and some of that isn't up to the
  user: the salt ceiling is recalibrated from the deck table on every run, the duplicate check
  keeps whichever copy EDHREC lists first, and owners delete decks. Pass the favorite ids to
  `select` and exempt them from the ceiling and the duplicate check (not from the user's own
  `--max-salt`, `-t`, `-T`, …); a deck EDHREC no longer lists could come from the deck database
  by slug in `start_single` (`main.rs`). `edh-scout favorites` covers it meanwhile. *Small–medium.*

- [ ] **Pick and weight commanders by their expected count** when searching across commanders.
  A commander page's inclusion rates give each commander's expected count of a filter's cards
  (sum over listed matching cards); use it to (1) weight the fetch budget by √eligible × P(pass)
  instead of √eligible, (2) update P(pass) from fetched decks (Beta prior worth ~6 decks; this
  folds in "learn each commander's match rate"; done in `plan.rs`, starting from every
  commander's observed rate instead of a page's estimate), and (3) widen the candidates when the
  outlook is poor (done for the candidates past `--commanders`, 5 at a time; not yet with new
  sources): matching cards that separate high- from low-fit commanders (Game Changers and >30%
  staples left out) → their EDHREC card pages (`cards/<slug>.json`: top 24 + 5 new commanders
  with inclusion) → those commanders' pages → keep the good ones. Use the target brackets' pages
  (`commanders/<slug>/core.json` = B2, `upgraded.json` = B3) where possible.
  Measured for `otag:ramp=25+`: the estimate is within ±1.3 of the real mean on 24.4k stored
  decks (Captain America, Cloud, Vivi); the 29 Ramp-theme commanders range from 10.8 to 29.4
  expected, but √eligible gave the most fetches to The Ur-Dragon, Teval, Pantlaza (~15); 8 seed
  cards found 148 commanders, of the top 25 new ones 8 expect 24+ (theme list: 5 of 29), e.g.
  Kozilek the Great Distortion 30.8, Ulalek 25.1 (28k decks); Kinnan drops from 24.0 (all decks)
  to 19.8/19.0 on its B3/B2 pages. A rough model gives ~2× the matches from the same 60
  fetches. Nothing in it is ramp-specific (only `CompiledFilter`'s matching cards); checked on
  12 filters × Captain America, Cloud, Vivi (24.4k stored decks): the raw estimate runs ~18% low
  (pages list 70–88% of the matching cards decks play), and dividing by the page's overall
  coverage (listed inclusion over an average deck's nonbasic cards, 82–87%) leaves 10% error,
  −2% bias. Spread within a commander ≈ max(√mean, 0.3·mean) (real/predicted median 0.99, 22 of
  33 within ±33%); wider for price filters (2–2.6×) and off-theme cards, narrower on-theme, so
  learn it from decks seen. Breaks for cards run in multiples (inclusion counts presence:
  `t:basic` estimated 2–3, real 11–20) and leans high for bracket-skewed ones (`usd>=10` on
  Vivi: 15.6 vs 7.3; bracket pages). Fit the settings from stored decks at the start of each
  search (no requests): a random-effects split of page error against deck noise over 30
  commanders says a commander's own decks outweigh its page after ~2 (price), 3.8 (ramp), 5.9
  (equipment) up to 59 (draw) decks, and the share of variation that comes from the commander
  (27% for draw and wipes, ~78% for equipment and auras) says whether expanding can help. A ridge
  model of count from the deck-table row, anchored on `Prior`, matches or beats `Prior` from ~50
  of a commander's decks (rank correlation with all decks: ramp 0.27→0.38, draw 0.12→0.36, price
  −0.07→0.92); correct for earlier searches' fetch order with it. In simulation, learning from
  counts only beats v1's pass/fail where pages are unreliable (+4% at doubled page error, 300
  decks). Touches `edhrec.rs` (card pages, bracket pages), `filters.rs` (expected count,
  seed cards), `scout.rs` `search` (score → scan → fetch with re-weighting and expansion; needs
  the filters on the fetch thread, see below), `main.rs` `start_search`. *Medium–large.*
  Done meanwhile: a filter for one card (`name:aetherspark=1`) adds the commanders on that card's
  page (`edhrec::card_commanders`, `filters::merge_candidates`), sharing the `--commanders` slots
  with the themes. What's still missing for it: the card page gives each commander's inclusion
  (`num_decks`/`potential_decks`; Blor 17%, Cloud 10%, Tam 5%, Atraxa 3%), which could weight the
  fetch budget, and it ranks by decks with the card, so smaller commanders that play it often
  are left off (Carth: 460 of 4,156 decks, 11%, below the 594 cutoff). With
  `-T planeswalkers=15+ -T equipment=10+ -T name:aetherspark=1` the 30 slots went to 24 card-page
  commanders, most with no 15-walker decks; Bolas, Carth, Jace and Chandra now wait in reserve
  and are scanned once the search widens.

- [ ] **Draw the script reader still gets wrong.** Since 0.3.7 draw is read from Forge's scripts
  (`draw` in `crates/edh-goldfish/src/forge_profile.rs`); against the blind audit's 461 draw-tagged
  cards it agrees on 392 (the oracle reader on 358). Left: optional rummages read as engines
  (Hazoret's Monument, Artist's Talent: the discard is an `UnlessCost`/optional step); draws counted
  per opponent who doesn't draw extra (Smuggler's Share) or takes a tempting offer (Tempt with
  Bunnies, as 3); an opponent's choice (Combustible Gearhulk's `GenericChoice`, read as no draw);
  Fact or Fiction's piles (left to oracle text: 1). Lodestone Bauble's draw for "that player"; a
  split card's second half (Expansion // Explosion's X draw); Struggle for Project Purity's upkeep
  draw (a mode chosen as it enters). *Small* each; none moves the corpus.
- [ ] **A `Branch` step counts both branches' draws.** `drawn` (`forge_profile.rs`) adds the true and
  false branches, so Heaven Sent (rank 12,461) reads draw 7, which comes only when an opponent is at
  0 life, and Behold the Unspeakable (13,350) draw 6 (4 with one card or fewer in hand, else 2).
  Reading Saga chapters (0.3.9) surfaced it. Take the better branch, or skip a branch gated on
  opponents. *Small*; nothing in the corpus.
- [ ] **Saga chapters are read one at a time.** `draw` (`forge_profile.rs`) takes the first chapter
  that nets a draw and ignores the others: The Flame of Keld (20,538) reads draw 2, though chapter I
  discards the hand first (a loss). Net the chapters together. Separately, `amount()` takes
  `Count$Compare` (a fixed number under a condition) as a variable 3: Nightmares and Daydreams
  (18,892) reads 3 where a goldfish gets 1 (twenty cards in a graveyard). *Small*; found checking
  0.3.9's Saga readings by hand (`notes/goldfish-audit/findings-2026-11-23.md`).
- [ ] **Draw misreads the third blind audit found** (2026-11-25,
  `notes/goldfish-audit/findings-2026-11-25.md`), all in `draw` in
  `crates/edh-goldfish/src/forge_profile.rs`. In order of reach:
  - `BecomeMonarch` is never a draw. In a goldfish nobody takes the crown, so it's an end-step
    draw engine: every Court (Grace 1630, Garenbrig 2366, Ambition 2580, Cunning 2802), Queen
    Marchesa (2746), Regal Behemoth, Palace Jailer. *Small.*
  - A draw trigger the card sets off itself isn't traced: Call of the Ring (1432; its upkeep
    `RingTemptsYou` feeds its own draw), Fathom Mage (1715; Evolve's counters feed
    `CounterAdded`). *Small–medium.*
  - A tap-to-draw ability followed by `GainControl Defined$ Self` to an opponent reads as an
    engine; Humble Defector (1789) is a one-time draw 2. *Small.*
  - Sacrificing itself for mana, then drawing as it goes to the graveyard, isn't read: Chromatic
    Star (2841) should be draw 1 like Mishra's Bauble; Nihil Spellbomb probably too (unchecked).
    *Small.*
  - An inline `NumCards$ Remembered$Amount/Plus.1` is looked up as an svar and falls back to
    `VARIABLE_DRAW`: Brass's Tunnel-Grinder (1621) reads draw 2, net 1. *Small.*
- [ ] **Mana flags and notes the third blind audit found** (2026-11-25). No card's mana units are
  wrong; only `undercounted` and the fidelity note. In `crates/edh-goldfish/src/forge_profile.rs`:
  `gives_others_mana` (~1337) counts a Station grant with `AffectedDefined$ Self` as given to
  others (Evendo 848, Uthros Titanic Godcore 1184, The Eternity Elevator 1434), and adds the
  note for mana given to other creatures without setting `undercounted` (Esika 2506, Orochi
  Merge-Keeper); `produced()` reads a `Count$Compare` amount (fixed, under a condition) as
  growing mana (Ugin's Labyrinth 2726, Ilysian Caryatid 1741, Leafkin Druid, Tower Worker).
  *Small.*
- [ ] **Settle the audit rulings still open, and add the third audit's.** The second audit's
  settled rulings are in `notes/goldfish-audit/brief.md` (2026-11-25), with half X and Windfall's
  `{"refill": 3}`. Still open: exile-to-play on a Draw-role spell, where the brief's "not a draw"
  and "card advantage without draw → 1" conflict (the census says 1; 7 disagreements in the third
  audit); spree; wheels that depend on hand size; modal and cascade draw sizes; fetch lands that
  enter tapped (Mountain Valley); `left_out` for per-turn Treasure makers. The third audit's
  smaller rulings aren't in yet (`findings-2026-11-25.md`, "Rulings for brief.md": "up to N" is
  N, X-only rocks, station never done, Wrenn and Seven, …). *Small*, docs; before the next run.
- [ ] **Make the blind audit sync first on a fresh machine.** The `goldfish-blind-audit` workflow
  (`.claude/workflows/goldfish-blind-audit.js`) fails at the census on a machine with no
  `~/.cache/edh-scout/cards.json.gz`, as on a new cbox (2026-11-23). The census agent *returns*
  that failure rather than erroring, so resuming the run replays the cached failure and only a
  fresh run recovers. Either run `edh-scout sync` in the census step when the cache is missing, or
  check for the cache before any agent starts. *Small.*

## Maybe

- [ ] **Let one run lift a configured `max_price`, `max_salt` or `since`.** Since 0.3.8 these
  can be set in `[search]`, and an option replaces them, but nothing on the command line removes
  them: a run without the cap needs a big number (`--max-price 100000`, `--since 100000`). An
  `any`/`none` value for the three options would do (`crates/edh-scout/src/main.rs`, and
  `since_date` in `crates/edh-data/src/text.rs`). *Small*. Nobody has asked yet.

- [ ] **Put the hand-written `[default: …]` hints where clap puts its own.** In `--help` (the
  long form), clap prints a `default_value` in a paragraph of its own under the help text, while
  the hints written into doc comments (most of `scout`'s options since 0.3.8 moved their
  defaults into the config, and `--cache-dir`, `--state-dir`, `--config`) stay inline at the end of it; in `-h`
  both are inline. A blank line before a hint would drop it from `-h`, so this would take
  separate `help`/`long_help` strings per option (`crates/edh-scout/src/main.rs`). *Small*;
  cosmetic only.

- [ ] **Say how much of a deck synergy could see.** Cards missing from the commander's EDHREC
  page count as 0, so a deck on a different plan from the page reads as about 0% with nothing
  saying why. Grand Arbiter Augustin IV deck `WsFaOHEZG81j3vpcqtUclA` (equipment and auras, the
  page being mostly stax): 4 of its 64 nonland cards are on the page, all mana rocks (−0.05 to
  +0.05), so it averages −0.09% and shows as "−0.1%". Keep a listed-card count in `Synergy`
  (`analysis.rs`) and show it next to the average in the detail view (`view.rs`), e.g.
  "−0.1% (4/64 cards on EDHREC's page)"; perhaps leave a deck out of the synergy percentile
  below some share listed. *Small*; the percentile part changes rankings.

- [ ] **Keep the mana goldfish to mana development, and add a mechanic only when it moves the
  numbers.** Its scope is lands, mana sources and what they cost, colours, land drops and plain
  card counts; combat, opponents, graveyards, the stack and commanders' own abilities are out.
  What it leaves out is weighed in the deck's confidence (`crates/edh-goldfish/src/doubts.rs`:
  per early card, mana engines 1, one-shot mana and sources counted as one that grow with the
  board ½, other Tagger ramp ¼; rough from 2, poor from 5) and named under the numbers. A mechanic earns its place when it moves commander-by-T4 by
  more than 2 points or mana on T4 by more than 0.2 on some deck in `tests/data/decks` (add the
  deck that shows it). Within scope and not played yet, roughly by how much they'd move the
  numbers: Treasure makers and rituals (one-shot mana: `TokenScript$ c_a_treasure…`,
  `A:SP$ Mana`); extra land drops (`AdjustLandPlays$`: Explore, Oracle of Mul Daya, Druid
  Class) and lands from the library's top or graveyard; suspend (`K:Suspend`: Mox Tantalite, Sol
  Talisman are never cast now); counter mana (Pentad Prism, Crystalline Crawler; the Vivid lands
  and Sphere of the Suns count as lasting); untapping snow lands (Sculptor of Winter, Rime
  Tender); exile-and-play counted as draw; tutors for lands or rocks; Fabled Passage's untap with
  four lands. Simplified as read: mana only some spells may use counts as any (not counting it
  was tried and was worse, since the commander is often what it pays for); a colour chosen as it
  enters, or reflected from other lands (Exotic Orchard), is any of the deck's; mana that grows
  with the board (Urza's lands, Cabal Coffers, Priest of Titania) counts as one, or as the
  creature's own power when it's the greatest power among your creatures (Selvala); X in a land
  search is two; a Saga's later chapter comes at once; a search for snow or nonbasic lands
  (Into the North) finds any land; a rock's filter taking in more than one mana counts from the
  start (a land's waits for the lands to pay for it); mana that grows with the board and takes mana
  in (Cabal Coffers) counts as one more than it takes in; a land taking a basic type as it enters,
  or copying another (Vesuva), makes any of the deck's colours; mana of each colour among your
  permanents (Bloom Tender) counts as its own colours; a land that taps for mana or sacrifices
  itself for a land for free plays as the fetch, and one whose search costs mana (Myriad
  Landscape) as the mana land, its search left out; colours a card gives other lands (Chromatic
  Lantern, Abundant Growth) are left out, as is River of Tears' {B} after a land drop. Ramp is cast
  before the commander whatever it costs (Lord of the Forsaken at 5 ahead of Kotis at 3: 1.2 points
  of Sultai Arisen's commander-by-T4). *Small* each.

- [ ] **Reconsider how strict the goldfish's confidence is, now that its card readings are
  audited.** Since cost reducers count as mana engines (weight 1, 0.3.6), 18 of the 27 corpus
  decks read poor, 6 rough and 3 good, which may make "poor" too common to notice. Kept as is on
  2026-11-22 until the blind audit (`notes/goldfish-audit`) had fixed the misreads, which 0.3.7
  did; look again at the weights and the rough-from-2/poor-from-5 cut-offs in
  `crates/edh-goldfish/src/doubts.rs`, against the corpus (33 decks) and the audit's findings.
  *Small.*

- [ ] **Mana the goldfish doesn't play yet, found by the blind audit (2026-11-22).** Each needs the
  stopping rule: mana from a planeswalker's `+` ability (Chandra, Torch of Defiance) or a
  main-phase trigger (Hulking Raptor); counter-limited mana without {T} (Wall of Roots); digging
  until a land onto the battlefield (Clifftop Lookout); a modal spell's land mode (Archdruid's
  Charm); the land-ramp half of adventures, omens and prepare cards (Beanstalk Giant, Bloomvine
  Regent, Studious First-Year). `crates/edh-goldfish/src/forge_profile.rs`. *Small* each; none in
  the corpus yet.

- [ ] **Show why the goldfish has no Forge scripts in the browser.** `check` notes it on stderr
  (a configured directory that can't be read, or no `sync --forge` yet); the browser's Mana line
  only says the cards were read without them. The background thread (`edh-scout/src/mana.rs`,
  `Background`) could hand the reason to the status line once. *Small.*

- [ ] **Mark rough and poor goldfish numbers in the deck list too.** Only the details pane,
  `check` and the report show them; a `≈` after the power value would let the list be scanned
  for them, but the goldfish plays one deck at a time (about 0.1 s each), so the list would fill
  in as the cursor moves, or need every deck played in the background. *Small-medium.*

- [ ] **`check` finds no commander in edh-scout's own exported decklists.** The plain export writes
  the commander first with no section (what MPCFill and proxy printers take), so `check` on it
  leaves out everything that needs the commander (the goldfish's commander-by-T4, synergy, the
  colour identity) unless `--commander` is given, and says so only in a warning. `check` could take
  a first line that can lead a deck as the commander when nothing else names one, saying so
  (`parse_decklist` in `edh-data/src/deck.rs`, `run_check` in `main.rs`). *Small.*

- [ ] **Score each deck faster still.** Scoring takes ~150-180 µs a deck since combos are looked up
  under their rarest card (it was ~285-435 µs). `edhpl::bracket` lowercases and regex-matches every
  card's oracle text for every deck (~40 µs a deck); those results depend only on the card, so
  they could be worked out once per card per run. `analyze` also rebuilds the commander's
  combo-vote table for every deck (~11 µs). Touches `edhpl/src/bracket.rs` (keep the golden tests
  passing) and `analysis.rs`. *Small–medium; only if rescoring after a sync still feels slow.*
- [ ] **Parse deck pages faster.** `parse_deck_page` takes ~330 µs a page, most of it in
  `enclosing_object` (`edh-data/src/edhrec.rs`), which tries a JSON parse from every `{` up to
  20 KB before `"recs":[`. With the deck database it only runs once per fetched deck and in the
  one-time import of older pages (24,696 pages: 14 s on two threads). *Small; measure first.*

- [ ] **Count only stored decks that pass the card filters** towards a commander's share when
  fetching across commanders. Today `search` in `scout.rs` counts every stored eligible deck
  (`sel.cached.len()`), because the `-T` decklist filters live in `Pool`, not the background
  thread; a commander whose 40 stored decks mostly fail `equipment=10+` is held back as if it had
  40 matches. Needs the compiled filters (and the card database) on the fetch thread. *Small–medium.*
- [ ] **Softer mixing in the ranking.** `rank::mix` interleaves commanders strictly by weight, so a
  commander's best deck takes its turn even when it scores well below the next deck of a popular
  one (within the in-target block, so all are fits). A score-aware variant (e.g. only interleave
  among decks within some margin of the best remaining) may read better; see how it feels first.
  *Small.*

- [ ] **Tell portrait from landscape by pixels.** `PaneLayout::side_by_side` (`config.rs`) takes a
  cell as twice as tall as wide; a font far from 1:2 (or a non-square cell setting) can misjudge a
  near-square terminal. Crossterm's `terminal::window_size()` gives the pixel size where the
  terminal reports it (many report 0); use that when it's there. *Small; only if the guess is
  wrong in practice.*
- [ ] **More search keys** in `edh-data/src/query.rs`: `pow`/`tou` and mana cost (`m:`). Needs
  those fields on `Card` and a `CARDS_SCHEMA` bump (rebuilds from the raw file, no download).
  *Small; only if wanted.*

- [ ] **`-T` for `edh-scout favorites`.** Decklist filters would just work (the pool applies
  them); EDHREC's own counts (`creatures=..5`) need the deck-table row, which isn't stored, so
  they'd be an error as in `check`, or counted from the type line. Touches `run_favorites` in
  `main.rs`. *Small.*
- [ ] **The last 0.5% of EDHREC's instant and sorcery counts.** Counted the way EDHREC files cards
  (`Card::primary_type`), 99.5% of 24,696 stored decks match EDHREC's instant and sorcery counts,
  and at least 99.9% for every other type. Find the cards behind the rest (split cards, adventures,
  something EDHREC files apart?) with the deck tables in the HTTP cache, as for The Aetherspark.
  Touches `edh-data/src/cards.rs`. *Small; only matters to the count columns and fetch order.*
- [ ] **Keep the favorites somewhere deleting the deck database can't reach.** They're a table in
  `decks.sqlite` (schema 3), the one file the README says can be deleted and rebuilt; deleting it
  now loses them. A plain `favorites.txt` of ids in the state dir would survive that and is easy
  to back up, at the cost of a second file to keep in step (two runs at once). Touches
  `edh-store/src/lib.rs` and the README's table. *Small.*

## Considered, probably not

- **Play the engines decks are built around in the mana goldfish.** Swamp doublers (Crypt Ghast,
  Nirkana Revenant, Bubbling Muck), Forsaken Monument's {C}, cost reducers (Etherium Sculptor,
  Foundry Inspector, Ugin the Ineffable), affinity and improvise (Thoughtcast, Kappa Cannoneer),
  Blinkmoth Urn, Koth's Mountain mana, commanders' own mana (Urza's "tap an artifact: add {U}",
  K'rrik's life for {B}). Out of the goldfish's scope (see the item on keeping to mana
  development): they need board state and per-turn effects in `game.rs`, which is where it would
  turn into a rules engine. The Mana block weighs the ones in a deck and says when they make its
  numbers rough or poor, so they aren't taken for more than they are.

- **End-to-end tests for `scout` and `favorites`.** `run_scout`/`run_favorites` (`main.rs`) need
  the full card database and the network or its cache, so testing them means splitting them into
  testable pieces or keeping fixture databases. What they're built from is covered (pool, ranking,
  browser scenarios, plain table, report); what's left is wiring. *Medium.*

- **Counting `planeswalkers`, `artifacts`, … by type line, like `t:planeswalker`.** The bare words
  are EDHREC's counts, which the deck table answers for free before fetching; the type line counts
  more (both faces, every main type: artifact creatures, MDFC lands, adventures), which only the
  decklist can tell. On 297,811 deck-table rows, `planeswalkers=8+` leaves 305 decks to fetch
  while `t:planeswalker=8+`'s bound leaves all of them (flip-walkers sit in the creature column,
  The Aetherspark in the artifact one). The answers differ often: of 24,696 stored decks, the type
  line gives another count in 82% for artifacts, 37% for sorceries, 29% for lands, 12% for
  planeswalkers; `t:artifact=25+` matches 3,613 decks `artifacts=25+` doesn't, and `t:land=..30`
  rejects 527 of 1,187 decks `lands=..30` lets through. So both stay, with `t:` for the type line.
- Searching every stored deck regardless of commander: cross-commander searches already read the
  stored decks of each commander they scan, for free.
- EDHREC deck tags as a fetch-order hint: 82% of decks are untagged, and `Prior` already finds the
  likely decks. Within a commander they say little even for ramp: on 24.7k stored decks,
  Ramp-tagged ones average 12.5 `otag:ramp` cards against 10.9 overall (Big Mana: 9.5), while
  commanders' expected counts range from 10.8 to 29.4.
- Better fetch order for ramp: ramp spreads over every card type, so the deck table can't predict
  it (rank correlation 0.45, against 0.86–0.93 for equipment and auras).
