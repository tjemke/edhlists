# Changelog

What changed in each version, newest first. How versions are numbered: see *Versioning* in
`AGENTS.md`.

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
