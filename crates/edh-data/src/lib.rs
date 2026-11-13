//! Data access for edh-scout: a polite cached HTTP client, local card (Scryfall) and combo
//! (Commander Spellbook) databases, EDHREC readers and the deck model.

pub mod cards;
pub mod combos;
pub mod config;
pub mod deck;
pub mod edhrec;
pub mod http;
pub mod query;
pub mod storage;
pub mod sync;
pub mod tags;
pub mod text;
