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

## Maybe

- [ ] **Teach the mana goldfish more cards.** `crates/edh-goldfish/src/profile.rs` reads lands,
  rocks/dorks, land-ramp and draw; everything else is a spell of its cost. Left out, roughly by how
  much they'd move the numbers: Treasure makers and rituals (one-shot mana: no ramp now), cost
  reducers (Jodah, Goreclaw, Urza's Incubator), tutors for lands or rocks, Fabled Passage's untap
  with four lands (counted tapped), Exotic Orchard and Fellwar Stone (taken to make any of the
  deck's colours), and commanders cast again after dying. Each is a pattern in `profile.rs` plus an
  `Effect` the game applies (`game.rs`), with a test. Also: counter mana (Pentad Prism,
  Crystalline Crawler) isn't counted, and should be one-shot; mana only some spells may use counts
  as any mana (Lord of the Forsaken: spells from the graveyard; Gwenna, Inga and Esika, Ixalli's
  Lorekeeper, Haven of the Spirit Dragon: creatures, Dinosaurs, Dragons). Not counting it at all
  was tried and was worse: the commander is often what it pays for (Pantlaza is a Dinosaur), and
  commander-by-T4 fell 2-5 points in three of the nine decks. A unit that knows what it may pay
  for would do it right. And ramp is cast before the commander whatever it costs: Lord of the
  Forsaken (5) goes ahead of Kotis (3), 1.2 points of Sultai Arisen's commander-by-T4. Found by playing the nine sim decks 2,000,000 times against the Python prototype's
  200,000 (2026-11-20), where each difference traced to cards one model reads better. *Small*
  each.

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
