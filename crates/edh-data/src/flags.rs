//! Small closed sets of rules values (colours, card types, roles, …) as bit sets.
//!
//! [`flags!`] declares an enum whose values each have a code (how Scryfall and our
//! database write them: "W", "Land", "ramp"); [`EnumSet`] holds any combination of them in one
//! integer and iterates in declaration order, so a colour set always reads in WUBRG order.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{BitAnd, BitOr, BitOrAssign};

use serde::de::{self, Deserialize, Deserializer, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeSeq, Serializer};

/// An enum that [`EnumSet`] can hold: at most 16 values, each with a code.
pub trait Flag: Copy + Eq + fmt::Debug + 'static {
    /// Every value, in declaration (and display) order.
    const ALL: &'static [Self];
    /// Position in [`Flag::ALL`].
    fn index(self) -> u8;
    /// How the value is written: in our database, and in Scryfall's data where it has one.
    fn code(self) -> &'static str;
    /// The value written `code` (exactly; case matters).
    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.code() == code)
    }
}

/// Declare a [`Flag`] enum: `flags! { pub enum Color { W = "W", U = "U" } }`. Values are
/// declared in display order; the derives are the ones every flag needs.
macro_rules! flags {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $($(#[$vmeta:meta])* $variant:ident = $code:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis enum $name { $($(#[$vmeta])* $variant),+ }

        impl $crate::flags::Flag for $name {
            const ALL: &'static [Self] = &[$($name::$variant),+];
            fn index(self) -> u8 {
                self as u8
            }
            fn code(self) -> &'static str {
                match self { $($name::$variant => $code),+ }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str($crate::flags::Flag::code(*self))
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> ::std::result::Result<S::Ok, S::Error> {
                s.serialize_str($crate::flags::Flag::code(*self))
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> ::std::result::Result<Self, D::Error> {
                let code = <::std::borrow::Cow<'de, str> as ::serde::Deserialize>::deserialize(d)?;
                <$name as $crate::flags::Flag>::from_code(&code).ok_or_else(|| {
                    ::serde::de::Error::custom(format!("unknown {} '{code}'", stringify!($name)))
                })
            }
        }
    };
}

pub(crate) use flags;

/// A set of `T`s. Iterates, prints and serialises in `T`'s declaration order.
pub struct EnumSet<T: Flag> {
    bits: u16,
    flag: PhantomData<T>,
}

impl<T: Flag> EnumSet<T> {
    pub const fn empty() -> Self {
        Self { bits: 0, flag: PhantomData }
    }
    pub fn all() -> Self {
        T::ALL.iter().copied().collect()
    }
    pub fn of(v: T) -> Self {
        Self { bits: 1 << v.index(), flag: PhantomData }
    }
    /// The values among `codes` (as [`Flag::from_code`] reads them); unknown codes are skipped.
    pub fn from_codes<'a>(codes: impl IntoIterator<Item = &'a str>) -> Self {
        codes.into_iter().filter_map(T::from_code).collect()
    }
    pub fn contains(self, v: T) -> bool {
        self.bits & (1 << v.index()) != 0
    }
    pub fn insert(&mut self, v: T) {
        self.bits |= 1 << v.index();
    }
    // By reference, so `#[serde(skip_serializing_if = "EnumSet::is_empty")]` takes it.
    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }
    pub fn len(&self) -> u32 {
        self.bits.count_ones()
    }
    /// Every value of `self` is in `other`.
    pub fn is_subset(self, other: Self) -> bool {
        self.bits & !other.bits == 0
    }
    pub fn intersects(self, other: Self) -> bool {
        self.bits & other.bits != 0
    }
    pub fn iter(self) -> impl Iterator<Item = T> {
        T::ALL.iter().copied().filter(move |v| self.contains(*v))
    }
}

// Written out rather than derived: a derive would ask the same of `T`, which `PhantomData` doesn't need.
impl<T: Flag> Clone for EnumSet<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Flag> Copy for EnumSet<T> {}
impl<T: Flag> PartialEq for EnumSet<T> {
    fn eq(&self, other: &Self) -> bool {
        self.bits == other.bits
    }
}
impl<T: Flag> Eq for EnumSet<T> {}
impl<T: Flag> std::hash::Hash for EnumSet<T> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.bits.hash(h);
    }
}
impl<T: Flag> Default for EnumSet<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T: Flag> From<T> for EnumSet<T> {
    fn from(v: T) -> Self {
        Self::of(v)
    }
}

impl<T: Flag> FromIterator<T> for EnumSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut s = Self::empty();
        iter.into_iter().for_each(|v| s.insert(v));
        s
    }
}

impl<T: Flag> BitOr for EnumSet<T> {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self { bits: self.bits | other.bits, flag: PhantomData }
    }
}
impl<T: Flag> BitOrAssign for EnumSet<T> {
    fn bitor_assign(&mut self, other: Self) {
        self.bits |= other.bits;
    }
}
impl<T: Flag> BitAnd for EnumSet<T> {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self { bits: self.bits & other.bits, flag: PhantomData }
    }
}

/// The codes run together ("WUG"), or each one quoted in a list if any is longer than a letter.
impl<T: Flag> fmt::Debug for EnumSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if T::ALL.iter().all(|v| v.code().len() == 1) {
            write!(f, "{{{self}}}")
        } else {
            f.debug_set().entries(self.iter().map(|v| v.code())).finish()
        }
    }
}

/// The codes run together, in declaration order ("WUG"); empty for the empty set.
impl<T: Flag> fmt::Display for EnumSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.iter().try_for_each(|v| f.write_str(v.code()))
    }
}

/// A list of codes, like Scryfall's (`["W", "U"]`).
impl<T: Flag> Serialize for EnumSet<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.len() as usize))?;
        for v in self.iter() {
            seq.serialize_element(v.code())?;
        }
        seq.end()
    }
}

impl<'de, T: Flag> Deserialize<'de> for EnumSet<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Codes<T>(PhantomData<T>);
        impl<'de, T: Flag> Visitor<'de> for Codes<T> {
            type Value = EnumSet<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a list of codes ({})", T::ALL.iter().map(|v| v.code()).collect::<Vec<_>>().join(", "))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut set = EnumSet::empty();
                while let Some(code) = seq.next_element::<std::borrow::Cow<'de, str>>()? {
                    set.insert(T::from_code(&code).ok_or_else(|| de::Error::custom(format!("unknown code '{code}'")))?);
                }
                Ok(set)
            }
        }
        d.deserialize_seq(Codes(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    flags! {
        enum Size { Small = "S", Medium = "M", Large = "L" }
    }

    #[test]
    fn a_set_reads_in_declaration_order_whatever_order_it_was_built_in() {
        let s: EnumSet<Size> = [Size::Large, Size::Small].into_iter().collect();
        assert_eq!(s.to_string(), "SL");
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![Size::Small, Size::Large]);
        assert_eq!(format!("{s:?}"), "{SL}");
    }

    #[test]
    fn a_set_round_trips_through_json_as_a_list_of_codes() {
        let s = EnumSet::of(Size::Medium) | Size::Large.into();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"["M","L"]"#);
        assert_eq!(serde_json::from_str::<EnumSet<Size>>(&json).unwrap(), s);
        assert!(serde_json::from_str::<EnumSet<Size>>(r#"["XL"]"#).is_err(), "an unknown code is an error");
        assert_eq!(EnumSet::<Size>::from_codes(["S", "XL"]), Size::Small.into(), "from_codes skips unknown codes");
    }

    #[test]
    fn subsets_and_overlaps() {
        let sm: EnumSet<Size> = [Size::Small, Size::Medium].into_iter().collect();
        assert!(EnumSet::of(Size::Small).is_subset(sm));
        assert!(!sm.is_subset(Size::Small.into()));
        assert!(EnumSet::<Size>::empty().is_subset(sm), "the empty set is within every set");
        assert!(sm.intersects(Size::Medium.into()) && !sm.intersects(Size::Large.into()));
        assert_eq!((sm & EnumSet::all()).len(), 2);
    }
}
