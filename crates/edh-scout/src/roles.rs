//! Is the deck well-rounded? Card roles measured against a deckbuilding template.
//!
//! Roles come from Scryfall Tagger's community-curated oracle tags (folded into bits by
//! `edh_data::tags`); cards Tagger doesn't know yet fall back to simple oracle-text heuristics.
//! The template is the common community baseline (e.g. the Command Zone's): ~36 lands, ~10 ramp,
//! ~10 card draw, ~8 targeted removal/interaction, ~2 board wipes.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use edh_data::cards::Card;
use edh_data::tags;
use edh_data::text::strip_reminder;

/// One resolved deck entry.
#[derive(Clone, Copy)]
pub struct Entry<'a> {
    pub card: &'a Card,
    pub qty: u32,
    pub commander: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Role {
    Ramp,
    Draw,
    Removal,
    Wipe,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Ramp, Role::Draw, Role::Removal, Role::Wipe];
    pub fn label(self) -> &'static str {
        match self {
            Role::Ramp => "Ramp",
            Role::Draw => "Card draw",
            Role::Removal => "Removal",
            Role::Wipe => "Board wipes",
        }
    }
    /// Fits the detail pane's label column.
    pub fn short_label(self) -> &'static str {
        match self {
            Role::Ramp => "Ramp",
            Role::Draw => "Draw",
            Role::Removal => "Removal",
            Role::Wipe => "Wipes",
        }
    }
}

fn re(s: &str) -> Regex {
    Regex::new(s).unwrap()
}

static RAMP: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        re(
            r"search your library for [^.]*?\b(?:lands?|plains|islands?|swamps?|mountains?|forests?)\b[^.]*?onto the battlefield",
        ),
        re(r"you may play (?:an|two|three) additional lands?"),
        re(r"put (?:a|an|up to (?:one|two|three)) (?:[a-z]+ )?lands? cards? from your hand onto the battlefield"),
        re(r"create (?:a|an|one|two|three|four|x|that many|[a-z]+) (?:tapped )?treasure tokens?"),
    ]
});
static OPPONENT_DRAWS: LazyLock<Regex> = LazyLock::new(|| {
    re(r"whenever (?:an opponent|a player|each opponent|another player|target opponent) draws [a-z ]*?cards?")
});
static DRAW: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        re(r"\bdraws? (?:[a-z]+ )?(?:additional )?cards?\b"),
        re(r"\bdraws? cards equal to"),
        re(r"exile the top [^.]*?of your library\b[\s\S]{0,160}?you may (?:play|cast)"),
    ]
});
/// Matches that must not be followed by " card" (graveyard hate) or " you control" (blink).
static REMOVAL_TARGETED: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        re(
            r"(?:destroy|exile) (?:up to (?:one|two|three|x) )?(?:another )?target (?:[a-z-]+ )*?(?:creature|artifact|enchantment|planeswalker|permanent|battle)s?\b",
        ),
        re(r"gain control of (?:up to one )?target (?:[a-z-]+ )*?(?:creature|permanent|artifact|planeswalker)s?\b"),
    ]
});
static REMOVAL: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        re(r"counter target (?:[a-z-]+ )*?(?:spell|ability)"),
        re(
            r"return (?:up to (?:one|two) )?(?:another )?target (?:[a-z-]+ )*?(?:creature|permanent|artifact|enchantment|planeswalker)s? to (?:its|their) owners?'? hands?",
        ),
        re(r"target (?:[a-z-]+ )*?creature[^.]*? gets -(?:\d+|x)/-(?:\d+|x)"),
        re(
            r"deals? [^.]*?damage to (?:any (?:other )?target|(?:up to (?:one|two) )?(?:another )?target (?:[a-z-]+ )*?(?:creature|planeswalker|battle))",
        ),
        re(r"fights? (?:up to one )?(?:another )?target (?:[a-z-]+ )*?creature"),
        re(
            r"(?:target|each) (?:player|opponent) sacrifices (?:a|an|one|two|x) (?:[a-z-]+ )*?(?:creature|permanent|artifact|enchantment|planeswalker)",
        ),
    ]
});
static WIPE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        re(
            r"(?:destroy|exile) all (?:other )?(?:[a-z,-]+ )*?(?:creatures|permanents|artifacts|enchantments|planeswalkers)\b",
        ),
        re(r"destroy each (?:[a-z-]+ )*?(?:creature|permanent)"),
        re(r"return all (?:[a-z-]+ )*?(?:creatures|permanents) to (?:their|its) owners?'? hands?"),
        re(r"all (?:other )?(?:[a-z-]+ )*?creatures get -(?:\d+|x)/-(?:\d+|x)"),
        re(r"each (?:other )?creature (?:[a-z ]+ )?gets -(?:\d+|x)/-(?:\d+|x)"),
        re(r"deals? (?:\d+|x) damage to each (?:other )?(?:creature|non[a-z]+ creature)"),
        re(r"each player sacrifices (?:all|[a-z]+) (?:[a-z-]+ )*?creatures"),
    ]
});

fn targeted_hit(text: &str) -> bool {
    REMOVAL_TARGETED.iter().any(|r| {
        r.find_iter(text).any(|m| {
            let after = &text[m.end()..];
            !after.starts_with(" card") && !after.starts_with(" you control")
        })
    })
}

/// The structural roles a card plays. Lands play none.
pub fn roles(card: &Card) -> Vec<Role> {
    if card.is_land() {
        return vec![];
    }
    if card.tagged {
        let bits = [
            (tags::RAMP, Role::Ramp),
            (tags::DRAW, Role::Draw),
            (tags::REMOVAL, Role::Removal),
            (tags::WIPE, Role::Wipe),
        ];
        return bits.into_iter().filter(|(b, _)| card.roles & b != 0).map(|(_, r)| r).collect();
    }
    heuristic_roles(card)
}

/// Oracle-text fallback for cards Scryfall Tagger hasn't tagged yet.
pub fn heuristic_roles(card: &Card) -> Vec<Role> {
    let text = strip_reminder(&card.oracle_joined().to_lowercase());
    let mut out = Vec::new();
    let t = card.front_type();
    let is_spell = t.contains("Instant") || t.contains("Sorcery");
    if (card.produces_mana && !is_spell) || RAMP.iter().any(|r| r.is_match(&text)) {
        out.push(Role::Ramp);
    }
    let draw_text = OPPONENT_DRAWS.replace_all(&text, "");
    if DRAW.iter().any(|r| r.is_match(&draw_text)) {
        out.push(Role::Draw);
    }
    if targeted_hit(&text) || REMOVAL.iter().any(|r| r.is_match(&text)) {
        out.push(Role::Removal);
    }
    if WIPE.iter().any(|r| r.is_match(&text)) {
        out.push(Role::Wipe);
    }
    out
}

/// Template targets: (minimum, comfortable maximum) for lands; minimum counts for the rest.
pub const LAND_RANGE: (u32, u32) = (34, 39);
pub fn target(role: Role) -> u32 {
    match role {
        Role::Ramp => 10,
        Role::Draw => 10,
        Role::Removal => 8,
        Role::Wipe => 2,
    }
}
fn weight(role: Role) -> f64 {
    if role == Role::Wipe { 0.5 } else { 1.0 }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Structure {
    pub lands: u32,
    pub counts: BTreeMap<Role, u32>,
    pub cards: BTreeMap<Role, Vec<String>>,
    /// 0-1: how close the deck is to the template (1 = meets every target).
    pub score: f64,
}

impl Structure {
    pub fn count(&self, role: Role) -> u32 {
        self.counts.get(&role).copied().unwrap_or(0)
    }

    pub fn land_score(&self) -> f64 {
        let (lo, hi) = LAND_RANGE;
        let dist = lo.saturating_sub(self.lands) + self.lands.saturating_sub(hi);
        (1.0 - 0.2 * f64::from(dist)).max(0.0)
    }

    /// Short notes on where the deck falls short of the template.
    pub fn gaps(&self) -> Vec<String> {
        let mut out = Vec::new();
        let (lo, hi) = LAND_RANGE;
        if self.lands < lo {
            out.push(format!("only {} lands", self.lands));
        } else if self.lands > hi {
            out.push(format!("{} lands is a lot", self.lands));
        }
        for role in Role::ALL {
            let (have, want) = (self.count(role), target(role));
            if have < want {
                out.push(format!("{} {} short", want - have, role.label().to_lowercase()));
            }
        }
        out
    }
}

pub fn structure(entries: &[Entry]) -> Structure {
    let mut s = Structure::default();
    for e in entries.iter().filter(|e| !e.commander) {
        let card = e.card;
        let land_back = card.is_mdfc() && card.type_line.split(" // ").any(|t| t.contains("Land"));
        if card.is_land() || land_back {
            s.lands += e.qty;
        }
        for role in roles(card) {
            *s.counts.entry(role).or_default() += e.qty;
            s.cards.entry(role).or_default().push(card.name.clone());
        }
    }
    let mut total = s.land_score();
    let mut weights = 1.0;
    for role in Role::ALL {
        let w = weight(role);
        total += w * (f64::from(s.count(role)) / f64::from(target(role))).min(1.0);
        weights += w;
    }
    s.score = total / weights;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, type_line: &str, text: &str) -> Card {
        Card {
            name: name.into(),
            type_line: type_line.into(),
            oracle: vec![text.into()],
            layout: "normal".into(),
            ..Default::default()
        }
    }

    fn r(type_line: &str, text: &str) -> Vec<Role> {
        roles(&card("X", type_line, text))
    }

    #[test]
    fn ramp() {
        assert_eq!(
            r(
                "Sorcery",
                "Search your library for up to two basic land cards, reveal those cards, put one onto the battlefield tapped and the other into your hand, then shuffle."
            ),
            vec![Role::Ramp]
        );
        let mut rock =
            card("Arcane Signet", "Artifact", "{T}: Add one mana of any color in your commander's color identity.");
        rock.produces_mana = true;
        assert_eq!(roles(&rock), vec![Role::Ramp]);
        let mut ritual = card("Dark Ritual", "Instant", "Add {B}{B}{B}.");
        ritual.produces_mana = true;
        assert!(roles(&ritual).is_empty());
        assert_eq!(r("Land", "{T}: Add {G}."), vec![]);
    }

    #[test]
    fn draw_but_not_cycling_or_punisher_triggers() {
        assert_eq!(r("Enchantment", "Whenever you cast a spell, draw a card."), vec![Role::Draw]);
        assert!(r("Creature", "Cycling {2} ({2}, Discard this card: Draw a card.)").is_empty());
        assert!(r("Creature", "Whenever an opponent draws a card, this deals 1 damage to that player.").is_empty());
        assert_eq!(
            r("Sorcery", "Draw cards equal to the greatest power among creatures you control."),
            vec![Role::Draw]
        );
    }

    #[test]
    fn removal_vs_blink_and_graveyard_hate() {
        assert_eq!(
            r("Instant", "Exile target creature. Its controller gains life equal to its power."),
            vec![Role::Removal]
        );
        assert!(
            r(
                "Instant",
                "Exile target creature you control, then return it to the battlefield under its owner's control."
            )
            .is_empty()
        );
        assert!(r("Instant", "Exile target card from a graveyard.").is_empty());
        assert!(r("Instant", "Exile target creature card from a graveyard.").is_empty());
        assert_eq!(r("Instant", "Counter target spell."), vec![Role::Removal]);
        assert_eq!(r("Instant", "Destroy target nonland permanent."), vec![Role::Removal]);
        assert_eq!(r("Sorcery", "Each opponent sacrifices a creature."), vec![Role::Removal]);
    }

    #[test]
    fn wipes() {
        assert_eq!(r("Sorcery", "Destroy all creatures. They can't be regenerated."), vec![Role::Wipe]);
        assert_eq!(r("Sorcery", "Destroy all artifacts, creatures, and enchantments."), vec![Role::Wipe]);
        assert_eq!(r("Sorcery", "Blasphemous Act deals 13 damage to each creature."), vec![Role::Wipe]);
        assert!(r("Sorcery", "Destroy all lands.").is_empty());
    }

    #[test]
    fn tagger_roles_win_over_heuristics() {
        let mut c = card(
            "Beast Within",
            "Instant",
            "Destroy target permanent. Its controller creates a 3/3 green Beast creature token.",
        );
        c.tagged = true;
        c.roles = tags::REMOVAL;
        assert_eq!(roles(&c), vec![Role::Removal]);
        c.roles = 0;
        assert!(roles(&c).is_empty(), "a tagged card with no role tags plays no role");
    }

    #[test]
    fn template_score() {
        let s =
            Structure { lands: 36, counts: Role::ALL.iter().map(|&r| (r, target(r))).collect(), ..Default::default() };
        assert!(s.gaps().is_empty());
        assert_eq!(s.land_score(), 1.0);
    }
}
