# Blind-reading audit, 2026-11-22: findings

Population: 1,500 cards (mana cards with rank ≤ ~3,800, draw-only with rank ≤ 2,000). Pilot 50 (Opus
+ Sonnet, same cards), then 1,450 in 7 Sonnet batches (~640K tokens). Disagreements with the census
(0.3.6 readings): 250 (53 only in `left_out`), 99 mana-side, 98 draw-side.

Triage: mana side read card by card; draw side sampled (batches 1 and 7: roughly 2 in 3 real). Most
mana-side disagreements are agent slips (opponent-count lands: Commander always has two or more
opponents, so the census's "untapped" is right), accepted simplifications, or undercounted flags.

## Real, mana side (fix or mark)

1. **Cabal Coffers makes nothing** (`makes: []`, rank 202): growing mana "counted as one" then the
   `{2}` it takes in is subtracted. Nykthos keeps its unit. Expect Cabal Stronghold, Coffers-likes.
2. **Lands whose mana comes from a type they take** make nothing: Vesuva (929), Multiversal Passage
   (1982). Should be any colour (approximate), Vesuva tapped-ish.
3. **"Mana of each colour among permanents you control"** is Unreadable and falls back to oracle →
   `other`: Bloom Tender (247), Faeburrow Elder (487), Jeweled Amulet. Could be one unit of its own
   colours, undercounted, approximate.
4. **Lands that tap for {C} or sacrifice to fetch** read as {C} lands only, losing the fixing:
   Myriad Landscape, Blighted Woodland, Krosan Verge, Terrain Generator, Twisted/Shattered/
   Deceptive/Perilous/... Landscapes, Promising Vein, Shire Terrace. (Mountain Valley reads as a
   fetch.) Affects colours, not mana. The model has one kind per land face.
5. **Colour fixing for your lands isn't flagged**: Chromatic Lantern (85) reads Exact (its "lands
   tap for any colour" left out silently); Abundant Growth (Aura: enchanted land taps for any) reads
   as draw 1 only; The World Tree (820) counts as any colour from the start (should be G until six
   lands). Urborg, Yavimaya are Unreadable (from the earlier dump).
6. **Small**: Dryad Arbor enters untapped but is summoning sick (can't tap the turn it's played);
   River of Tears makes U only (B after a land drop, which a goldfish always has).

## Real, draw side (step 5: read draw from scripts)

-  Draw engines that need something a goldfish doesn't do: life gain (Well of Lost Dreams, The
  Gaffer, Exemplar of Light), discarding (Bone Miser), losing life (Vilis), a creature dying (Morbid
  Opportunist, rank 244: "die" escapes the `dies` pattern), tokens (Idol of Oblivion, 190),
  opponents (Smuggler's Share). - One-time draws read as engines: Proft's Eidetic Memory (ETB),
  Ominous Seas (cycling). - Rummages and looters as engines or full draws: Currency Converter,
  Sensei's Divining Top. - Draw lost behind one-shot ramp: Big Score (138), Unexpected Windfall
  (270), Seize the Spoils (720): read `other`, should be draw 1 (rummage + Treasure).
- Draw counts: Fact or Fiction 1 (≈2–3), Mystic Confluence 1 (3 by modes); Kindred Discovery (339)
  dropped because its trigger also says "attacks".

## Not modelled, candidates for the stopping rule (§3)

- Mana from planeswalker loyalty (Chandra, Torch of Defiance) or main-phase triggers (Hulking
  Raptor); counter-limited abilities without {T} (Wall of Roots).
- Dig-until-land onto the battlefield (Clifftop Lookout), land modes of modal spells (Archdruid's
  Charm), land-ramp halves of adventures/omens/prepare cards (Beanstalk Giant, Bloomvine Regent,
  Studious First-Year).

## What was done about it (0.3.7)

The mana classes 1–5 and Dryad Arbor were fixed or marked; draw is read from Forge's scripts, which
agrees with the blind readers on 392 of 461 draw-tagged cards (the oracle reader on 358).

## Brief changes for the next run (now in brief.md's rulings)

Opponents (three, untapped-with-two-opponents lands are untapped); single-type "untapped if you
control a Forest" is a check land; "a basic land" → untapped with basics 1; not_played takes Ramp
first even when the card draws; tagged card advantage without "draw" is 1 card; effect priority
rock > land_ramp > draw; adventure/omen/prepare: main face only; undercounted only for cards read as
lands or rocks; land-ramp activation goes in both `activation` and `extra`; no `back_face` on
pathways.

## Closing deck round, 2026-11-22

Decks (EDHREC, B3–4, `--format sectioned`; in the corpus as heliod, nekusar, tatyova, jhoira,
mizzix, gitrog): 87% of their mana and draw cards were among the 1,500 checked blind; the other
47 were read by hand. Misreads found: wheels that shuffle hand and graveyard (Day's Undoing, Time
Spiral: drew 7 for free), draws inside `RepeatEach` over every player (Winds of Change), depletion
lands (Hickory Woodlot, Peat Bog: unreadable); rarer ones logged in TODO.md. Fixing them moved the
six decks by at most 0.4 points of commander by turn 4 (Nekusar) and 0.04 mana on turn 4, within
the plan's tolerances (2 points, 0.2 mana): the audit stops here.
