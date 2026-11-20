//! Colours.

use crate::flags::{EnumSet, Flag, flags};

flags! {
    /// One of Magic's five colours. Colourless isn't one.
    pub enum Color { W = "W", U = "U", B = "B", R = "R", G = "G" }
}

/// A card's colours or colour identity; reads in WUBRG order ("WUG").
pub type ColorSet = EnumSet<Color>;

impl ColorSet {
    /// The colours named by their letters, in any case and order ("rg", "WUBRG"); `None` if
    /// anything else is in there.
    pub fn from_letters(letters: &str) -> Option<Self> {
        letters.chars().map(|c| Color::from_code(&c.to_ascii_uppercase().to_string())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_read_in_wubrg_order() {
        assert_eq!(ColorSet::from_codes(["G", "W", "U"]).to_string(), "WUG");
        assert_eq!(ColorSet::all().to_string(), "WUBRG");
        assert_eq!(ColorSet::from_codes(["C"]), ColorSet::empty(), "colourless isn't a colour");
        assert_eq!(ColorSet::from_letters("gw"), Some(ColorSet::from_codes(["W", "G"])));
        assert_eq!(ColorSet::from_letters("gc"), None, "c isn't a colour letter");
    }
}
