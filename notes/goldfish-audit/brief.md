# Blind reading brief: what a mana goldfish should make of each card

You are reading Magic: The Gathering cards for a **mana goldfish**: a solitaire simulation of the
first ten turns of a Commander deck, with no opponents and no combat. It plays a land a turn, casts
mana sources and land ramp first, then the commander, then card draw, then other spells. It only
cares about how fast the deck's mana develops and how many cards it sees.

For each card in your input you write down **how the goldfish should read it**, in the JSON
schema below, from the card's oracle text and your knowledge of the rules. You are an independent
check on a program that reads these cards automatically, so work only from the card itself and
this brief. **Do not open any other file** (no source code, no other data). When the card's real
behaviour and this model disagree, follow the model's rules and simplifications below and say
so in `note`.

Assume the deck is **five colours** (WUBRG), so "mana of any colour" is `"WUBRG"`.

## Output: one JSON object per card, one per line (JSONL)

```json
{"name": "...", "lands": [...], "spell": null | {"extra": 0, "effect": ...},
 "not_played": null | "Ramp" | "Card draw", "left_out": null | "engine" | "one-shot" | "other",
 "undercounted": false, "note": "", "unsure": ""}
```

### `lands`: one entry per way the card can be played as a land

- A plain land has one entry. A modal double-faced card (`layout` `ModalDfc`) has one per land
  face; a land back face of a spell front gets `"back_face": true`. A transforming card's land back
  (`Transform`) cannot be played as a land: no entry. Non-lands: `[]`.
- A land that makes mana:

```json
{"makes": ["G"], "with_lands": null, "enters": "untapped", "bounce": false, "sacrifices_lands": 0}
```

  - `makes`: the mana **one turn's tap** gives, as a list of **units**. A unit is one mana, written
    as the kinds it can be: `"G"`, `"UB"` (one mana, blue or black), `"WUBRG"` (any colour), `"C"`
    colourless. A land with several mana abilities makes the **one ability making the most mana**;
    if each makes one, **one unit of any of their kinds** (`{T}: Add {C}` and `{T}: Add {U} or {B},
    deals 1 damage to you` → `["UBC"]`). A bounce land's `{T}: Add {G}{U}` is two units
    `["G", "U"]`. Mana an ability **takes in** comes off what it makes (a filter's `{B/R}, {T}: Add
    {B}{B}` takes in 1 and makes 2: net one unit, of the kinds it adds; `{1}, {T}: Add {W}{U}` →
    one unit `"WU"`), and a filter whose ability nets nothing adds its kinds to the land's other
    unit instead. Abilities that **sacrifice** the card are ignored.
  - `with_lands`: mana that needs a number of lands first (Temple of the False God: `"makes": []`,
    `"with_lands": {"lands": 5, "makes": ["C", "C"]}`); otherwise `null`.
  - `enters`: `"untapped"`, `"tapped"`, `"untapped for life"` (shock lands: the goldfish always
    pays), `{"untapped if you control": ["Island", "Swamp"]}` (check lands),
    `{"untapped if you reveal": ["Island", "Swamp"]}` (snarls: reveal a card of that type from
    hand),
    `{"untapped with basics": 2}`, `{"untapped with other lands at least": 2}` (slow lands),
    `{"untapped with other lands at most": 2}` (fast lands).
  - `bounce`: returns a land you control to hand as it enters. `sacrifices_lands`: lands it
    sacrifices as it enters (Lotus Field: 2).
- A land sacrificed to find a land (fetch lands, Evolving Wilds):

```json
{"fetch": {"basic": true, "types": [], "tapped": true}}
```

  `basic`: only basic lands; `types`: basic land types it may find (`["Forest", "Island"]`), `[]`
  for any; `tapped`: the found land enters tapped. A fetch costing mana or life still counts.

### `spell`: how it plays when cast (null for lands, and for cards with no mana cost)

`extra`: generic mana paid on top of the printed cost: X = 1 for a mana rock with X in its cost,
one kick of a multikicker rock (Everflowing Chalice: 2), the mana an ability needs on top to
search for a land (Wayfarer's Bauble's `{2}`). Usually 0.

`effect` is one of:

- `{"rock": {"makes": [...], "from_next_turn": bool, "once": bool}}`: a permanent with a
  **repeatable mana ability** (rocks, dorks, mana Auras on your land: "whenever enchanted land is
  tapped for mana, add …" is one unit, usable at once). `makes` as for lands. `from_next_turn`: a
  creature (summoning sickness) or it enters tapped. `once`: it doesn't untap by itself (Grim
  Monolith). Abilities that sacrifice it, or remove counters from it, don't make it a rock.
- `{"land_ramp": {"basic": bool, "types": [...], "to_battlefield": n, "to_hand": n, "tapped": bool,
  "activation": n, "sacrifices_lands": n}}`: puts lands from your **library** onto the battlefield
  (and maybe into hand): Cultivate is 1 to battlefield tapped + 1 to hand, basic, any type.
  Includes creatures/artifacts whose own ability does it by sacrificing themselves (Sakura-Tribe
  Elder, Wayfarer's Bauble with `activation` 2). Swapping a land for a land (sacrifice one, get
  one) is not ramp. A search that needs an attack, combat damage or a death isn't land ramp.
- `{"draw": n}`: draws n cards **once**, net of cards discarded or put back (draw 3 then discard 1
  → 2). A draw whose size depends on the game counts 3; X counts 2. A permanent draws once only
  as it enters or by sacrificing itself (Mishra's Bauble).
- `"draw engine"`: a permanent that draws a card **every turn** from the next one: an upkeep or
  end-step draw trigger, a trigger the deck sets off by itself (casting spells, playing lands), or
  an ability usable every turn that doesn't sacrifice, discard, exile or remove counters; a
  planeswalker with a `+` or `0` loyalty ability that draws. Not a trigger that needs an opponent,
  an attack, combat damage, a death or Treasure.
- `"other"`: everything else (creatures, removal, rituals, Treasure makers, cost reducers, …).

### `not_played`, `left_out`, `undercounted`

- `not_played`: the card's `roles` (Scryfall Tagger's, given in the input) say `Ramp` but it's read
  as no land, rock or land ramp → `"Ramp"`; else they say `Draw` but its effect is `"other"` →
  `"Card draw"`; else `null`. Exceptions: a card that **reduces the cost of your spells** is
  `"Ramp"` not played even when roles don't say Ramp (if it isn't read as a land, rock or land
  ramp); a land search **for an opponent** (Path to Exile) isn't ramp for this deck: `null`.
- `left_out`, only when `not_played` is `"Ramp"`: `"engine"` (mana every turn from the
  battlefield: doubling mana, reducing costs, extra land drops, untapping lands, mana given to other
  permanents), `"one-shot"` (mana once: Treasure, rituals, a source sacrificed or out of counters),
  `"other"` (anything else Tagger calls ramp: a land search that needs an attack, free spells, mana
  for an opponent). Otherwise `null`.
- `undercounted`: its mana is counted as less than it grows to (mana that grows with the board
  counted as one; mana it gives other permanents left out).

## Simplifications the goldfish makes on purpose (apply them; name the one used in `note`)

- Mana only some spells may spend ("spend this mana only to cast creature spells") counts like any
  other.
- A colour chosen as it enters, or mana "of a type/colour another land could produce" (Exotic
  Orchard, Fellwar Stone, Reflecting Pool), is any of the deck's colours: `"WUBRG"`.
- Mana that grows with the board (Gaea's Cradle, Priest of Titania, Cabal Coffers, Urza's lands)
  counts as **one** unit, `undercounted: true`; a creature making mana equal to the greatest power
  among your creatures makes at least its own power.
- A mana ability that waits for something on the battlefield (a land of a type, an artifact)
  counts as one of its colours, always there.
- A land that enters untapped under a condition not listed in `enters` above (Turbulent lands,
  Gates, Towns, opponents' counts, "if you control a legendary creature") counts as `"tapped"`.
- A land kept by paying for it (Transguild Promenade) counts as free; a land kept by returning
  another (Karoo) counts as a bounce land; a land that doesn't untap counts as one that does.
- Charge-counter mana (Vivid lands, Sphere of the Suns) counts as lasting.
- A search for X lands counts as two; a Saga's later chapter happens at once; a search for snow or
  nonbasic lands finds any land; a search that waits for something counts as always happening.
- A rock's filter that takes in more than one mana counts from the start; a land's waits for
  enough lands to pay for it (put that in `with_lands`).
- Treasure, rituals, Eldrazi Spawn, one-shot mana: not modelled (effect `"other"`; `not_played`
  per roles).

## Rulings from the pilot (apply these)

- **Not rocks**: mana from a planeswalker's loyalty ability (Chandra's `+1: Add {R}{R}`) and mana
  from a trigger ("at the beginning of your precombat main phase, add {G}{G}"). They are ramp the
  goldfish doesn't play: effect `"other"` (or the draw it also has), `not_played` per roles,
  `left_out` `"engine"`.
- **Not draws**: exiling cards to play them ("exile the top card; you may play it this turn") and
  revealing the top card to put it somewhere by its type (Coiling Oracle) are not draws and not land
  ramp: effect `"other"`.
- **Draw triggers that need something the goldfish doesn't do** are not engines: gaining life,
  discarding, sacrificing, a creature dying, attacking, combat damage, an opponent's action,
  Treasure. Triggers on casting your spells of a kind, or your creatures or lands entering, are
  engines (the deck sets them off), even when only some cards do.
- **Land types**: "search for a Plains card" or "a Forest or Island card" finds any land with that
  type, nonbasic ones too: `"basic": false`. `"basic": true` only when it says *basic*.
- **Mana that grows with the board and also takes mana in** (Nykthos, Cabal Coffers: `{2}, {T}`:
  add X): one unit of its kinds, the mana taken in not subtracted, `undercounted: true`. On a land
  taking in more than one mana it waits for the lands to pay: `with_lands` = mana taken in + 1
  (Coffers: `"makes": []`, `"with_lands": {"lands": 3, "makes": ["B"]}`). Not when the count may be
  nothing (creatures that entered this turn).
- **Mana of each colour among your permanents** (Bloom Tender): one unit per colour in its own mana
  cost, `undercounted: true`.
- **A card with no mana cost** (suspend only, like Sol Talisman): `"spell": null`.
- **`left_out`** matters least; when nothing fits, choose `"other"`.
- **Opponents**: it's Commander, so there are always three opponents, each with about as many lands
  as you. A land that "enters tapped unless you have two or more opponents" enters `"untapped"`.
- **Single land type**: "enters tapped unless you control a Forest" is `{"untapped if you control":
  ["Forest"]}`; "unless you control a basic land" is `{"untapped with basics": 1}`.
- **`not_played` takes Ramp first**: roles say Ramp and it isn't read as a land, rock or land ramp →
  `"Ramp"`, even when it's read as a draw or draw engine.
- **Card advantage without the word "draw"** (look at N and put one into your hand, discover, reveal
  and put into hand) on a non-permanent with the Draw role: `{"draw": 1}`.
- **Draw only with the Draw role**: an effect `{"draw": n}` or `"draw engine"` only when `roles`
  include `Draw`; otherwise `"other"` (Big Score's rummage, Tagger calls it ramp only).
- **A hand thrown away for new cards** (Wheel of Fortune, Windfall) counts as three cards
  discarded: Wheel of Fortune is `{"draw": 4}`.
- **Several effects**: rock first, then land ramp, then draw.
- **Adventures, omens, prepare cards**: read the main (first) face only. A land with an adventure
  side is just the land (`spell: null`).
- **`undercounted`** only on cards read as a land or a rock.
- **Land ramp's activation** is counted in both `activation` and `extra`.
- **`back_face`** only for a land back of a spell front (not a pathway's second land face).
- **A land that taps for mana and can also sacrifice itself to search**: if the search costs no mana
  (`{T}, Sacrifice`: the Landscapes), the `fetch` entry only; if it costs mana (Myriad Landscape's
  `{2}`, Krosan Verge), the mana entry only.

## Fields you're unsure of

Fill `unsure` with the field and why, briefly; leave it `""` when you're confident. Keep `note`
short too: the simplification used, or a rules point that decided it.

## Examples

```json
{"name":"Sol Ring","lands":[],"spell":{"extra":0,"effect":{"rock":{"makes":["C","C"],"from_next_turn":false,"once":false}}},"not_played":null,"left_out":null,"undercounted":false,"note":"","unsure":""}
{"name":"Watery Grave","lands":[{"makes":["UB"],"with_lands":null,"enters":"untapped for life","bounce":false,"sacrifices_lands":0}],"spell":null,"not_played":null,"left_out":null,"undercounted":false,"note":"","unsure":""}
{"name":"Cultivate","lands":[],"spell":{"extra":0,"effect":{"land_ramp":{"basic":true,"types":[],"to_battlefield":1,"to_hand":1,"tapped":true,"activation":0,"sacrifices_lands":0}}},"not_played":null,"left_out":null,"undercounted":false,"note":"","unsure":""}
{"name":"Crypt Ghast","lands":[],"spell":{"extra":0,"effect":"other"},"not_played":"Ramp","left_out":"engine","undercounted":false,"note":"doubles Swamps' mana: not modelled","unsure":""}
{"name":"Phyrexian Arena","lands":[],"spell":{"extra":0,"effect":"draw engine"},"not_played":null,"left_out":null,"undercounted":false,"note":"","unsure":""}
```
