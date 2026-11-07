//! edh-scout: find well-rounded, fun Commander decklists on EDHREC.
//!
//! Decks are scored with EDHPowerLevel's power level and bracket (the `edhpl` crate), supplemented
//! by EDHREC's salt, a structural "is this deck well-rounded" check and how on-plan the deck is for
//! its commander. Data access lives in `edh-data`.

pub mod analysis;
pub mod export;
pub mod filters;
pub mod picker;
pub mod plain;
pub mod rank;
pub mod roles;
pub mod scout;
pub mod select;
pub mod tui;
pub mod view;
