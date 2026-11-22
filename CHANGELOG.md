# Changelog

What changed in each version, newest first. How versions are numbered: see *Versioning* in
`AGENTS.md`.

## 0.3.7

- The mana goldfish reads more mana cards right, found by checking how it reads the 1,500 most
  played mana and draw cards: Cabal Coffers and its kin make mana again (one more than they take in,
  once there are lands to pay for it; they made none); Vesuva, Multiversal Passage and Thran Portal
  make any of the deck's colours (they made none); Bloom Tender and Faeburrow Elder make a mana of
  each of their own colours (they were read as no mana at all); the Landscapes play as the fetch
  they are (Twisted Landscape, Shattered Landscape); Dryad Arbor can't tap the turn it's played; The
  World Tree makes {G} until there are six lands. Cards whose mana for other permanents is left out
  (Chromatic Lantern's colours for your lands) and lands whose paid search is left out (Myriad
  Landscape) are marked as approximate, as are lands using up depletion counters (Hickory Woodlot,
  Peat Bog), which couldn't be read before. On the corpus decks this moves commander by turn 4 by up
  to 1.2 points (Sultai Arisen) and how often colours held it up by up to 1.9.
- The mana goldfish reads card draw from Forge's scripts too, for the cards Scryfall's tags say
  draw: what a spell draws net of what it discards or puts back (Brainstorm 1, Mystic Confluence 3,
  a wheel about 4, Day's Undoing and Timetwister too), and whether a permanent draws every turn (an
  upkeep trigger, your own spells, your creatures entering: Dark Confidant, Kindred Discovery), once
  (Proft's Eidetic Memory, Kozilek, Butcher of Truth), or only on something a goldfish doesn't do
  (life gained, a death, an opponent's spell: The Gaffer, Morbid Opportunist, Sensei's Divining
  Top's way back to the library). On the corpus decks it moves mana on turn 4 by up to 0.12 and the
  draw it says it doesn't play by up to four cards.
- The count columns for `-T equipment` and `-T creatures` are headed `equip` and `creats`, as
  `planeswalkers` and `enchantments` already were `walkers` and `enchants`.

## 0.3.6

- The mana goldfish reads what cards do for mana from Forge's card scripts: what a mana ability
  costs and makes, when a land enters tapped, what a land search puts onto the battlefield and into
  hand, what a land fetches or returns as it enters. Across the 3,700 lands and ramp cards this
  changes about 250 readings, nearly all for the better: Thriving lands and Gates make the colour
  chosen for them, Burnished Hart costs its {3} to activate, Overgrowth adds {G}{G}, lands that
  enter tapped unless you control a basic land do, the New Capenna lands that sacrifice themselves
  for a basic (Riveteers Overlook) fetch it instead of making nothing, Selvala, Heart of the Wilds
  makes at least her own power, Castle Garenbrig's six {G} wait for the lands to pay for them, a
  search for someone else's land (Path to Exile) isn't ramp, and a storage land's or energy's mana
  isn't counted every turn (Crucible of the Spirit Dragon, Conversion Apparatus). `sync` downloads
  the scripts from Forge's latest release (~250 MB, only when Forge has released since; `sync
  --forge` for them alone); a Nix build reads nixpkgs' `forge-mtg` instead, and `[goldfish]
  forge_cards` or `EDH_SCOUT_FORGE_CARDS` can point at any. Without them, cards are read from
  oracle text as before.
- The details pane, `check` and the report say when the goldfish's numbers are doubtful, and why:
  the Mana label turns yellow (`≈ rough`) or red (`≈ poor`) when the ramp it doesn't play (mana
  engines, one-shot mana, the commander's own mana) or the mana sources it couldn't read would
  likely move the mana on turn 4 by more than 0.2. Cost reducers count among the engines, by their
  Forge scripts, though Scryfall's tags don't call them ramp (Animar, Goreclaw, Etherium
  Sculptor).
- The mana goldfish no longer counts a card drawn "whenever you attack" (Bonny Pall, Clearcutter)
  as a card every turn: a goldfish doesn't attack.

## 0.3.5

- The details pane, `check` and the exported report say how a deck's mana develops: a goldfish
  plays the deck 5,000 times on its own and reports how often the commander is out by turn 4, the
  mana on turn 4 against a land a turn, how often colours held the commander up, and the cards
  most often stuck on colours (when it's more than 0.5% of games). It names the ramp and card draw
  it doesn't play (Swamp doublers, cost reducers, Treasure), which its numbers leave out. In the
  browser it plays in the background, a deck at a time. `[goldfish] games` in the config file
  changes the number of games; 0 turns it off.

## 0.3.4

- The card database keeps each card's mana cost and the kinds of mana it makes, for the mana
  goldfish to come. It rebuilds itself once from the download on disk, with no request.

## 0.3.3

- Two runs saving the same file at once (the card or combo database during a `sync`, a cached
  page, a download) no longer leave it with both runs' bytes mixed together, or fail: each writes
  a temporary file of its own, and the last to finish replaces the file whole.
- Several runs creating or upgrading the deck database at once no longer fail with "database is
  locked" or "table deck already exists": the first does it and the others wait for it.

## 0.3.2

- Searching across commanders with a `-T` that asks for one particular card
  (`-T name:aetherspark=1`) starts from EDHREC's page for that card: the commanders it lists as
  playing the card most come first, sharing the `--commanders` slots with the theme ones.
- Searching across commanders widens when too few commanders have matching decks: the candidates
  past `--commanders` wait in reserve and are scanned 5 at a time, and each commander's share of
  the fetches follows how many of its decks pass the `-T` filters, not only its table. A commander
  counts as a miss after 3 failing decks, none passing, and the search widens at most every 5
  fetched decks: `--miss-after N` and `--widen-every N` (0: never), or `miss_after` and
  `widen_every` in the config file's `[search]`, change them.
- Colour searches take a number, as on Scryfall: `c=3` is exactly three colours, `c>=4` four or
  more, and `-T 'c>=wu c=3=5+'` asks for five three-colour cards with white and blue.
- A colour search (`c=w`, `c=3`) takes a double-faced card as all its faces' colours together:
  Plargg // Augusta (red, then white) is red-white, and Gwen Stacy // Ghost-Spider (red, then
  white, blue and red) a three-colour card. It was the front face's alone. Scryfall matches each
  face on its own instead. The card database rebuilds itself once from the download on disk.
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
