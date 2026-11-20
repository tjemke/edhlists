//! What a type line says: supertypes, card types and basic land types.
//!
//! Subtypes other than the basic land types are an open set that grows with every release, so
//! they stay in the type line's text (`t:` searches it).

use crate::flags::{EnumSet, Flag, flags};
use crate::mana::Color;

flags! {
    /// A card type. Kindred was called Tribal until 2024; older type lines still read that way.
    pub enum CardType {
        Artifact = "Artifact",
        Battle = "Battle",
        Creature = "Creature",
        Enchantment = "Enchantment",
        Instant = "Instant",
        Kindred = "Kindred",
        Land = "Land",
        Planeswalker = "Planeswalker",
        Sorcery = "Sorcery",
    }
}

flags! {
    pub enum Supertype { Basic = "Basic", Legendary = "Legendary", Ongoing = "Ongoing", Snow = "Snow", World = "World" }
}

flags! {
    /// The five basic land types, in WUBRG order. A land with one taps for its colour, basic or
    /// not (Tropical Island is a Forest and an Island). Wastes is a basic land with none.
    pub enum BasicLandType { Plains = "Plains", Island = "Island", Swamp = "Swamp", Mountain = "Mountain", Forest = "Forest" }
}

impl BasicLandType {
    /// The colour of mana a land of this type taps for.
    pub fn color(self) -> Color {
        match self {
            BasicLandType::Plains => Color::W,
            BasicLandType::Island => Color::U,
            BasicLandType::Swamp => Color::B,
            BasicLandType::Mountain => Color::R,
            BasicLandType::Forest => Color::G,
        }
    }
}

/// The types that make a card a permanent.
pub fn permanent_types() -> EnumSet<CardType> {
    [
        CardType::Artifact,
        CardType::Battle,
        CardType::Creature,
        CardType::Enchantment,
        CardType::Land,
        CardType::Planeswalker,
    ]
    .into_iter()
    .collect()
}

/// One face's types, read from its type line ("Legendary Snow Land — Forest Island").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Types {
    pub supertypes: EnumSet<Supertype>,
    pub types: EnumSet<CardType>,
    pub land_types: EnumSet<BasicLandType>,
}

impl Types {
    /// Read one face's type line. Words it doesn't know (other subtypes, Conspiracy, Plane, …)
    /// are left out.
    pub fn parse(face: &str) -> Types {
        let (main, sub) = face.split_once('—').unwrap_or((face, ""));
        let mut t = Types::default();
        for word in main.split_whitespace() {
            match word {
                "Tribal" => t.types.insert(CardType::Kindred),
                w => {
                    if let Some(c) = CardType::from_code(w) {
                        t.types.insert(c);
                    } else if let Some(s) = Supertype::from_code(w) {
                        t.supertypes.insert(s);
                    }
                }
            }
        }
        if t.is(CardType::Land) {
            t.land_types = EnumSet::from_codes(sub.split_whitespace());
        }
        t
    }

    pub fn is(&self, t: CardType) -> bool {
        self.types.contains(t)
    }

    pub fn is_permanent(&self) -> bool {
        self.types.intersects(permanent_types())
    }

    /// The one main type EDHREC files the card under: land before creature before artifact before
    /// the rest ("Artifact Creature" is a creature, "Artifact Land" a land, The Aetherspark's
    /// "Artifact Planeswalker" an artifact). `None` for a face with none of these (a lone
    /// Kindred, a Conspiracy).
    pub fn primary(&self) -> Option<CardType> {
        use CardType::*;
        [Land, Creature, Artifact, Planeswalker, Battle, Instant, Sorcery, Enchantment]
            .into_iter()
            .find(|t| self.is(*t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_line_splits_into_supertypes_types_and_basic_land_types() {
        let t = Types::parse("Legendary Snow Land — Forest Island");
        assert_eq!(t.supertypes, [Supertype::Legendary, Supertype::Snow].into_iter().collect());
        assert_eq!(t.types, CardType::Land.into());
        assert_eq!(t.land_types.iter().map(BasicLandType::color).collect::<Vec<_>>(), [Color::U, Color::G]);
        let elf = Types::parse("Legendary Creature — Elf Druid");
        assert!(elf.is(CardType::Creature) && elf.land_types.is_empty(), "Elf and Druid aren't types we keep");
    }

    #[test]
    fn wastes_is_basic_but_has_no_basic_land_type() {
        let wastes = Types::parse("Basic Land");
        assert!(wastes.supertypes.contains(Supertype::Basic) && wastes.is(CardType::Land));
        assert!(wastes.land_types.is_empty(), "{:?}", wastes.land_types);
        assert!(!Types::parse("Basic Land — Forest").land_types.is_empty());
    }

    #[test]
    fn only_lands_have_basic_land_types() {
        assert!(Types::parse("Creature — Forest Dryad").land_types.is_empty(), "Forest only counts on a land");
        assert_eq!(Types::parse("Land Creature — Forest Dryad").land_types, BasicLandType::Forest.into());
    }

    #[test]
    fn tribal_is_read_as_kindred() {
        assert!(Types::parse("Tribal Instant — Elf").is(CardType::Kindred));
        assert_eq!(Types::parse("Kindred Instant — Elf").primary(), Some(CardType::Instant));
        assert_eq!(Types::parse("Kindred Enchantment — Elf").primary(), Some(CardType::Enchantment));
    }
}
