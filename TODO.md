# TODO

Possible next steps for the `-T` decklist filters. Phases 1–2 are done (the seven commits after
`d172f0e`). Nothing below is started.

## Worth doing

- [ ] **Load the databases faster.** Startup (`check` on a local list) is ~0.57 s, down from ~1.1 s
  since `ensure_databases` stopped loading each file twice; nearly all of what's left is
  `ComboDb::load` (~0.37 s, 120k combos) and `CardDb::load` (~0.13 s). Options:
  `read_json_gz` (`edh-data/src/storage.rs`) parses through `from_reader`, and reading into a
  buffer first is usually faster; the two files could load on two threads (`sync.rs`
  `ensure_databases`), ~0.13 s off; or the combos could load in the background, since only
  scoring needs them. *Small each; measure first.*

- [ ] **Change filters live in the browser.** Keep every fetched deck in the pool (today
  `Pool::add` in `scout.rs` drops decks that fail a card filter) and let a key add, edit or remove
  a `-T` filter without new requests; a toggle shows failing decks dimmed. The commander picker
  (`picker.rs`) already has typing-and-narrowing code to reuse. Fetching keeps the order it started
  with. *Value high, cost medium–large.*
- [ ] **Rank by a filter's count.** An extra `r` mode in `rank.rs`: most equipment first, decks in
  the target still first; left out of the cycle when there are no card filters. *Small.*
- [ ] **Browser header overflow.** Long filter descriptions push the run summary off the header
  ("5 didn't m" at 200 columns); shorten or wrap it in `tui.rs` `draw_header`. *Small.*

## Maybe

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

- [ ] **Learn each commander's match rate** in cross-commander searches, giving commanders whose
  decks keep passing more of the budget. Needs the background fetcher (`search` in `scout.rs`) to
  check matches itself. Measure first: the per-deck estimate (`filters::Prior`) already gave ~7×
  on Wyleth `aura=12+`, and the equipment search hit 8 of 8. *Medium.*
- [ ] **Tell portrait from landscape by pixels.** `PaneLayout::side_by_side` (`config.rs`) takes a
  cell as twice as tall as wide; a font far from 1:2 (or a non-square cell setting) can misjudge a
  near-square terminal. Crossterm's `terminal::window_size()` gives the pixel size where the
  terminal reports it (many report 0); use that when it's there. *Small; only if the guess is
  wrong in practice.*
- [ ] **More search keys** in `edh-data/src/query.rs`: `pow`/`tou` and mana cost (`m:`). Needs
  those fields on `Card` and a `CARDS_SCHEMA` bump (rebuilds from the raw file, no download).
  *Small; only if wanted.*

## Considered, probably not

- Searching every stored deck regardless of commander: cross-commander searches already read the
  stored decks of each commander they scan, for free.
- EDHREC deck tags as a fetch-order hint: 82% of decks are untagged, and `Prior` already finds the
  likely decks.
- Better fetch order for ramp: ramp spreads over every card type, so the deck table can't predict
  it (rank correlation 0.45, against 0.86–0.93 for equipment and auras).
