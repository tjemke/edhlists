# Changelog

What changed in each version, newest first. How versions are numbered: see *Versioning* in
`AGENTS.md`.

## 0.3.2

- Searching across commanders with a `-T` that asks for one particular card
  (`-T name:aetherspark=1`) starts from EDHREC's page for that card: the commanders it lists as
  playing the card most come first, sharing the `--commanders` slots with the theme ones.
- Colour searches take a number, as on Scryfall: `c=3` is exactly three colours, `c>=4` four or
  more, and `-T 'c>=wu c=3=5+'` asks for five three-colour cards with white and blue.
- In a `-T` like `'t:creature mv=3'` or `'c>=wu c=3'`, the last `=` is part of the search, as it
  already was when the search was only `mv=3`; it was taken for the range (exactly 3) and the
  search for `t:creature mv`, where a bare `mv` means a name containing "mv".
- A `-T` whose `=` belongs to the search (`c=w`, `mv<=2`) and so has no range says that it only
  counts, with how to filter (`-T 'c=w=1+'`): before `scout` or `check` starts and in the
  browser's status line.

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
