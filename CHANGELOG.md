# Changelog

What changed in each version, newest first. How versions are numbered: see *Versioning* in
`AGENTS.md`.

## 0.3.1

- The Aetherspark counts as an artifact, as EDHREC files it, not a planeswalker: in the decklist's
  grouping and in which decks a `-T` fetches first.
- EDHREC's counts (`-T planeswalkers=10+`, `-T creatures=..5`) get a count column too, like every
  other `-T`: counted in the decklist the way EDHREC files cards, listed in the details and marked
  in the decklist. They still filter on EDHREC's own count.
- Each `-T` has a colour of its own, for its column, its line in the details and its marks in the
  decklist; a card several `-T` count gets a mark for each.

## 0.3.0

Breaking: the deck database moves to schema 3 (for favorites), and older versions refuse it from
then on. To go back, delete `decks.sqlite`: decks are read again from the page cache while their
pages last, and the favorites are lost.

- Mark decks as favorites with `*` in the browser: they come first in every scout that includes
  them, and `F` shows only them.
- `edh-scout favorites` lists every favorite, across commanders.
- The browser's key hints fit the terminal: `↑↓` dropped, two shortened, and the least needed left
  out when it's narrow.
- The browser's `#` column grows past 999 instead of cutting the number off.

## 0.2.0

- The Rust rewrite, and everything up to the `v0.2.0` tag (no changelog was kept before).
