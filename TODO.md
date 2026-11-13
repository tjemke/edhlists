# TODO

Possible next steps for the `-T` decklist filters. Phases 1–2 are done (the seven commits after
`d172f0e`). Nothing below is started.

## Worth doing

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

- [ ] **Learn each commander's match rate** in cross-commander searches, giving commanders whose
  decks keep passing more of the budget. Needs the background fetcher (`search` in `scout.rs`) to
  check matches itself. Measure first: the per-deck estimate (`filters::Prior`) already gave ~7×
  on Wyleth `aura=12+`, and the equipment search hit 8 of 8. *Medium.*
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
