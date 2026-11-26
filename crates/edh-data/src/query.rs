//! Scryfall-style card search (`t:equipment`, `o:"create a treasure"`, `otag:ramp`, ...), evaluated
//! locally against the card database: no requests, works offline.
//!
//! Scryfall's syntax (<https://scryfall.com/docs/syntax>), as far as it can say something about a
//! card in a decklist:
//!
//! | Key | Matches |
//! |---|---|
//! | `t:` `type:` | the type line (either face) contains the text; partial words are fine |
//! | `o:` `oracle:` | the rules text without reminder text contains the text; `~` is the card's own name |
//! | `fo:` `fulloracle:` | the same, reminder text included |
//! | `name:`, a bare word or `"phrase"` | the name contains the text; `!"Exact Name"` for one card |
//! | `kw:` `keyword:` | has this keyword ability |
//! | `mv` `cmc` `manavalue` | mana value: `mv<=2`, `mv:even`, `mv:odd` (`:` means `=`) |
//! | `m:` `mana:` | the mana cost has at least these symbols: `m:2WW`, `m:{R/P}`; `m=` exactly, `m<` fewer |
//! | `devotion:` | that much devotion: `devotion:rrr`, `devotion>={u/b}{u/b}` |
//! | `produces:` | makes these kinds of mana: `produces:wu` (at least), `produces=wu` (just these), `produces:c` |
//! | `pow` `tou` `pt` `loy` `def` | power, toughness, both added up, loyalty, defense (and their long names): `pow>=8`, `pow>tou`; `*` counts as 0 |
//! | `c` `color`, `id` `identity` | colours: `c:rg` is at least red and green, `id:esper` fits an Esper deck; a number counts them: `c=3`, `c>=wu c=3` (exactly three, white and blue among them); a double-faced card has all its faces' colours |
//! | `has:indicator` | a colour indicator |
//! | `f:` `format:` `legal:`, `banned:`, `restricted:` | legality in a format: `f:pauper`, `banned:commander` |
//! | `usd` `eur` `tix` | price of the cheapest printing: `usd>10` (unpriced cards never match) |
//! | `edhrec` `edhrecrank` | EDHREC's popularity rank: `edhrec<=100` |
//! | `is:` / `not:` | see [`IS_VALUES`]: kinds of card (`spell`, `permanent`, `vanilla`, `bear`, `party`, `modal`, ...), layouts (`split`, `transform`, `mdfc`, `meld`, ...), mana (`hybrid`, `phyrexian`), who can lead (`commander`, `partner`, `companion`, ...), land cycles (`fetchland`, `shockland`, `triome`, ...) |
//! | `otag:` `function:` | a Scryfall Tagger tag, its sub-tags included ([`Query::resolve_tags`]) |
//! | `s:` `e:` `set:` `edition:` | printed in this set: `s:neo` |
//! | `st:` | printed in a set of this type: `st:commander`, `st:masters` |
//! | `b:` `block:`, `g:` `group:` | printed in this set's block (`b:wwk`) or release (`g:fin`: FIN, FIC, their promos, …) |
//! | `in:` | ever printed in this set, set type, game or rarity: `in:lea`, `in:commander`, `in:arena`, `in:common` |
//! | `r:` `rarity:` | printed at this rarity: `r:common`, `r>=rare` |
//! | `year`, `date` | printed then: `year<=1994`, `date>=2015-08-18`, `date>ori` (after Magic Origins), `date<now` |
//! | `is:reprint`, `is:unique`, `is:universesbeyond` | a reprint, only ever in one set, a Universes Beyond printing |
//! | `is:ubonly` | only ever printed as Universes Beyond (not Scryfall's: Sol Ring has UB printings, so it's `is:ub` but not this) |
//!
//! The set keys are about printings, and like Scryfall (showing one result per card) a search
//! matches a card when one of its printings matches all of it: `s:neo r:mythic` is a mythic in
//! Kamigawa: Neon Dynasty, and `year=2024 not:reprint` is first printed in 2024. So `-s:lea` is "has
//! a printing outside Alpha", true of nearly everything; "never printed in Alpha" is `-in:lea`
//! ([`Query::resolve_sets`]).
//!
//! Text values take `"quotes"` for phrases and `/slashes/` for (case-insensitive) regular
//! expressions. Terms side by side must all match; `or` between them, `-` in front to negate,
//! `( )` to group.
//!
//! Anything else is an error, never silently ignored. The other keys about printings (`a:`, `ft:`,
//! `frame:`, `cn:`, `is:foil`, `atag:`, ...) say so: a decklist names cards, not which printing of
//! them. So do Scryfall's display keys (`order:`, `unique:`, ...), which only change how its results
//! are shown.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Result, anyhow, bail};
use regex::Regex;

use crate::cards::{Card, FORMATS, Legality, search_key};
use crate::flags::{EnumSet, Flag};
use crate::mana::{ColorSet, ManaCost, ManaSet, Pip};
use crate::printed::{Game, Printed, Rarity, SetInfo, Sets, parse_date, today};
use crate::tags::{TagIndex, tag_slug};
use crate::text::{fold, front_face, strip_reminder};
use crate::types::{CardType, Layout, Supertype, Types};

const SUPPORTED: &str = "t: o: fo: name: kw: mv m: devotion: produces: pow tou pt loy def c id has: f: banned: restricted: usd eur tix edhrec is: not: otag: s: st: b: g: in: r: year date";

/// Every `is:` value, with its aliases after the first.
pub const IS_VALUES: &[&[&str]] = &[
    &["gamechanger", "game-changer"],
    &["commander"],
    &["brawler"],
    &["duelcommander"],
    &["oathbreaker"],
    &["partner"],
    &["companion"],
    &["reserved"],
    &["spell"],
    &["permanent"],
    &["historic"],
    &["party"],
    &["outlaw"],
    &["modal"],
    &["vanilla"],
    &["frenchvanilla"],
    &["bear"],
    &["hybrid"],
    &["phyrexian"],
    &["split"],
    &["flip"],
    &["transform", "tdfc"],
    &["meld"],
    &["meldpart"],
    &["meldresult"],
    &["leveler"],
    &["dfc", "doublefaced"],
    &["mdfc"],
    &["creatureland", "manland"],
    &["bikeland", "cycleland", "bicycleland"],
    &["bondland", "crowdland", "battlebondland", "bbdland"],
    &["bounceland", "karoo"],
    &["canopyland", "canland"],
    &["checkland"],
    &["dual"],
    &["fastland"],
    &["fetchland"],
    &["filterland"],
    &["gainland"],
    &["painland"],
    &["pathway"],
    &["scryland"],
    &["surveilland"],
    &["shadowland", "snarl"],
    &["shockland"],
    &["slowland"],
    &["storageland"],
    &["tangoland", "battleland"],
    &["tricycleland", "trikeland", "triome"],
    &["triland"],
    &["reprint"],
    &["unique"],
    &["universesbeyond", "ub"],
    &["ubonly", "universesbeyondonly"],
];

/// Scryfall keys about printings, which a decklist doesn't say.
const PRINTING_KEYS: &[&str] = &[
    "cn",
    "number",
    "new",
    "a",
    "artist",
    "artists",
    "ft",
    "flavor",
    "wm",
    "watermark",
    "border",
    "frame",
    "stamp",
    "game",
    "lang",
    "language",
    "prints",
    "sets",
    "paperprints",
    "papersets",
    "illustrations",
    "atag",
    "art",
    "arttag",
    "cheapest",
];

/// Scryfall `is:` values about printings.
const PRINTING_IS: &[&str] = &[
    "firstprint",
    "promo",
    "spotlight",
    "scryfallpreview",
    "digital",
    "alchemy",
    "foil",
    "nonfoil",
    "etched",
    "glossy",
    "hires",
    "full",
    "fullart",
    "booster",
    "masterpiece",
    "colorshifted",
    "old",
    "new",
    "default",
    "atypical",
    "textless",
    "funny",
    "datestamped",
    "prerelease",
    "planeswalker_deck",
    "league",
    "buyabox",
    "giftbox",
    "intro_pack",
    "gameday",
    "release",
    "fnm",
    "judge_gift",
    "arena_league",
    "player_rewards",
    "media_insert",
    "instore",
    "convention",
    "set_promo",
    "serialized",
];

/// Scryfall keys that change how its results are shown, not which.
const DISPLAY_KEYS: &[&str] = &["unique", "display", "order", "sort", "prefer", "direction", "include"];

/// A parsed search. [`Query::matches`] tests one card.
#[derive(Clone, Debug)]
pub struct Query {
    source: String,
    expr: Expr,
    /// Does it ask about one printing (`s:`, `r:`, `year`, …), so each printing is tried in turn?
    per_printing: bool,
}

#[derive(Clone, Debug)]
enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Term(Term),
}

#[derive(Clone, Debug)]
enum Text {
    /// Lowercased (folded for names).
    Contains(String),
    Regex(Regex),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Colors {
    Set(ColorSet),
    Colorless,
    Multicolor,
    /// How many colours (`c=3`, `id>=2`), 0 to 5.
    Count(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Is {
    GameChanger,
    Commander,
    Brawler,
    DuelCommander,
    Oathbreaker,
    Partner,
    Companion,
    Reserved,
    Spell,
    Permanent,
    Historic,
    Party,
    Outlaw,
    Modal,
    Vanilla,
    FrenchVanilla,
    Bear,
    Hybrid,
    Phyrexian,
    Split,
    Flip,
    Transform,
    Meld,
    MeldPart,
    MeldResult,
    Leveler,
    Dfc,
    Mdfc,
    Land(LandCycle),
}

/// Land cycles Scryfall has nicknames for ([`lands`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LandCycle {
    Creature,
    Bike,
    Bond,
    Bounce,
    Canopy,
    Check,
    Dual,
    Fast,
    Fetch,
    Filter,
    Gain,
    Pain,
    Pathway,
    Scry,
    Surveil,
    Shadow,
    Shock,
    Slow,
    Storage,
    Tango,
    Triome,
    Tri,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stat {
    Power,
    Toughness,
    /// Power and toughness added up.
    Pt,
    Loyalty,
    Defense,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StatValue {
    Num(f64),
    Stat(Stat),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Currency {
    Usd,
    Eur,
    Tix,
}

/// A mana cost as a bag of symbols: generic mana, and how many of each other symbol (hybrid
/// pairs in either order are the same symbol).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Symbols {
    generic: u32,
    pips: Vec<(Pip, u32)>,
    /// No mana cost at all (a land): less than any cost, but not {0}.
    none: bool,
}

#[derive(Clone, Debug)]
enum Term {
    Name(Text),
    ExactName(String),
    Type(Text),
    Oracle {
        text: Text,
        full: bool,
    },
    Keyword(String),
    Mv(Cmp, f64),
    /// `mv:even` (true) or `mv:odd`.
    MvEven(bool),
    Mana(Cmp, Symbols),
    /// Devotion to `colors` (each pip with any of them counts once).
    Devotion {
        cmp: Cmp,
        colors: ColorSet,
        n: u32,
    },
    Produces(Cmp, ManaSet),
    Stat(Stat, Cmp, StatValue),
    Price(Currency, Cmp, f64),
    Rank(Cmp, f64),
    Indicator,
    /// In the `format`th of [`FORMATS`].
    Legality(Legality, usize),
    Color {
        identity: bool,
        cmp: Cmp,
        colors: Colors,
    },
    Is(Is),
    /// `otag:`; `ids` is filled in by [`Query::resolve_tags`].
    Tag {
        slug: String,
        ids: Option<Arc<HashSet<String>>>,
    },
    /// `s:`, `st:`, `b:`, `g:` (the printing's set) and `in:` (any printing's); `codes` is filled
    /// in by [`Query::resolve_sets`].
    Sets {
        kind: SetKind,
        value: String,
        codes: Option<Arc<HashSet<String>>>,
    },
    /// `in:paper`, `in:arena`, …: any printing in that game.
    InGame(Game),
    /// `in:rare`: any printing at that rarity, side products left out ([`Printed::SIDE_PRODUCT`]).
    InRarity(Rarity),
    /// `r:`: the printing's rarity.
    Rarity(Cmp, Rarity),
    /// `year`: the printing's year.
    Year(Cmp, u32),
    /// `date`: the printing's release date.
    Date(Cmp, DateValue),
    /// `is:reprint`, `is:universesbeyond`: one of [`Printed`]'s flags.
    Printing(u8),
    /// `is:unique`: printed in one set only.
    Unique,
    /// `is:ubonly`: only ever printed as Universes Beyond (not Scryfall's: there, a search can only
    /// ask for some printing).
    OnlyUb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetKind {
    /// `s:neo`
    Code,
    /// `st:commander`
    Type,
    /// `b:wwk`
    Block,
    /// `g:fin`
    Group,
    /// `in:`: a set code or set type, any printing.
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DateValue {
    Day(u32),
    /// A set's release date, looked up by [`Query::resolve_sets`].
    Set(String),
}

const LANGUAGES: &[&str] = &[
    "en",
    "es",
    "fr",
    "de",
    "it",
    "pt",
    "ja",
    "ko",
    "ru",
    "zhs",
    "zht",
    "he",
    "la",
    "grc",
    "ar",
    "sa",
    "ph",
    "english",
    "spanish",
    "french",
    "german",
    "italian",
    "portuguese",
    "japanese",
    "korean",
    "russian",
];

fn unknown_set(code: &str, sets: &Sets) -> anyhow::Error {
    let near = sets.suggestions(code);
    if near.is_empty() {
        anyhow!("there's no set '{code}' (set codes are on scryfall.com/sets)")
    } else {
        anyhow!("there's no set '{code}'; did you mean {}?", near.join(", "))
    }
}

impl SetKind {
    /// The set codes `value` stands for.
    fn resolve(self, value: &str, sets: &Sets) -> Result<HashSet<String>> {
        let v = value.to_lowercase();
        let one =
            |code: &str| sets.get(code).map(|s| HashSet::from([s.code.clone()])).ok_or_else(|| unknown_set(code, sets));
        let related = |what: &str| {
            if !sets.knows_relations() {
                bail!("'{what}' needs Scryfall's set list; run `edh-scout sync --cards` once (~0.6 MB)")
            }
            one(&v)?;
            Ok(())
        };
        match self {
            SetKind::Code => one(&v),
            SetKind::Type if sets.is_type(&v) => Ok(sets.of_type(&v)),
            SetKind::Type => bail!("there's no set type '{v}' (use expansion, core, commander, masters, …)"),
            SetKind::Block => {
                related("b:")?;
                Ok(sets.block(&v))
            }
            SetKind::Group => {
                related("g:")?;
                Ok(sets.group(&v))
            }
            SetKind::Any if sets.is_type(&v) => Ok(sets.of_type(&v)),
            SetKind::Any if sets.get(&v).is_some() => one(&v),
            SetKind::Any if LANGUAGES.contains(&v.as_str()) => {
                bail!("'in:{v}' is about languages; the card database only has English printings")
            }
            SetKind::Any => Err(unknown_set(&v, sets)),
        }
    }
}

impl Query {
    pub fn parse(s: &str) -> Result<Self> {
        let toks = tokenize(s)?;
        if toks.is_empty() {
            bail!("empty search");
        }
        let mut p = Parser { toks, pos: 0 };
        let expr = p.or()?;
        if p.pos < p.toks.len() {
            bail!("unmatched ')' in '{s}'");
        }
        let mut per_printing = false;
        expr.walk(&mut |t, _| per_printing |= t.per_printing());
        Ok(Self { source: s.trim().to_string(), expr, per_printing })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Does it use `otag:` (and so need [`Query::resolve_tags`] before matching)?
    pub fn needs_tags(&self) -> bool {
        let mut found = false;
        self.expr.walk(&mut |t, _| found |= matches!(t, Term::Tag { .. }));
        found
    }

    /// Look up every `otag:` in Scryfall Tagger's tag tree. Unknown tags are an error, with
    /// suggestions.
    pub fn resolve_tags(&mut self, tags: &TagIndex) -> Result<()> {
        self.expr.resolve_tags(tags)
    }

    /// Does it name sets (and so need [`Query::resolve_sets`] before matching)?
    pub fn needs_sets(&self) -> bool {
        let mut found = false;
        self.expr.walk(&mut |t, _| found |= matches!(t, Term::Sets { .. } | Term::Date(_, DateValue::Set(_))));
        found
    }

    /// Look up every set it names in the card database's set list ([`crate::cards::CardDb::sets`]).
    /// Unknown sets are an error, with suggestions.
    pub fn resolve_sets(&mut self, sets: &[SetInfo]) -> Result<()> {
        self.expr.resolve_sets(&Sets(sets))
    }

    /// A card matches when the search matches it with one of its printings (or without any, when
    /// it doesn't ask about printings or the card has none listed).
    pub fn matches(&self, card: &Card) -> bool {
        if self.per_printing && !card.printings.is_empty() {
            card.printings.iter().any(|p| self.expr.matches(card, Some(p)))
        } else {
            self.expr.matches(card, None)
        }
    }

    /// Single words the query looks for in type lines, rules text and tags (not negated): what a
    /// deck built around it might be called (`t:equipment` -> "equipment").
    pub fn words(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.expr.walk(&mut |t, negated| {
            if negated {
                return;
            }
            let word = match t {
                Term::Type(Text::Contains(w)) | Term::Oracle { text: Text::Contains(w), .. } => Some(w.as_str()),
                Term::Tag { slug, .. } => Some(slug.as_str()),
                _ => None,
            };
            if let Some(w) = word.filter(|w| !w.is_empty() && !w.contains(' ')) {
                out.push(w.to_string());
            }
        });
        out
    }
}

impl Query {
    /// The text it looks for in type lines and rules text, not negated (and not regexes or `~`):
    /// "equipment" for `t:equipment`, "create a treasure" for `o:"create a treasure"`.
    pub fn texts(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.expr.walk(&mut |t, negated| {
            if let (false, Term::Type(Text::Contains(s)) | Term::Oracle { text: Text::Contains(s), .. }) = (negated, t)
                && !s.is_empty()
                && !s.contains('~')
            {
                out.push(s.clone());
            }
        });
        out
    }
}

impl FromStr for Query {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl Expr {
    /// Visit every term, with whether it sits under a negation.
    fn walk(&self, f: &mut impl FnMut(&Term, bool)) {
        fn go(e: &Expr, negated: bool, f: &mut impl FnMut(&Term, bool)) {
            match e {
                Expr::And(v) | Expr::Or(v) => v.iter().for_each(|e| go(e, negated, f)),
                Expr::Not(e) => go(e, !negated, f),
                Expr::Term(t) => f(t, negated),
            }
        }
        go(self, false, f)
    }

    fn resolve_tags(&mut self, tags: &TagIndex) -> Result<()> {
        match self {
            Expr::And(v) | Expr::Or(v) => v.iter_mut().try_for_each(|e| e.resolve_tags(tags)),
            Expr::Not(e) => e.resolve_tags(tags),
            Expr::Term(Term::Tag { slug, ids }) => {
                let found = tags.oracle_ids(slug).ok_or_else(|| {
                    let near = tags.suggestions(slug);
                    if near.is_empty() {
                        anyhow!("Scryfall Tagger has no tag 'otag:{slug}' (browse them at tagger.scryfall.com)")
                    } else {
                        anyhow!("Scryfall Tagger has no tag 'otag:{slug}'; did you mean {}?", near.join(", "))
                    }
                })?;
                *ids = Some(Arc::new(found));
                Ok(())
            }
            Expr::Term(_) => Ok(()),
        }
    }

    fn resolve_sets(&mut self, sets: &Sets) -> Result<()> {
        match self {
            Expr::And(v) | Expr::Or(v) => v.iter_mut().try_for_each(|e| e.resolve_sets(sets)),
            Expr::Not(e) => e.resolve_sets(sets),
            Expr::Term(Term::Sets { kind, value, codes }) => {
                *codes = Some(Arc::new(kind.resolve(value, sets)?));
                Ok(())
            }
            Expr::Term(Term::Date(cmp, DateValue::Set(code))) => {
                let set = sets.get(code).filter(|s| s.released != 0).ok_or_else(|| unknown_set(code, sets))?;
                *self = Expr::Term(Term::Date(*cmp, DateValue::Day(set.released)));
                Ok(())
            }
            Expr::Term(_) => Ok(()),
        }
    }

    fn matches(&self, card: &Card, at: Option<&Printed>) -> bool {
        match self {
            Expr::And(v) => v.iter().all(|e| e.matches(card, at)),
            Expr::Or(v) => v.iter().any(|e| e.matches(card, at)),
            Expr::Not(e) => !e.matches(card, at),
            Expr::Term(t) => t.matches(card, at),
        }
    }
}

impl Text {
    fn is_match(&self, hay: &str) -> bool {
        match self {
            Text::Contains(needle) => hay.contains(needle.as_str()),
            Text::Regex(re) => re.is_match(hay),
        }
    }

    fn mentions_self(&self) -> bool {
        match self {
            Text::Contains(s) => s.contains('~'),
            Text::Regex(re) => re.as_str().contains('~'),
        }
    }
}

impl Cmp {
    fn ord<T: PartialOrd>(self, a: T, b: T) -> bool {
        match self {
            Cmp::Eq => a == b,
            Cmp::Ne => a != b,
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
        }
    }

    fn num(self, a: f64, b: f64) -> bool {
        match self {
            Cmp::Eq => a == b,
            Cmp::Ne => a != b,
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
        }
    }

    /// Colour (or mana) sets: `<=` is "within", `>=` is "includes".
    fn set<T: Flag>(self, card: EnumSet<T>, q: EnumSet<T>) -> bool {
        let within = card.is_subset(q);
        let includes = q.is_subset(card);
        match self {
            Cmp::Eq => card == q,
            Cmp::Ne => card != q,
            Cmp::Le => within,
            Cmp::Lt => within && card != q,
            Cmp::Ge => includes,
            Cmp::Gt => includes && card != q,
        }
    }
}

/// What a card's rules text may call itself (all of which `~` stands for, as on Scryfall): its
/// name ("Krenko, Mob Boss"), short name ("Krenko"), or "this" and one of its types ("this
/// creature", "this Class") or "spell", "card", "permanent", "token".
fn self_names(card: &Card, face: usize) -> Vec<String> {
    let full = card.faces.get(face).map(String::as_str).unwrap_or(front_face(&card.name));
    let mut v = vec![card.name.to_lowercase(), full.to_lowercase()];
    if let Some((short, _)) = full.split_once(", ") {
        v.push(short.to_lowercase());
    }
    let type_line = card.type_line.split(" // ").nth(face).unwrap_or(&card.type_line).to_lowercase();
    let types = type_line.split(|c: char| !c.is_alphanumeric() && c != '-').filter(|w| !w.is_empty());
    v.extend(types.chain(["spell", "card", "permanent", "token"]).map(|w| format!("this {w}")));
    v.sort_by_key(|n| std::cmp::Reverse(n.len()));
    v.dedup();
    v
}

/// The costs `m:` tests: the front face's (nothing, for a land), and any other face's that has
/// one (a split card's halves, an adventure's spell; not a transformed back face).
fn costs(card: &Card) -> Vec<Symbols> {
    let mut faces = card.mana_cost.iter().map(Option::as_ref);
    let front = Symbols::of(faces.next().flatten());
    std::iter::once(front).chain(faces.flatten().map(|c| Symbols::of(Some(c)))).collect()
}

/// The colours a mana symbol counts towards devotion to.
fn pip_colors(p: Pip) -> ColorSet {
    let of = |m: crate::mana::Mana| m.color().map(ColorSet::of).unwrap_or_default();
    match p {
        Pip::One(m) => of(m),
        Pip::Hybrid(a, b) => of(a) | of(b),
        Pip::TwoOr(c) | Pip::Phyrexian(c) => ColorSet::of(c),
        Pip::PhyrexianHybrid(a, b) => ColorSet::of(a) | ColorSet::of(b),
        Pip::Snow | Pip::X => ColorSet::empty(),
    }
}

/// What a permanent adds to devotion to `colors`: its cost's symbols with any of them.
fn devotion(card: &Card, colors: ColorSet) -> u32 {
    if !card.types().is_permanent() {
        return 0;
    }
    let cost = card.mana_cost.first().and_then(Option::as_ref);
    cost.map_or(0, |c| c.pips.iter().filter(|p| pip_colors(**p).intersects(colors)).count() as u32)
}

/// A hybrid symbol reads the same either way round ({U/B} is {B/U}).
fn normal_pip(p: Pip) -> Pip {
    match p {
        Pip::Hybrid(a, b) if b.index() < a.index() => Pip::Hybrid(b, a),
        Pip::PhyrexianHybrid(a, b) if b.index() < a.index() => Pip::PhyrexianHybrid(b, a),
        p => p,
    }
}

impl Symbols {
    fn of(cost: Option<&ManaCost>) -> Symbols {
        let mut out = Symbols { generic: cost.map_or(0, |c| c.generic), pips: vec![], none: cost.is_none() };
        for p in cost.iter().flat_map(|c| &c.pips) {
            let p = normal_pip(*p);
            match out.pips.iter_mut().find(|(q, _)| *q == p) {
                Some((_, n)) => *n += 1,
                None => out.pips.push((p, 1)),
            }
        }
        out
    }

    fn count(&self, p: Pip) -> u32 {
        self.pips.iter().find(|(q, _)| *q == p).map_or(0, |(_, n)| *n)
    }

    /// Every symbol of `other`, at least as many times.
    fn includes(&self, other: &Symbols) -> bool {
        self.generic >= other.generic && other.pips.iter().all(|(p, n)| self.count(*p) >= *n)
    }

    fn same(&self, other: &Symbols) -> bool {
        self.none == other.none && self.includes(other) && other.includes(self)
    }
}

impl Cmp {
    /// Mana costs: one is more than another when it has all its symbols and more.
    fn symbols(self, card: &Symbols, q: &Symbols) -> bool {
        match self {
            Cmp::Eq => card.same(q),
            Cmp::Ne => !card.same(q),
            Cmp::Ge => card.includes(q),
            Cmp::Gt => card.includes(q) && !card.same(q),
            Cmp::Le => q.includes(card),
            Cmp::Lt => q.includes(card) && (card.none || !card.same(q)),
        }
    }
}

/// A power, toughness, loyalty or defense as a number: `*` and `X` count as 0 (as on Scryfall),
/// so "1+*" is 1 and "7-*" is 7; `None` for anything else ("?").
fn stat_number(s: &str) -> Option<f64> {
    let s = s.trim().replace('½', ".5").replace(['*', 'X', 'x'], "0");
    if s == "∞" {
        return Some(f64::INFINITY);
    }
    // A sign past the first character joins two numbers ("1+0"); one at the start is the number's.
    match s.char_indices().skip(1).find(|(_, c)| matches!(c, '+' | '-')) {
        Some((i, sign)) => {
            let (a, b) = (s[..i].parse::<f64>().ok()?, s[i + 1..].parse::<f64>().ok()?);
            Some(if sign == '+' { a + b } else { a - b })
        }
        None => s.parse().ok(),
    }
}

impl Stat {
    fn of(self, face: &crate::cards::FaceStats) -> Option<f64> {
        let num = |s: &Option<String>| s.as_deref().and_then(stat_number);
        match self {
            Stat::Power => num(&face.power),
            Stat::Toughness => num(&face.toughness),
            Stat::Pt => Some(num(&face.power)? + num(&face.toughness)?),
            Stat::Loyalty => num(&face.loyalty),
            Stat::Defense => num(&face.defense),
        }
    }

    fn named(key: &str) -> Option<Stat> {
        Some(match key {
            "pow" | "power" => Stat::Power,
            "tou" | "toughness" => Stat::Toughness,
            "pt" | "powtou" => Stat::Pt,
            "loy" | "loyalty" => Stat::Loyalty,
            "def" | "defense" => Stat::Defense,
            _ => return None,
        })
    }
}

/// The card's faces: types and rules text (reminder text left out, lowercase, its own name as
/// `~`).
fn faces(card: &Card) -> impl Iterator<Item = (Types, String)> + '_ {
    card.type_line.split(" // ").enumerate().map(|(i, t)| {
        let mut text = strip_reminder(&card.oracle.get(i).map(|o| o.to_lowercase()).unwrap_or_default());
        for n in self_names(card, i) {
            text = text.replace(&n, "~");
        }
        (Types::parse(t), text)
    })
}

fn subtypes(type_line: &str) -> impl Iterator<Item = &str> {
    type_line.split_once('—').map_or("", |(_, s)| s).split_whitespace()
}

impl Is {
    fn matches(self, card: &Card) -> bool {
        let front = card.types();
        // Changelings are every creature type; "Tajuru Paragon is also a Cleric, Rogue, …" adds some.
        let changeling = card.keywords.iter().any(|k| k == "Changeling")
            || faces(card).any(|(_, text)| text.contains("~ is every creature type"));
        let also: Vec<String> = faces(card)
            .filter_map(|(_, text)| Some(cached_regex(r"~ is also an? ([^.]*)").captures(&text)?[1].to_string()))
            .collect();
        let creature_types = |names: &[&str], kindred: bool| {
            let typed = card.type_line.split(" // ").any(|t| {
                (t.contains("Creature") || (kindred && (t.contains("Kindred") || t.contains("Tribal"))))
                    && (changeling || subtypes(t).any(|s| names.contains(&s)))
            });
            typed
                || (front.is(CardType::Creature)
                    && also.iter().any(|a| names.iter().any(|n| a.contains(n.to_lowercase().as_str()))))
        };
        let front_text = || faces(card).next().map(|(_, text)| text).unwrap_or_default();
        let pips = || card.mana_cost.iter().flatten().flat_map(|c| c.pips.iter().copied());
        let keyword = |k: &str| card.keywords.iter().any(|x| x.eq_ignore_ascii_case(k));
        let stats = card.stats.first();
        let stat = |s: Stat| stats.and_then(|f| s.of(f));
        match self {
            Is::GameChanger => card.game_changer,
            Is::Commander => commander(card) && card.legalities.get(format_index("commander")) != Legality::Banned,
            Is::Brawler => {
                (commander(card)
                    || (front.supertypes.contains(Supertype::Legendary) && front.is(CardType::Planeswalker)))
                    && (legal_in(card, "brawl") || legal_in(card, "standardbrawl"))
            }
            Is::DuelCommander => {
                commander(card)
                    && !subtypes(card.front_type_line()).any(|s| s == "Background")
                    && legal_in(card, "duel")
            }
            // A planeswalker, or a spell that says it can lead (Ashaya's Enduring Bond).
            Is::Oathbreaker => {
                (front.is(CardType::Planeswalker)
                    || (!front.is(CardType::Creature)
                        && card.oracle.iter().any(|o| o.contains("can be your commander"))))
                    && legal_in(card, "oathbreaker")
            }
            // A commander that can share the command zone (Backgrounds too, as on Scryfall).
            Is::Partner => {
                commander(card)
                    && (card.keywords.iter().any(|k| {
                        let k = k.to_lowercase();
                        k.starts_with("partner") || k == "friends forever" || k == "doctor's companion"
                    }) || card.oracle.iter().any(|o| o.contains("Partner—") || o.contains("Choose a Background"))
                        || subtypes(card.front_type_line()).any(|s| s == "Background")
                        || card.front_type_line().contains("Time Lord Doctor"))
            }
            Is::Companion => keyword("Companion"),
            Is::Reserved => card.reserved,
            // Cast from the front, or from another face with a cost of its own (a modal card's
            // back, an adventure); a transformed back face isn't cast.
            Is::Spell => {
                let castable = |t: &Types| !t.is(CardType::Land) && !t.types.is_empty();
                castable(&front)
                    || card.face_types().zip(&card.mana_cost).skip(1).any(|(t, cost)| cost.is_some() && castable(&t))
            }
            Is::Permanent => front.is_permanent(),
            Is::Historic => ["Legendary", "Artifact", "Saga"].iter().any(|t| card.type_line.contains(t)),
            // Creatures only; outlaws can be Kindred spells too (as on Scryfall).
            Is::Party => creature_types(&["Cleric", "Rogue", "Warrior", "Wizard"], false),
            Is::Outlaw => creature_types(&["Assassin", "Mercenary", "Pirate", "Rogue", "Warlock"], true),
            // Modes to choose from: "Choose one —", "Choose three.", Spree's "+ {1} —", Tiered, the
            // seasons' "{P} worth of modes"; not other choices (a Siege's "choose Abzan or Mardu").
            Is::Modal => {
                card.keywords.iter().any(|k| k == "Tiered")
                    || faces(card).any(|(_, text)| {
                        let choose = r"chooses? (one|two|three|four|five|x|any number|up to \w+|one or (more|both))\b";
                        let modes = cached_regex(r"(?m)^•");
                        (cached_regex(
                            r"chooses? (one|two|three|four|five|x|any number|up to \w+|one or (more|both))\b[^.\n]*—",
                        )
                        .is_match(&text)
                            || (modes.is_match(&text) && cached_regex(choose).is_match(&text)))
                            || cached_regex(r"(?m)worth of modes|^\+ \{").is_match(&text)
                    })
            }
            Is::Vanilla => front.is(CardType::Creature) && !front.is(CardType::Land) && front_text().trim().is_empty(),
            // Keyword abilities and nothing else, on every face; not ones with a number (Rampage 3),
            // as on Scryfall. Scryfall's own list differs at the edges (it leaves out most
            // protection, and old "Flying; banding" lines).
            Is::FrenchVanilla => {
                front.is(CardType::Creature)
                    && !front_text().trim().is_empty()
                    && faces(card).all(|(_, text)| text.lines().all(|l| keyword_line(l, &card.keywords)))
            }
            // Vehicles too, as on Scryfall.
            Is::Bear => card.mv == 2.0 && stat(Stat::Power) == Some(2.0) && stat(Stat::Toughness) == Some(2.0),
            Is::Hybrid => pips().any(|p| matches!(p, Pip::Hybrid(..) | Pip::TwoOr(_) | Pip::PhyrexianHybrid(..))),
            // In its cost or its rules text ({4}{U/P}: Transform …), as on Scryfall.
            Is::Phyrexian => {
                pips().any(|p| matches!(p, Pip::Phyrexian(_) | Pip::PhyrexianHybrid(..)))
                    || card.oracle.iter().any(|o| o.contains("/P}"))
            }
            Is::Split => card.layout == Layout::Split,
            Is::Flip => card.layout == Layout::Flip,
            Is::Transform => card.layout == Layout::Transform,
            Is::Meld => card.layout == Layout::Meld,
            Is::MeldPart => card.layout == Layout::Meld && !card.meld_result,
            Is::MeldResult => card.meld_result,
            Is::Leveler => card.layout == Layout::Leveler,
            // Transforming and modal cards; meld cards are `is:meld` (as on Scryfall).
            Is::Dfc => matches!(card.layout, Layout::Transform | Layout::ModalDfc),
            Is::Mdfc => card.is_mdfc(),
            Is::Land(cycle) => front.is(CardType::Land) && cycle.matches(card),
        }
    }
}

/// Is `name` legal (or restricted) in `format`, one of [`FORMATS`]?
fn legal_in(card: &Card, format: &str) -> bool {
    matches!(card.legalities.get(format_index(format)), Legality::Legal | Legality::Restricted)
}

fn format_index(format: &str) -> usize {
    FORMATS.iter().position(|f| *f == format).expect("a format Scryfall knows")
}

/// Can it lead a deck, as Scryfall's `is:commander` reads the rules: what
/// [`Card::can_be_commander`] says, legendary Vehicles and Spacecraft too, and not a meld result.
fn commander(card: &Card) -> bool {
    let front = card.types();
    let vehicle = front.supertypes.contains(Supertype::Legendary)
        && subtypes(card.front_type_line()).any(|s| s == "Vehicle" || s == "Spacecraft")
        && card.stats.first().is_some_and(|f| f.power.is_some());
    (card.can_be_commander() || vehicle) && !card.meld_result
}

/// A line of rules text (reminder text left out) that only lists keyword abilities: "flying,
/// vigilance", "equip {1}", "protection from red", "ward—pay 2 life", "prototype {1}{b}{b} — 3/3".
/// An ability word's " — " (power-up — …), an activated ability ("waterbend {3}: …") or a
/// keyword with a number ("rampage 3") isn't one.
fn keyword_line(line: &str, keywords: &[String]) -> bool {
    let keywords: Vec<String> = keywords.iter().map(|k| k.to_lowercase()).collect();
    let mut rest = line.trim().trim_end_matches('.');
    while !rest.is_empty() {
        let Some(k) = keywords.iter().filter(|k| rest.starts_with(k.as_str())).max_by_key(|k| k.len()) else {
            return false;
        };
        let after = &rest[k.len()..];
        // "ward—pay 2 life, …": the cost runs to the end of the line.
        if after.starts_with('—') {
            return true;
        }
        let end = after.find([',', ';']).unwrap_or(after.len());
        let (param, next) = (after[..end].trim(), &after[end..]);
        let cost = cached_regex(r"^(\{[^}]+\})+( — \d+/\d+)?$");
        let ok = param.is_empty() || cost.is_match(param) || param.starts_with("from ");
        if !ok {
            return false;
        }
        rest = next.trim_start_matches([',', ';']).trim_start();
    }
    true
}

/// A compiled regex, kept: land cycles are tested against every land.
fn cached_regex(pattern: &'static str) -> Regex {
    static CACHE: LazyLock<Mutex<HashMap<&'static str, Regex>>> = LazyLock::new(Default::default);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.entry(pattern).or_insert_with(|| Regex::new(pattern).expect("a land cycle regex")).clone()
}

impl LandCycle {
    /// Scryfall keeps a list of each cycle; this reads rules text instead, so reprints under new
    /// names (A.I.M. Labs is a Khans gain land) and cycles printed later count too.
    fn matches(self, card: &Card) -> bool {
        let Some((types, text)) = faces(card).next() else { return false };
        let basic_types = types.land_types.len();
        let has = |s: &str| text.contains(s);
        let re = |r: &'static str| cached_regex(r).is_match(&text);
        let tapped = has("~ enters tapped") && !has("enters tapped unless");
        // Taps for two colours, written out or by its basic land types.
        let two_colors = re(r"\{t\}: add \{[wubrg]\} or \{[wubrg]\}\.") || basic_types == 2;
        let cycling = card.keywords.iter().any(|k| k == "Cycling");
        match self {
            LandCycle::Creature => re(r"(~|it) becomes? an? [^.]*creature"),
            LandCycle::Bike => basic_types == 2 && cycling,
            LandCycle::Triome => basic_types == 3 && cycling,
            LandCycle::Bond => has("enters tapped unless you have two or more opponents"),
            LandCycle::Bounce => re(r"return (a land|an untapped \w+) you control to its owner's hand"),
            LandCycle::Canopy => {
                re(r"\{t\}, pay 1 life: add \{[wubrg]\} or \{[wubrg]\}") && has("sacrifice ~: draw a card")
            }
            LandCycle::Check => {
                two_colors
                    && re(
                        r"enters tapped unless you control an? (plains|island|swamp|mountain|forest) or an? (plains|island|swamp|mountain|forest)\.",
                    )
            }
            LandCycle::Dual => basic_types == 2 && types.supertypes.is_empty() && text.trim().is_empty(),
            LandCycle::Fast => two_colors && has("enters tapped unless you control two or fewer other lands"),
            LandCycle::Fetch => re(r"pay 1 life, sacrifice ~: search your library for an? \w+ or \w+ card"),
            LandCycle::Filter => {
                re(
                    r"\{[wubrg]/[wubrg]\}, \{t\}: add \{[wubrg]\}\{[wubrg]\}, \{[wubrg]\}\{[wubrg]\}, or \{[wubrg]\}\{[wubrg]\}",
                ) || re(r"^\{1\}, \{t\}: add \{[wubrg]\}\{[wubrg]\}\.$")
                    || re(r"\{5\}, \{t\}: add (five mana in any combination of colors|\{w\}\{u\}\{b\}\{r\}\{g\})\.")
            }
            LandCycle::Gain => tapped && two_colors && has("when ~ enters, you gain 1 life"),
            LandCycle::Pain => {
                !has("enters tapped")
                    && has("{t}: add {c}.")
                    && re(r"\{t\}: add \{[wubrg]\} or \{[wubrg]\}\. ~ deals 1 damage to you")
            }
            LandCycle::Pathway => {
                card.is_mdfc() && card.face_types().all(|t| t.is(CardType::Land)) && card.name.contains("Pathway")
            }
            LandCycle::Scry => tapped && two_colors && has("when ~ enters, scry 1"),
            LandCycle::Surveil => tapped && two_colors && has("when ~ enters, surveil 1"),
            LandCycle::Shadow => {
                re(r"you may reveal an? \w+ or \w+ card from your hand\. if you don't, ~ enters tapped")
            }
            LandCycle::Shock => basic_types == 2 && has("you may pay 2 life. if you don't, it enters tapped"),
            LandCycle::Slow => two_colors && has("enters tapped unless you control two or more other lands"),
            LandCycle::Storage => has("storage counter"),
            LandCycle::Tango => has("enters tapped unless you control two or more basic lands"),
            LandCycle::Tri => {
                basic_types == 0
                    && cached_regex(r"^~ enters tapped\.\n\{t\}: add \{[wubrg]\}, \{[wubrg]\}, or \{[wubrg]\}\.$")
                        .is_match(text.trim())
            }
        }
    }
}

impl Term {
    /// Is it about the printing being tried, rather than the card or all its printings?
    fn per_printing(&self) -> bool {
        matches!(
            self,
            Term::Sets { kind: SetKind::Code | SetKind::Type | SetKind::Block | SetKind::Group, .. }
                | Term::Rarity(..)
                | Term::Year(..)
                | Term::Date(..)
                | Term::Printing(_)
        )
    }

    fn matches(&self, card: &Card, at: Option<&Printed>) -> bool {
        match self {
            Term::Sets { kind: SetKind::Any, codes, .. } => {
                codes.as_ref().is_some_and(|codes| card.printings.iter().any(|p| codes.contains(&p.set)))
            }
            Term::Sets { codes, .. } => codes.as_ref().zip(at).is_some_and(|(codes, p)| codes.contains(&p.set)),
            Term::InGame(g) => card.printings.iter().any(|p| p.in_game(*g)),
            Term::InRarity(r) => card.printings.iter().any(|p| p.rarity == *r && !p.has(Printed::SIDE_PRODUCT)),
            Term::Rarity(cmp, r) => at.is_some_and(|p| cmp.ord(p.rarity, *r)),
            Term::Year(cmp, y) => at.is_some_and(|p| p.released != 0 && cmp.ord(p.released / 10000, *y)),
            Term::Date(cmp, DateValue::Day(d)) => at.is_some_and(|p| p.released != 0 && cmp.ord(p.released, *d)),
            Term::Date(_, DateValue::Set(_)) => false,
            Term::Printing(flag) => at.is_some_and(|p| p.has(*flag)),
            Term::OnlyUb => !card.printings.is_empty() && card.printings.iter().all(|p| p.has(Printed::UB)),
            Term::Unique => {
                let mut sets = card.printings.iter().map(|p| p.set.as_str());
                sets.next().is_some_and(|first| sets.all(|s| s == first))
            }
            Term::Name(Text::Contains(needle)) => search_key(&card.name).contains(needle.as_str()),
            Term::Name(text) => text.is_match(&card.name),
            Term::ExactName(name) => {
                fold(&card.name) == *name
                    || card.faces.iter().any(|f| fold(f) == *name)
                    || card.flavor_name.as_deref().is_some_and(|f| fold(f) == *name)
            }
            Term::Type(text) => text.is_match(&card.type_line.to_lowercase()),
            Term::Oracle { text, full } => card.oracle.iter().enumerate().any(|(i, o)| {
                let mut hay = if *full { o.to_lowercase() } else { strip_reminder(&o.to_lowercase()) };
                if text.mentions_self() {
                    for n in self_names(card, i) {
                        hay = hay.replace(&n, "~");
                    }
                }
                text.is_match(&hay)
            }),
            Term::Keyword(kw) => card.keywords.iter().any(|k| k.to_lowercase() == *kw),
            Term::Mv(cmp, x) => cmp.num(card.mv, *x),
            Term::MvEven(even) => card.mv.fract() == 0.0 && ((card.mv as i64) % 2 == 0) == *even,
            Term::Mana(cmp, q) => costs(card).iter().any(|c| cmp.symbols(c, q)),
            Term::Devotion { cmp, colors, n } => cmp.num(f64::from(devotion(card, *colors)), f64::from(*n)),
            Term::Produces(cmp, q) => cmp.set(card.produced, *q),
            Term::Stat(stat, cmp, rhs) => card.stats.iter().any(|face| {
                let rhs = match rhs {
                    StatValue::Num(x) => Some(*x),
                    StatValue::Stat(s) => s.of(face),
                };
                stat.of(face).zip(rhs).is_some_and(|(a, b)| cmp.num(a, b))
            }),
            // Cards without a price never match a price comparison (as on Scryfall).
            Term::Price(currency, cmp, x) => {
                let price = match currency {
                    Currency::Usd => card.usd,
                    Currency::Eur => card.eur,
                    Currency::Tix => card.tix,
                };
                price > 0.0 && cmp.num(price, *x)
            }
            Term::Rank(cmp, x) => card.rank.is_some_and(|r| cmp.num(f64::from(r), *x)),
            Term::Indicator => card.indicator,
            Term::Legality(Legality::Legal, i) => {
                matches!(card.legalities.get(*i), Legality::Legal | Legality::Restricted)
            }
            Term::Legality(l, i) => card.legalities.get(*i) == *l,
            Term::Color { identity, cmp, colors } => {
                let test = |have: ColorSet| match colors {
                    Colors::Set(q) => cmp.set(have, *q),
                    Colors::Colorless => cmp.set(have, ColorSet::empty()),
                    Colors::Multicolor => (have.len() >= 2) == (*cmp != Cmp::Ne),
                    Colors::Count(n) => cmp.num(f64::from(have.len()), f64::from(*n)),
                };
                if *identity {
                    test(card.identity)
                } else if card.face_colors.is_empty() {
                    test(card.colors.unwrap_or_default())
                } else {
                    // A double-faced card is all its faces' colours together (Scryfall matches
                    // each face on its own instead).
                    test(card.face_colors.iter().fold(ColorSet::empty(), |m, f| m | *f))
                }
            }
            Term::Is(is) => is.matches(card),
            Term::Tag { ids, .. } => ids.as_ref().is_some_and(|ids| ids.contains(&card.oracle_id)),
        }
    }
}

// --- tokens ----------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Colon,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Plain(String),
    Quoted(String),
    Regex(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Open,
    Close,
    Or,
    And,
    Neg,
    Term(String, Op, Val),
    Word(Val),
    Exact(String),
}

fn is_stop(c: char) -> bool {
    c.is_whitespace() || c == '(' || c == ')'
}

fn read_op(chars: &[char], i: usize) -> Option<(Op, usize)> {
    let two: String = chars.iter().skip(i).take(2).collect();
    Some(match two.as_str() {
        "!=" => (Op::Ne, 2),
        "<=" => (Op::Le, 2),
        ">=" => (Op::Ge, 2),
        _ => match chars.get(i)? {
            ':' => (Op::Colon, 1),
            '=' => (Op::Eq, 1),
            '<' => (Op::Lt, 1),
            '>' => (Op::Gt, 1),
            _ => return None,
        },
    })
}

/// A quoted phrase starting at `chars[i] == '"'`; returns it and the index past the closing quote.
fn read_quoted(chars: &[char], i: usize) -> Result<(String, usize)> {
    let end = (i + 1..chars.len()).find(|&k| chars[k] == '"').ok_or_else(|| anyhow!("unclosed \" quote"))?;
    Ok((chars[i + 1..end].iter().collect(), end + 1))
}

/// A `/regex/` starting at `chars[i] == '/'`; `\/` stands for a slash.
fn read_regex(chars: &[char], i: usize) -> Result<(String, usize)> {
    let mut out = String::new();
    let mut k = i + 1;
    while k < chars.len() {
        match chars[k] {
            '\\' if chars.get(k + 1) == Some(&'/') => {
                out.push('/');
                k += 2;
            }
            '\\' => {
                out.push('\\');
                if let Some(&c) = chars.get(k + 1) {
                    out.push(c);
                }
                k += 2;
            }
            '/' => return Ok((out, k + 1)),
            c => {
                out.push(c);
                k += 1;
            }
        }
    }
    bail!("unclosed /regex/")
}

fn read_value(chars: &[char], i: usize, key: &str) -> Result<(Val, usize)> {
    match chars.get(i) {
        None => bail!("'{key}' needs a value"),
        Some(&c) if is_stop(c) => bail!("'{key}' needs a value"),
        Some('"') => read_quoted(chars, i).map(|(v, j)| (Val::Quoted(v), j)),
        Some('/') => read_regex(chars, i).map(|(v, j)| (Val::Regex(v), j)),
        Some(_) => {
            let j = (i..chars.len()).find(|&k| is_stop(chars[k])).unwrap_or(chars.len());
            Ok((Val::Plain(chars[i..j].iter().collect()), j))
        }
    }
}

fn tokenize(s: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            _ if c.is_whitespace() => i += 1,
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            '-' => {
                if chars.get(i + 1).is_none_or(|n| n.is_whitespace()) {
                    bail!("a lone '-' negates nothing (write -t:creature, with no space)");
                }
                out.push(Tok::Neg);
                i += 1;
            }
            '!' => {
                let (v, j) = read_value(&chars, i + 1, "!")?;
                let name = match v {
                    Val::Plain(s) | Val::Quoted(s) => s,
                    Val::Regex(_) => bail!("'!' takes an exact name, not a /regex/"),
                };
                out.push(Tok::Exact(name));
                i = j;
            }
            '"' => {
                let (v, j) = read_quoted(&chars, i)?;
                out.push(Tok::Word(Val::Quoted(v)));
                i = j;
            }
            _ => {
                let key_end = (i..chars.len()).find(|&k| !chars[k].is_ascii_alphabetic()).unwrap_or(chars.len());
                if key_end > i
                    && let Some((op, len)) = read_op(&chars, key_end)
                {
                    let key: String = chars[i..key_end].iter().collect::<String>().to_lowercase();
                    let (v, j) = read_value(&chars, key_end + len, &key)?;
                    out.push(Tok::Term(key, op, v));
                    i = j;
                    continue;
                }
                let j = (i..chars.len()).find(|&k| is_stop(chars[k])).unwrap_or(chars.len());
                let word: String = chars[i..j].iter().collect();
                out.push(match word.to_lowercase().as_str() {
                    "or" => Tok::Or,
                    "and" => Tok::And,
                    _ => Tok::Word(Val::Plain(word)),
                });
                i = j;
            }
        }
    }
    Ok(out)
}

// --- parsing ---------------------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn or(&mut self) -> Result<Expr> {
        let mut alts = vec![self.and()?];
        while self.peek() == Some(&Tok::Or) {
            self.pos += 1;
            alts.push(self.and()?);
        }
        Ok(if alts.len() == 1 { alts.pop().expect("one") } else { Expr::Or(alts) })
    }

    fn and(&mut self) -> Result<Expr> {
        let mut all = Vec::new();
        loop {
            match self.peek() {
                None | Some(Tok::Or | Tok::Close) => break,
                Some(Tok::And) => {
                    if all.is_empty() {
                        bail!("'and' needs something before it");
                    }
                    self.pos += 1;
                }
                Some(_) => all.push(self.unary()?),
            }
        }
        match all.len() {
            0 => bail!(match self.peek() {
                Some(Tok::Close) => "empty ( ) group, or an unmatched ')'",
                _ => "'or' needs something on both sides",
            }),
            1 => Ok(all.pop().expect("one")),
            _ => Ok(Expr::And(all)),
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.peek() == Some(&Tok::Neg) {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        match self.next() {
            Some(Tok::Open) => {
                let e = self.or()?;
                if self.next() != Some(Tok::Close) {
                    bail!("missing ')'");
                }
                Ok(e)
            }
            Some(Tok::Term(key, op, val)) => term(&key, op, val),
            Some(Tok::Word(val)) => Ok(Expr::Term(Term::Name(name_text(val)?))),
            Some(Tok::Exact(name)) => Ok(Expr::Term(Term::ExactName(fold(&name)))),
            Some(Tok::Neg) => bail!("'-' needs something to negate"),
            Some(Tok::Close) => bail!("unmatched ')'"),
            Some(Tok::Or | Tok::And) | None => bail!("'or'/'and' need something on both sides"),
        }
    }
}

fn regex(pattern: &str) -> Result<Regex> {
    Regex::new(&format!("(?i){pattern}")).map_err(|e| anyhow!("bad /regex/: {e}"))
}

fn text(val: Val) -> Result<Text> {
    Ok(match val {
        Val::Plain(s) | Val::Quoted(s) => Text::Contains(s.to_lowercase()),
        Val::Regex(r) => Text::Regex(regex(&r)?),
    })
}

/// Names match loosely, like commander names do: case, accents and punctuation ignored.
fn name_text(val: Val) -> Result<Text> {
    Ok(match val {
        Val::Plain(s) | Val::Quoted(s) => Text::Contains(search_key(&s).trim().to_string()),
        Val::Regex(r) => Text::Regex(regex(&r)?),
    })
}

fn plain(val: Val, key: &str) -> Result<String> {
    match val {
        Val::Plain(s) | Val::Quoted(s) => Ok(s),
        Val::Regex(_) => bail!("'{key}' doesn't take a /regex/"),
    }
}

fn cmp(op: Op) -> Cmp {
    match op {
        Op::Colon | Op::Eq => Cmp::Eq,
        Op::Ne => Cmp::Ne,
        Op::Lt => Cmp::Lt,
        Op::Le => Cmp::Le,
        Op::Gt => Cmp::Gt,
        Op::Ge => Cmp::Ge,
    }
}

fn colors(value: &str) -> Option<Colors> {
    let v = value.to_lowercase();
    let set = |s: &str| Colors::Set(ColorSet::from_letters(s).expect("colour letters"));
    Some(match v.as_str() {
        "c" | "colorless" | "colourless" => Colors::Colorless,
        "m" | "multicolor" | "multicolour" => Colors::Multicolor,
        "white" => set("w"),
        "blue" => set("u"),
        "black" => set("b"),
        "red" => set("r"),
        "green" => set("g"),
        "azorius" => set("wu"),
        "dimir" => set("ub"),
        "rakdos" => set("br"),
        "gruul" => set("rg"),
        "selesnya" => set("gw"),
        "orzhov" | "silverquill" => set("wb"),
        "izzet" | "prismari" => set("ur"),
        "golgari" | "witherbloom" => set("bg"),
        "boros" | "lorehold" => set("rw"),
        "simic" | "quandrix" => set("gu"),
        "bant" => set("gwu"),
        "esper" => set("wub"),
        "grixis" => set("ubr"),
        "jund" => set("brg"),
        "naya" => set("rgw"),
        "abzan" => set("wbg"),
        "jeskai" => set("urw"),
        "sultai" => set("bgu"),
        "mardu" => set("rwb"),
        "temur" => set("gur"),
        "chaos" | "glint" => set("ubrg"),
        "aggression" | "dune" => set("wbrg"),
        "altruism" | "ink" => set("wurg"),
        "growth" | "witch" => set("wubg"),
        "artifice" | "yore" => set("wubr"),
        "wubrg" | "rainbow" | "five-color" => set("wubrg"),
        s if !s.is_empty() && s.chars().all(|c| "wubrg".contains(c)) => set(s),
        "0" | "1" | "2" | "3" | "4" | "5" => Colors::Count(v.parse().expect("a digit")),
        _ => return None,
    })
}

/// An `is:` term: the printing ones, or [`is_kind`].
fn is_term(value: &str) -> Result<Term> {
    Ok(match value.to_lowercase().as_str() {
        "reprint" => Term::Printing(Printed::REPRINT),
        "universesbeyond" | "ub" => Term::Printing(Printed::UB),
        "unique" => Term::Unique,
        "ubonly" | "universesbeyondonly" => Term::OnlyUb,
        _ => Term::Is(is_kind(value)?),
    })
}

fn is_kind(value: &str) -> Result<Is> {
    let v = value.to_lowercase();
    let canonical = IS_VALUES.iter().find(|names| names.contains(&v.as_str())).map(|names| names[0]);
    let Some(name) = canonical else {
        if PRINTING_IS.contains(&v.as_str()) || PRINTING_IS.contains(&v.replace('_', "").as_str()) {
            bail!(
                "'is:{v}' is about printings; a decklist names cards, not which printing of them, so it can't count here"
            );
        }
        bail!(
            "'is:{v}' isn't something this knows (try {})",
            IS_VALUES.iter().map(|n| n[0]).collect::<Vec<_>>().join(", ")
        );
    };
    use LandCycle as L;
    Ok(match name {
        "gamechanger" => Is::GameChanger,
        "commander" => Is::Commander,
        "brawler" => Is::Brawler,
        "duelcommander" => Is::DuelCommander,
        "oathbreaker" => Is::Oathbreaker,
        "partner" => Is::Partner,
        "companion" => Is::Companion,
        "reserved" => Is::Reserved,
        "spell" => Is::Spell,
        "permanent" => Is::Permanent,
        "historic" => Is::Historic,
        "party" => Is::Party,
        "outlaw" => Is::Outlaw,
        "modal" => Is::Modal,
        "vanilla" => Is::Vanilla,
        "frenchvanilla" => Is::FrenchVanilla,
        "bear" => Is::Bear,
        "hybrid" => Is::Hybrid,
        "phyrexian" => Is::Phyrexian,
        "split" => Is::Split,
        "flip" => Is::Flip,
        "transform" => Is::Transform,
        "meld" => Is::Meld,
        "meldpart" => Is::MeldPart,
        "meldresult" => Is::MeldResult,
        "leveler" => Is::Leveler,
        "dfc" => Is::Dfc,
        "mdfc" => Is::Mdfc,
        "creatureland" => Is::Land(L::Creature),
        "bikeland" => Is::Land(L::Bike),
        "bondland" => Is::Land(L::Bond),
        "bounceland" => Is::Land(L::Bounce),
        "canopyland" => Is::Land(L::Canopy),
        "checkland" => Is::Land(L::Check),
        "dual" => Is::Land(L::Dual),
        "fastland" => Is::Land(L::Fast),
        "fetchland" => Is::Land(L::Fetch),
        "filterland" => Is::Land(L::Filter),
        "gainland" => Is::Land(L::Gain),
        "painland" => Is::Land(L::Pain),
        "pathway" => Is::Land(L::Pathway),
        "scryland" => Is::Land(L::Scry),
        "surveilland" => Is::Land(L::Surveil),
        "shadowland" => Is::Land(L::Shadow),
        "shockland" => Is::Land(L::Shock),
        "slowland" => Is::Land(L::Slow),
        "storageland" => Is::Land(L::Storage),
        "tangoland" => Is::Land(L::Tango),
        "tricycleland" => Is::Land(L::Triome),
        "triland" => Is::Land(L::Tri),
        "reprint" | "unique" | "universesbeyond" | "ubonly" => unreachable!("is:{name} is read by is_term"),
        other => unreachable!("is:{other} is in IS_VALUES but not read"),
    })
}

/// A mana cost as typed: `{2}{W}{W}`, or the shorthand `2WW` (braces needed for split symbols
/// like `{R/P}`).
fn mana_query(value: &str, key: &str) -> Result<Symbols> {
    let chars: Vec<char> = value.trim().chars().collect();
    let mut braced = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                let end =
                    (i..chars.len()).find(|&k| chars[k] == '}').ok_or_else(|| anyhow!("unclosed {{ in '{value}'"))?;
                braced.extend(chars[i..=end].iter().map(|c| c.to_ascii_uppercase()));
                i = end + 1;
            }
            c if c.is_ascii_digit() => {
                let end = (i..chars.len()).find(|&k| !chars[k].is_ascii_digit()).unwrap_or(chars.len());
                braced.push('{');
                braced.extend(&chars[i..end]);
                braced.push('}');
                i = end;
            }
            c => {
                braced.push('{');
                braced.push(c.to_ascii_uppercase());
                braced.push('}');
                i += 1;
            }
        }
    }
    let cost: ManaCost = braced.parse().map_err(|e| anyhow!("'{key}' takes mana symbols like 2WW or {{R/P}}: {e}"))?;
    Ok(Symbols::of(Some(&cost)))
}

fn term(key: &str, op: Op, val: Val) -> Result<Expr> {
    let text_key = || {
        if matches!(op, Op::Colon | Op::Eq) { Ok(()) } else { Err(anyhow!("'{key}' takes ':' (like {key}:value)")) }
    };
    let number = |val: Val| -> Result<f64> {
        let s = plain(val, key)?;
        s.trim().parse::<f64>().map_err(|_| anyhow!("'{key}' compares numbers, got '{s}'"))
    };
    let t = match key {
        "t" | "type" => {
            text_key()?;
            Term::Type(text(val)?)
        }
        "o" | "oracle" | "fo" | "fulloracle" => {
            text_key()?;
            Term::Oracle { text: text(val)?, full: key.starts_with('f') }
        }
        "name" => {
            text_key()?;
            Term::Name(name_text(val)?)
        }
        "kw" | "keyword" => {
            text_key()?;
            Term::Keyword(plain(val, key)?.to_lowercase())
        }
        "mv" | "cmc" | "manavalue" => match &val {
            Val::Plain(v) if matches!(v.to_lowercase().as_str(), "even" | "odd") => {
                text_key()?;
                Term::MvEven(v.eq_ignore_ascii_case("even"))
            }
            _ => Term::Mv(cmp(op), number(val)?),
        },
        "m" | "mana" => {
            let q = mana_query(&plain(val, key)?, key)?;
            Term::Mana(if op == Op::Colon { Cmp::Ge } else { cmp(op) }, q)
        }
        "devotion" => {
            let q = mana_query(&plain(val, key)?, key)?;
            let one_kind = match q.pips.as_slice() {
                [(p, n)] if q.generic == 0 && !pip_colors(*p).is_empty() => Some((pip_colors(*p), *n)),
                _ => None,
            };
            let (colors, n) = one_kind.ok_or_else(|| {
                anyhow!("'devotion' takes one coloured symbol, repeated: devotion:rrr or devotion:{{u/b}}{{u/b}}")
            })?;
            Term::Devotion { cmp: if op == Op::Colon { Cmp::Ge } else { cmp(op) }, colors, n }
        }
        "produces" => {
            let v = plain(val, key)?.to_lowercase();
            let set = if !v.is_empty() && v.chars().all(|c| "wubrgc".contains(c)) {
                ManaSet::from_codes(
                    v.chars()
                        .map(|c| c.to_ascii_uppercase().to_string())
                        .collect::<Vec<_>>()
                        .iter()
                        .map(String::as_str),
                )
            } else {
                match colors(&v) {
                    Some(Colors::Set(c)) => c.iter().map(crate::mana::Mana::from).collect(),
                    Some(Colors::Colorless) => ManaSet::of(crate::mana::Mana::C),
                    _ => bail!("'produces' takes kinds of mana: letters like wu or c, or names like azorius"),
                }
            };
            Term::Produces(if op == Op::Colon { Cmp::Ge } else { cmp(op) }, set)
        }
        k if Stat::named(k).is_some() => {
            let stat = Stat::named(k).expect("a stat");
            let v = plain(val, key)?;
            let rhs = match Stat::named(&v.to_lowercase()) {
                Some(other) => StatValue::Stat(other),
                None => StatValue::Num(
                    stat_number(&v)
                        .ok_or_else(|| anyhow!("'{key}' compares numbers or pow, tou, pt, loy, def; got '{v}'"))?,
                ),
            };
            Term::Stat(stat, cmp(op), rhs)
        }
        "usd" => Term::Price(Currency::Usd, cmp(op), number(val)?),
        "eur" => Term::Price(Currency::Eur, cmp(op), number(val)?),
        "tix" => Term::Price(Currency::Tix, cmp(op), number(val)?),
        "edhrec" | "edhrecrank" => Term::Rank(cmp(op), number(val)?),
        "has" => {
            text_key()?;
            match plain(val, key)?.to_lowercase().as_str() {
                "indicator" | "colorindicator" | "ci" => Term::Indicator,
                other => bail!(
                    "'has:{other}' isn't something this knows: only has:indicator is about cards (watermarks and security stamps are on printings)"
                ),
            }
        }
        "f" | "format" | "legal" | "banned" | "restricted" => {
            text_key()?;
            let v = plain(val, key)?.to_lowercase();
            let name = match v.as_str() {
                "edh" => "commander",
                "duelcommander" => "duel",
                "pdh" | "pauperedh" => "paupercommander",
                "pennydreadful" => "penny",
                other => other,
            };
            let i = FORMATS
                .iter()
                .position(|f| *f == name)
                .ok_or_else(|| anyhow!("'{v}' isn't a format Scryfall knows (use {})", FORMATS.join(", ")))?;
            let legality = match key {
                "banned" => Legality::Banned,
                "restricted" => Legality::Restricted,
                _ => Legality::Legal,
            };
            Term::Legality(legality, i)
        }
        "c" | "color" | "colour" | "id" | "identity" | "ci" => {
            let identity = matches!(key, "id" | "identity" | "ci");
            let v = plain(val, key)?;
            let colors = colors(&v).ok_or_else(|| {
                anyhow!("'{v}' isn't a colour (use letters like rg, names like azorius, or a count from 0 to 5)")
            })?;
            // `c:rg` means "at least red and green", `id:rg` "fits a red-green deck"; `c:c`, `c:m`
            // and `c:3` mean colourless, multicolour and exactly three colours outright.
            let cmp = match (op, colors) {
                (Op::Colon, Colors::Colorless | Colors::Multicolor | Colors::Count(_)) => Cmp::Eq,
                (Op::Colon, _) if identity => Cmp::Le,
                (Op::Colon, _) => Cmp::Ge,
                (op, _) => cmp(op),
            };
            if colors == Colors::Multicolor && !matches!(cmp, Cmp::Eq | Cmp::Ne) {
                bail!("'{key}' can only test multicolour with ':', '=' or '!='");
            }
            Term::Color { identity, cmp, colors }
        }
        "is" | "not" => {
            if op != Op::Colon {
                bail!("'{key}' takes ':' (like {key}:commander)");
            }
            let t = Expr::Term(is_term(&plain(val, key)?)?);
            return Ok(if key == "not" { Expr::Not(Box::new(t)) } else { t });
        }
        "otag" | "oracletag" | "function" => {
            text_key()?;
            Term::Tag { slug: tag_slug(&plain(val, key)?), ids: None }
        }
        "s" | "e" | "set" | "edition" | "st" | "b" | "block" | "g" | "group" | "in" => {
            text_key()?;
            let value = plain(val, key)?.to_lowercase();
            let kind = match key {
                "st" => SetKind::Type,
                "b" | "block" => SetKind::Block,
                "g" | "group" => SetKind::Group,
                "in" => SetKind::Any,
                _ => SetKind::Code,
            };
            if kind == SetKind::Any {
                if let Some(g) = Game::named(&value) {
                    return Ok(Expr::Term(Term::InGame(g)));
                }
                // Full names only: `in:m` would rather be a set.
                if let Some(r) = Rarity::named(&value).filter(|_| value.len() > 1) {
                    return Ok(Expr::Term(Term::InRarity(r)));
                }
            }
            Term::Sets { kind, value, codes: None }
        }
        "r" | "rarity" => {
            let v = plain(val, key)?;
            let r = Rarity::named(&v)
                .ok_or_else(|| anyhow!("'{v}' isn't a rarity (use common, uncommon, rare, mythic, special, bonus)"))?;
            Term::Rarity(cmp(op), r)
        }
        "year" => {
            let v = plain(val, key)?;
            let y = match v.to_lowercase().as_str() {
                "now" | "today" => today() / 10000,
                _ => v.trim().parse().map_err(|_| anyhow!("'year' compares years, got '{v}'"))?,
            };
            Term::Year(cmp(op), y)
        }
        "date" => {
            let v = plain(val, key)?.to_lowercase();
            let day = match v.as_str() {
                "now" | "today" => DateValue::Day(today()),
                _ => match parse_date(&v) {
                    Some(d) => DateValue::Day(d),
                    None if v.chars().all(|c| c.is_ascii_alphanumeric()) => DateValue::Set(v),
                    None => bail!("'date' takes a date (2015-08-18), a set code (ori) or now, got '{v}'"),
                },
            };
            Term::Date(cmp(op), day)
        }
        k if PRINTING_KEYS.contains(&k) => bail!(
            "'{k}' is about printings (set, rarity, art, ...); a decklist names cards, not which printing of them, so it can't count here"
        ),
        k if DISPLAY_KEYS.contains(&k) => {
            bail!("'{k}:' only changes how Scryfall shows its results, not which cards match")
        }
        "cube" => bail!("Scryfall's cube lists aren't in the card database"),
        other => bail!("'{other}' isn't a search key this supports (use {SUPPORTED})"),
    };
    Ok(Expr::Term(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(letters: &str) -> ColorSet {
        ColorSet::from_letters(letters).unwrap()
    }

    fn card(name: &str, type_line: &str, oracle: &str) -> Card {
        Card {
            name: name.into(),
            oracle_id: name.to_lowercase(),
            type_line: type_line.into(),
            oracle: vec![oracle.into()],
            ..Default::default()
        }
    }

    fn cards() -> Vec<Card> {
        let mut bonesplitter = card("Bonesplitter", "Artifact — Equipment", "Equipped creature gets +2/+0.\nEquip {1}");
        bonesplitter.keywords = vec!["Equip".into()];
        bonesplitter.mv = 1.0;
        bonesplitter.colors = Some(ColorSet::empty());
        bonesplitter.usd = 0.5;
        let mut lizard = card(
            "Lizard Blades",
            "Artifact Creature — Equipment Lizard",
            "Double strike\nEquipped creature has double strike.\nReconfigure {2} ({2}: Attach to target creature you control; or unattach from a creature. Reconfigure only as a sorcery. While attached, this isn't a creature.)",
        );
        lizard.keywords = vec!["Double strike".into(), "Reconfigure".into()];
        lizard.mv = 2.0;
        lizard.colors = Some(cs("R"));
        lizard.identity = cs("R");
        let mut saga = card(
            "History of Benalia",
            "Enchantment — Saga",
            "(As this Saga enters and after your draw step, add a lore counter.)\nI, II — Create a 2/2 white Knight creature token with vigilance.",
        );
        saga.mv = 3.0;
        saga.colors = Some(cs("W"));
        saga.identity = cs("W");
        let mut krenko = card(
            "Krenko, Mob Boss",
            "Legendary Creature — Goblin Warrior",
            "{T}: Create X 1/1 red Goblin creature tokens, where X is the number of Goblins you control.",
        );
        krenko.mv = 4.0;
        krenko.colors = Some(cs("R"));
        krenko.identity = cs("R");
        let mut bolt = card("Lightning Bolt", "Instant", "Lightning Bolt deals 3 damage to any target.");
        bolt.mv = 1.0;
        bolt.colors = Some(cs("R"));
        bolt.identity = cs("R");
        bolt.usd = 1.0;
        let mut mdfc = card(
            "Valakut Awakening // Valakut Stoneforge",
            "Instant // Land",
            "Put any number of cards from your hand on the bottom of your library, then draw that many cards plus one.",
        );
        mdfc.oracle.push(
            "As Valakut Stoneforge enters, you may pay 3 life. If you don't, it enters tapped.\n{T}: Add {R}.".into(),
        );
        mdfc.faces = vec!["Valakut Awakening".into(), "Valakut Stoneforge".into()];
        mdfc.layout = crate::types::Layout::ModalDfc;
        mdfc.mv = 3.0;
        mdfc.colors = Some(cs("R"));
        mdfc.identity = cs("R");
        let mut rings = card(
            "Boros Charm",
            "Instant",
            "Choose one —\n• Boros Charm deals 4 damage to target player or planeswalker.",
        );
        rings.colors = Some(cs("RW"));
        rings.identity = cs("RW");
        rings.game_changer = true;
        vec![bonesplitter, lizard, saga, krenko, bolt, mdfc, rings]
    }

    fn hits(q: &str) -> Vec<String> {
        let q = Query::parse(q).unwrap_or_else(|e| panic!("{q}: {e}"));
        cards().into_iter().filter(|c| q.matches(c)).map(|c| front_face(&c.name).to_string()).collect()
    }

    #[test]
    fn types_match_partial_words_on_either_face() {
        assert_eq!(hits("t:equipment"), vec!["Bonesplitter", "Lizard Blades"]);
        assert_eq!(hits("t:equip -t:creature"), vec!["Bonesplitter"]);
        assert_eq!(hits("t:legend"), vec!["Krenko, Mob Boss"]);
        assert_eq!(hits("type:saga"), vec!["History of Benalia"]);
        assert_eq!(hits("t:land"), vec!["Valakut Awakening"]);
        assert_eq!(hits(r#"t:"legendary creature""#), vec!["Krenko, Mob Boss"]);
        assert_eq!(hits("T:Equipment"), hits("t:equipment"));
    }

    #[test]
    fn oracle_text_skips_reminder_text_unless_full() {
        assert_eq!(hits("o:reconfigure"), vec!["Lizard Blades"]);
        assert_eq!(hits(r#"o:"attach to target""#), Vec::<String>::new());
        assert_eq!(hits(r#"fo:"attach to target""#), vec!["Lizard Blades"]);
        assert_eq!(hits("o:lore"), Vec::<String>::new());
        assert_eq!(hits(r#"o:"draw that many""#), vec!["Valakut Awakening"]);
        assert_eq!(hits(r#"o:"you may pay 3 life""#), vec!["Valakut Awakening"]); // back face
    }

    #[test]
    fn tilde_is_the_cards_own_name() {
        let mut land = card("Adarkar Wastes", "Land", "{T}: Add {W} or {U}. This land deals 1 damage to you.");
        land.oracle_id = "adarkar".into();
        let q = Query::parse(r#"o:"~ deals 1 damage to you""#).unwrap();
        assert!(q.matches(&land), "'this land' is the card itself");
        assert!(!q.matches(&card("Other", "Instant", "This creature deals 1 damage to you.")));
        assert_eq!(hits(r#"o:"~ deals""#), vec!["Lightning Bolt", "Boros Charm"]);
        assert_eq!(hits(r#"o:"~ enters""#), vec!["Valakut Awakening"]);
        assert_eq!(hits(r#"o:/^~ deals \d damage/"#), vec!["Lightning Bolt"]);
    }

    #[test]
    fn regexes_are_case_insensitive() {
        assert_eq!(hits(r"o:/create (a|x) .*goblin/"), vec!["Krenko, Mob Boss"]);
        assert_eq!(hits("t:/^artifact —/"), vec!["Bonesplitter"]);
        assert_eq!(hits(r"o:/equip \{1\}/"), vec!["Bonesplitter"]);
    }

    #[test]
    fn names_keywords_and_numbers() {
        assert_eq!(hits("krenko"), vec!["Krenko, Mob Boss"]);
        assert_eq!(hits(r#""mob boss""#), vec!["Krenko, Mob Boss"]);
        assert_eq!(hits("name:valakut"), vec!["Valakut Awakening"]);
        assert_eq!(hits(r#"!"lightning bolt""#), vec!["Lightning Bolt"]);
        assert_eq!(hits(r#"!"valakut stoneforge""#), vec!["Valakut Awakening"]);
        assert_eq!(hits("!lightning"), Vec::<String>::new());
        assert_eq!(hits("kw:equip"), vec!["Bonesplitter"]);
        assert_eq!(hits(r#"keyword:"double strike""#), vec!["Lizard Blades"]);
        assert_eq!(hits("t:equipment mv<=1"), vec!["Bonesplitter"]);
        assert_eq!(hits("mv>=3 mv!=4"), vec!["History of Benalia", "Valakut Awakening"]);
        assert_eq!(hits("cmc=2"), vec!["Lizard Blades"]);
        assert_eq!(hits("usd<1"), vec!["Bonesplitter"]); // unpriced cards never match
    }

    #[test]
    fn colours_and_identity() {
        assert_eq!(hits("c:w"), vec!["History of Benalia", "Boros Charm"]); // at least white
        assert_eq!(hits("c=r t:instant"), vec!["Lightning Bolt", "Valakut Awakening"]);
        assert_eq!(hits("c:boros"), vec!["Boros Charm"]);
        assert_eq!(hits("c:m"), vec!["Boros Charm"]);
        assert_eq!(hits("c:c"), vec!["Bonesplitter"]);
        assert_eq!(hits("id:r t:instant"), vec!["Lightning Bolt", "Valakut Awakening"]); // fits mono-red
        assert_eq!(hits("id<=rw t:instant").len(), 3);
        assert_eq!(hits("id>r"), vec!["Boros Charm"]);
    }

    #[test]
    fn a_double_faced_card_is_all_its_faces_colours() {
        let mut plargg = card("Plargg, Dean of Chaos // Augusta, Dean of Order", "Legendary Creature", "");
        plargg.layout = crate::types::Layout::ModalDfc;
        plargg.colors = Some(cs("R"));
        plargg.face_colors = vec![cs("R"), cs("W")];
        plargg.identity = cs("RW");
        let matches = |q: &str| Query::parse(q).unwrap().matches(&plargg);
        // Red on one face and white on the other make a red-white card. Scryfall answers the
        // other way round for each of these, matching each face on its own.
        for q in ["c=rw", "c=2", "c>=rw", "c:m", "c:w", "id=rw"] {
            assert!(matches(q), "{q} should match: Plargg // Augusta is red and white");
        }
        for q in ["c=r", "c=w", "c=1", "-c:w"] {
            assert!(!matches(q), "{q} shouldn't match: Plargg // Augusta isn't one colour");
        }
    }

    #[test]
    fn a_number_counts_colours_as_on_scryfall() {
        assert_eq!(hits("c=2"), vec!["Boros Charm"]);
        assert_eq!(hits("c:2"), vec!["Boros Charm"], "with a number, : means exactly, as on Scryfall");
        assert_eq!(hits("c>=2"), vec!["Boros Charm"]);
        assert_eq!(hits("c=0"), hits("c:c"), "no colours is colourless");
        assert_eq!(hits("c=1 t:instant"), vec!["Lightning Bolt", "Valakut Awakening"]);
        assert_eq!(hits("c>=r c=2"), vec!["Boros Charm"], "named colours and a count combine");
        assert_eq!(hits("c>=r c=3"), Vec::<String>::new());
        assert_eq!(hits("c!=1 t:instant"), vec!["Boros Charm"]);
        assert_eq!(hits("id<=1 t:instant"), vec!["Lightning Bolt", "Valakut Awakening"]);
        let err = Query::parse("c=6").unwrap_err().to_string();
        assert!(err.contains("a count from 0 to 5"), "c=6 should say colour counts stop at 5: {err}");
    }

    #[test]
    fn is_and_not() {
        assert_eq!(hits("is:gamechanger"), vec!["Boros Charm"]);
        assert_eq!(hits("is:commander"), vec!["Krenko, Mob Boss"]);
        assert_eq!(hits("is:mdfc"), vec!["Valakut Awakening"]);
        assert_eq!(
            hits("is:historic"),
            vec!["Bonesplitter", "Lizard Blades", "History of Benalia", "Krenko, Mob Boss"]
        );
        assert_eq!(hits("t:instant not:gamechanger"), vec!["Lightning Bolt", "Valakut Awakening"]);
        assert_eq!(hits("is:permanent -t:artifact"), vec!["History of Benalia", "Krenko, Mob Boss"]);
    }

    #[test]
    fn boolean_structure() {
        assert_eq!(hits("t:saga or t:legendary"), vec!["History of Benalia", "Krenko, Mob Boss"]);
        assert_eq!(hits("(t:saga OR t:instant) c:w"), vec!["History of Benalia", "Boros Charm"]);
        assert_eq!(hits("t:instant and -(c:w or is:mdfc)"), vec!["Lightning Bolt"]);
        assert_eq!(hits("t:equipment or t:saga mv>=3"), vec!["Bonesplitter", "Lizard Blades", "History of Benalia"]);
    }

    #[test]
    fn otags_need_the_tag_tree() {
        let raw = concat!(
            r#"{"id":"1","label":"burn","slug":"burn","child_ids":[],"taggings":[{"oracle_id":"lightning bolt"}]}"#,
            "\n",
            r#"{"id":"2","label":"removal","slug":"removal","child_ids":["1"],"taggings":[]}"#,
        );
        let tags = TagIndex::read(raw.as_bytes()).unwrap();
        let mut q = Query::parse("otag:removal t:instant").unwrap();
        assert!(q.needs_tags() && !Query::parse("t:instant").unwrap().needs_tags());
        assert!(cards().iter().all(|c| !q.matches(c)), "unresolved tags match nothing");
        q.resolve_tags(&tags).unwrap();
        let names: Vec<_> = cards().into_iter().filter(|c| q.matches(c)).map(|c| c.name).collect();
        assert_eq!(names, vec!["Lightning Bolt"]);
        let err = Query::parse("otag:remov").unwrap().resolve_tags(&tags).unwrap_err().to_string();
        assert!(err.contains("did you mean removal"), "{err}");
    }

    #[test]
    fn words_name_what_the_query_looks_for() {
        assert_eq!(Query::parse("t:equipment -t:creature").unwrap().words(), vec!["equipment"]);
        assert_eq!(Query::parse(r#"o:treasure or otag:"mana rock""#).unwrap().words(), vec!["treasure", "mana-rock"]);
        assert!(Query::parse(r#"o:"create a treasure""#).unwrap().words().is_empty());
        let q = Query::parse(r#"(t:equipment or o:"create a treasure") -t:creature o:/x/ o:"~ deals""#).unwrap();
        assert_eq!(q.texts(), vec!["equipment", "create a treasure"]);
    }

    #[test]
    fn mistakes_are_errors() {
        for bad in [
            "",
            "cn:12",
            "t:",
            "t: equipment",
            "(t:saga",
            "t:saga)",
            "t:saga or",
            "or t:saga",
            "- t:saga",
            "o:\"unclosed",
            "o:/unclosed",
            "o:/(/",
            "mv:two",
            "c:purple",
            "is:legendary",
            "t>saga",
            "c<m",
            "()",
        ] {
            assert!(Query::parse(bad).is_err(), "should reject {bad:?}");
        }
        let e = Query::parse("frobnicate:x").unwrap_err().to_string();
        assert!(e.contains("frobnicate") && e.contains("t: o:"), "an unknown key should list the known ones: {e}");
    }

    #[test]
    fn printing_and_display_keys_say_why_they_cant_count() {
        for (q, says) in [
            ("cn>400", "printings"),
            ("frame:showcase", "printings"),
            ("a:avon", "printings"),
            ("ft:mishra", "printings"),
            ("atag:squirrel", "printings"),
            ("is:promo", "printings"),
            ("is:foil", "printings"),
            ("has:watermark", "printings"),
            ("order:cmc", "shows"),
            ("unique:prints", "shows"),
            ("cube:vintage", "cube"),
        ] {
            let e = Query::parse(q).unwrap_err().to_string();
            assert!(e.contains(says), "{q} should say why it can't count ({says}): {e}");
        }
    }

    use crate::cards::{FaceStats, Legalities};

    fn cost(c: &str) -> Vec<Option<ManaCost>> {
        c.split(" // ").map(|f| f.parse().ok()).collect()
    }

    fn stats(p: &str, t: &str) -> FaceStats {
        FaceStats { power: Some(p.into()), toughness: Some(t.into()), ..Default::default() }
    }

    /// Cards for the newer keys, each with what they need set.
    fn more_cards() -> Vec<Card> {
        let mut serra = card("Serra Angel", "Creature — Angel", "Flying, vigilance");
        (serra.mana_cost, serra.mv, serra.stats) = (cost("{3}{W}{W}"), 5.0, vec![stats("4", "4")]);
        serra.keywords = vec!["Flying".into(), "Vigilance".into()];
        serra.legalities = Legalities::from_scryfall([("commander", "legal"), ("pauper", "legal")]);
        serra.usd = 0.25;
        serra.eur = 0.1;
        serra.rank = Some(5000);
        let mut goyf = card("Tarmogoyf", "Creature — Lhurgoyf", "Tarmogoyf's power is equal to …");
        (goyf.mana_cost, goyf.mv, goyf.stats) = (cost("{1}{G}"), 2.0, vec![stats("*", "1+*")]);
        goyf.legalities = Legalities::from_scryfall([("commander", "legal"), ("vintage", "legal")]);
        let mut bears = card("Grizzly Bears", "Creature — Bear", "");
        (bears.mana_cost, bears.mv, bears.stats) = (cost("{1}{G}"), 2.0, vec![stats("2", "2")]);
        let mut hoverbike = card("Hoverbike", "Artifact — Vehicle", "Crew 1");
        (hoverbike.mana_cost, hoverbike.mv, hoverbike.stats) = (cost("{2}"), 2.0, vec![stats("2", "2")]);
        hoverbike.keywords = vec!["Crew".into()];
        let mut kitchen =
            card("Kitchen Finks", "Creature — Ouphe", "When this creature enters, you gain 2 life.\nPersist");
        (kitchen.mana_cost, kitchen.mv, kitchen.stats) = (cost("{1}{G/W}{G/W}"), 3.0, vec![stats("3", "2")]);
        let mut mutagenic = card("Mutagenic Growth", "Instant", "Target creature gets +2/+2 until end of turn.");
        (mutagenic.mana_cost, mutagenic.mv) = (cost("{G/P}"), 1.0);
        let mut jace = card("Jace Beleren", "Legendary Planeswalker — Jace", "+2: Each player draws a card.");
        (jace.mana_cost, jace.mv) = (cost("{1}{U}{U}"), 3.0);
        jace.stats = vec![FaceStats { loyalty: Some("3".into()), ..Default::default() }];
        jace.legalities = Legalities::from_scryfall([("vintage", "restricted"), ("commander", "banned")]);
        let mut fire_ice = card("Fire // Ice", "Instant // Instant", "Fire deals 2 damage divided as you choose …");
        fire_ice.oracle.push("Tap target permanent.\nDraw a card.".into());
        fire_ice.faces = vec!["Fire".into(), "Ice".into()];
        (fire_ice.mana_cost, fire_ice.mv, fire_ice.layout) = (cost("{1}{R} // {1}{U}"), 4.0, Layout::Split);
        let mut tower =
            card("Command Tower", "Land", "{T}: Add one mana of any color in your commander's color identity.");
        tower.mana_cost = vec![None];
        tower.produced = ManaSet::from_codes(["W", "U", "B", "R", "G"]);
        let mut sol = card("Sol Ring", "Artifact", "{T}: Add {C}{C}.");
        (sol.mana_cost, sol.mv, sol.produced) = (cost("{1}"), 1.0, ManaSet::from_codes(["C"]));
        let mut signet = card("Azorius Signet", "Artifact", "{1}, {T}: Add {W}{U}.");
        (signet.mana_cost, signet.mv, signet.produced) = (cost("{2}"), 2.0, ManaSet::from_codes(["W", "U"]));
        let mut ornithopter = card("Ornithopter", "Artifact Creature — Thopter", "Flying");
        (ornithopter.mana_cost, ornithopter.stats) = (cost("{0}"), vec![stats("0", "2")]);
        ornithopter.keywords = vec!["Flying".into()];
        let mut delver = card(
            "Delver of Secrets // Insectile Aberration",
            "Creature — Human Wizard // Creature — Human Insect",
            "At the beginning of your upkeep, look at the top card of your library.",
        );
        delver.oracle.push("Flying".into());
        delver.faces = vec!["Delver of Secrets".into(), "Insectile Aberration".into()];
        (delver.mana_cost, delver.mv, delver.layout) =
            (vec![Some("{U}".parse().unwrap()), None], 1.0, Layout::Transform);
        delver.stats = vec![stats("1", "1"), stats("3", "2")];
        delver.indicator = true;
        vec![serra, goyf, bears, hoverbike, kitchen, mutagenic, jace, fire_ice, tower, sol, signet, ornithopter, delver]
    }

    fn more_hits(q: &str) -> Vec<String> {
        let q = Query::parse(q).unwrap_or_else(|e| panic!("{q}: {e}"));
        more_cards().into_iter().filter(|c| q.matches(c)).map(|c| front_face(&c.name).to_string()).collect()
    }

    #[test]
    fn mana_costs_compare_by_their_symbols() {
        assert_eq!(more_hits("m:2WW"), vec!["Serra Angel"], "{{3}}{{W}}{{W}} has at least {{2}}{{W}}{{W}}");
        assert_eq!(more_hits("m:WW"), more_hits("m:{W}{W}"), "shorthand and braces are the same");
        assert_eq!(more_hits("m={1}{G}"), vec!["Tarmogoyf", "Grizzly Bears"], "= is exactly");
        assert_eq!(more_hits("m:{W/G}"), vec!["Kitchen Finks"], "a hybrid symbol reads either way round");
        assert_eq!(more_hits("m:{G/P}"), vec!["Mutagenic Growth"]);
        assert_eq!(more_hits("m:1U"), vec!["Jace Beleren", "Fire"], "either half of a split card");
        assert_eq!(more_hits("m=0"), vec!["Ornithopter"], "{{0}} is a cost; a land has none");
        assert!(more_hits("m<{1}").contains(&"Command Tower".to_string()), "no cost is less than any");
        assert!(!more_hits("m:U").contains(&"Insectile Aberration".to_string()));
        assert!(Query::parse("m:{Q}").is_err());
    }

    #[test]
    fn devotion_counts_a_permanents_coloured_symbols() {
        assert_eq!(more_hits("devotion:ww"), vec!["Serra Angel", "Kitchen Finks"], "{{G/W}} counts towards white");
        assert_eq!(more_hits("devotion={g/w}{g/w}"), vec!["Serra Angel", "Kitchen Finks"], "white or green, either");
        assert_eq!(more_hits("devotion=gg"), vec!["Kitchen Finks"]);
        assert!(more_hits("devotion:g").iter().all(|n| n != "Mutagenic Growth"), "an instant has no devotion");
        assert!(Query::parse("devotion:wu").is_err(), "one kind of symbol at a time");
    }

    #[test]
    fn produces_compares_kinds_of_mana_like_colours() {
        assert_eq!(more_hits("produces:wu"), vec!["Command Tower", "Azorius Signet"], ": is at least");
        assert_eq!(more_hits("produces=wu"), vec!["Azorius Signet"], "= is just these");
        assert_eq!(more_hits("produces:c"), vec!["Sol Ring"]);
        assert_eq!(more_hits("produces:azorius"), more_hits("produces:wu"));
    }

    #[test]
    fn power_toughness_and_loyalty_compare_each_face() {
        assert_eq!(more_hits("pow>=4"), vec!["Serra Angel"]);
        assert_eq!(more_hits("pow=0 t:creature"), vec!["Tarmogoyf", "Ornithopter"], "* counts as 0");
        assert_eq!(more_hits("tou=1 t:lhurgoyf"), vec!["Tarmogoyf"], "1+* is 1");
        assert_eq!(more_hits("pow>tou"), vec!["Kitchen Finks", "Delver of Secrets"], "the back face is 3/2");
        assert_eq!(more_hits("pt=8"), vec!["Serra Angel"]);
        assert_eq!(more_hits("loy=3"), vec!["Jace Beleren"]);
        assert!(Query::parse("pow>lots").is_err());
    }

    #[test]
    fn legality_prices_rank_and_colour_indicators() {
        assert_eq!(more_hits("f:pauper"), vec!["Serra Angel"]);
        assert_eq!(more_hits("f:vintage"), vec!["Tarmogoyf", "Jace Beleren"], "restricted is still playable");
        assert_eq!(more_hits("restricted:vintage"), vec!["Jace Beleren"]);
        assert_eq!(more_hits("banned:edh"), vec!["Jace Beleren"], "edh is commander");
        assert_eq!(more_hits("eur<1"), vec!["Serra Angel"], "unpriced cards never match");
        assert_eq!(more_hits("edhrec>100"), vec!["Serra Angel"]);
        assert_eq!(more_hits("has:indicator"), vec!["Delver of Secrets"]);
        assert_eq!(more_hits("mv:odd t:creature"), vec!["Serra Angel", "Kitchen Finks", "Delver of Secrets"]);
        assert!(Query::parse("f:freeform").unwrap_err().to_string().contains("pauper"));
    }

    #[test]
    fn kinds_of_card() {
        assert_eq!(more_hits("is:bear"), vec!["Grizzly Bears", "Hoverbike"], "2/2 for 2, Vehicles too");
        assert_eq!(more_hits("is:vanilla"), vec!["Grizzly Bears"]);
        assert_eq!(more_hits("is:frenchvanilla"), vec!["Serra Angel", "Ornithopter"]);
        assert_eq!(more_hits("is:hybrid"), vec!["Kitchen Finks"]);
        assert_eq!(more_hits("is:phyrexian"), vec!["Mutagenic Growth"]);
        assert_eq!(more_hits("is:split"), vec!["Fire"]);
        assert_eq!(more_hits("is:transform"), more_hits("is:tdfc"));
        assert!(!more_hits("is:spell").contains(&"Command Tower".to_string()));
        let text = |type_line: &str, oracle: &str| card("X", type_line, oracle);
        let is = |q: &str, c: &Card| Query::parse(q).unwrap().matches(c);
        assert!(is("is:party", &text("Creature — Elf", "X is also a Cleric, Rogue, Warrior, and Wizard.")));
        let mut changeling = text("Creature — Shapeshifter", "Changeling");
        changeling.keywords = vec!["Changeling".into()];
        assert!(is("is:outlaw", &changeling) && is("is:party", &changeling));
        assert!(is("is:modal", &text("Instant", "Choose one —\n• Draw a card.\n• Gain 3 life.")));
        assert!(is(
            "is:modal",
            &text("Sorcery", "Choose three. You may choose the same mode more than once.\n• A\n• B")
        ));
        assert!(!is(
            "is:modal",
            &text("Enchantment", "As this enchantment enters, choose Abzan or Mardu.\n• Abzan — A")
        ));
        let mut rampager = text("Creature — Human", "Rampage 3 (Whenever this creature becomes blocked, …)");
        rampager.keywords = vec!["Rampage".into()];
        assert!(!is("is:frenchvanilla", &rampager), "as on Scryfall, a keyword with a number doesn't count");
        let mut warder = text("Creature — Dwarf", "Ward—Pay 2 life.\nProwess");
        warder.keywords = vec!["Ward".into(), "Prowess".into()];
        assert!(is("is:frenchvanilla", &warder));
        let mut powered = text("Creature — Gamma", "Trample\nPower-up — {5}: Put a +1/+1 counter on X.");
        powered.keywords = vec!["Trample".into(), "Power-up".into()];
        assert!(!is("is:frenchvanilla", &powered), "an ability word isn't a keyword ability");
    }

    #[test]
    fn who_can_lead() {
        let lead = |type_line: &str, oracle: &str, keywords: &[&str], legal: &str| {
            let mut c = card("X", type_line, oracle);
            c.keywords = keywords.iter().map(|k| k.to_string()).collect();
            c.stats = vec![stats("4", "4")];
            c.legalities = Legalities::from_scryfall([("commander", legal), ("duel", legal), ("oathbreaker", legal)]);
            c
        };
        let is = |q: &str, c: &Card| Query::parse(q).unwrap().matches(c);
        assert!(is("is:commander", &lead("Legendary Artifact — Vehicle", "Crew 4", &["Crew"], "legal")));
        assert!(!is("is:commander", &lead("Legendary Creature — Demon", "", &[], "banned")), "banned in Commander");
        let background = lead("Legendary Enchantment — Background", "", &[], "legal");
        assert!(is("is:partner", &background), "a Background shares the command zone");
        assert!(!is("is:duelcommander", &background));
        assert!(is("is:partner", &lead("Legendary Creature — Elf", "Partner", &["Partner"], "legal")));
        assert!(
            !is("is:partner", &lead("Creature — Human", "Partner with Y", &["Partner with"], "legal")),
            "not legendary"
        );
        assert!(is("is:oathbreaker", &lead("Legendary Sorcery", "X can be your commander.", &[], "legal")));
    }

    #[test]
    fn land_cycles_read_rules_text() {
        let land = |name: &str, type_line: &str, oracle: &str| {
            let mut c = card(name, type_line, oracle);
            c.mana_cost = vec![None];
            c
        };
        let is = |q: &str, c: &Card| Query::parse(q).unwrap().matches(c);
        let shock = land(
            "Steam Vents",
            "Land — Island Mountain",
            "({T}: Add {U} or {R}.)\nAs Steam Vents enters, you may pay 2 life. If you don't, it enters tapped.",
        );
        assert!(is("is:shockland", &shock) && !is("is:dual", &shock));
        assert!(is("is:dual", &land("Volcanic Island", "Land — Island Mountain", "({T}: Add {U} or {R}.)")));
        let check = land(
            "Sulfur Falls",
            "Land",
            "This land enters tapped unless you control an Island or a Mountain.\n{T}: Add {U} or {R}.",
        );
        assert!(is("is:checkland", &check));
        let mono = land(
            "Kishla Village",
            "Land",
            "This land enters tapped unless you control a Forest or an Island.\n{T}: Add {C}.",
        );
        assert!(!is("is:checkland", &mono), "a check land taps for two colours");
        let gain = |name: &str| {
            land(
                name,
                "Land",
                "This land enters tapped.\nWhen this land enters, you gain 1 life.\n{T}: Add {U} or {B}.",
            )
        };
        assert!(
            is("is:gainland", &gain("Dismal Backwater")) && is("is:gainland", &gain("A.I.M. Labs")),
            "renamed reprints count"
        );
        let tri = land("Arcane Sanctum", "Land", "This land enters tapped.\n{T}: Add {W}, {U}, or {B}.");
        assert!(
            is("is:triland", &tri)
                && is(
                    "is:karoo",
                    &land(
                        "Dimir Aqueduct",
                        "Land",
                        "This land enters tapped.\nWhen this land enters, return a land you control to its owner's hand.\n{T}: Add {U}{B}."
                    )
                )
        );
        let fetch = land(
            "Polluted Delta",
            "Land",
            "{T}, Pay 1 life, Sacrifice this land: Search your library for an Island or Swamp card, put it onto the battlefield, then shuffle.",
        );
        assert!(is("is:fetchland", &fetch) && !is("is:fetchland", &tri));
        let manland = land(
            "Mutavault",
            "Land",
            "{T}: Add {C}.\n{1}: Until end of turn, this land becomes a 2/2 creature with all creature types. It's still a land.",
        );
        assert!(is("is:manland", &manland) && is("is:creatureland", &manland));
    }

    fn printed(set: &str, rarity: Rarity, released: u32, flags: u8) -> Printed {
        Printed { set: set.into(), rarity, released, flags }
    }

    fn set_list() -> Vec<SetInfo> {
        let set = |code: &str, set_type: &str, block: Option<&str>, parent: Option<&str>, released: u32| SetInfo {
            code: code.into(),
            name: format!("Set {code}"),
            set_type: set_type.into(),
            block: block.map(Into::into),
            parent: parent.map(Into::into),
            released,
        };
        vec![
            set("lea", "core", Some("lea"), None, 19930805),
            set("m10", "core", None, None, 20090717),
            set("zen", "expansion", Some("zen"), None, 20091002),
            set("wwk", "expansion", Some("zen"), None, 20100205),
            set("fin", "expansion", None, None, 20250613),
            set("fic", "commander", None, Some("fin"), 20250613),
            set("sld", "box", None, None, 20191202),
        ]
    }

    /// Lightning Bolt (Alpha common, M10 common, a Secret Lair rare), Lotus Cobra (Worldwake rare),
    /// a Final Fantasy commander (a Universes Beyond mythic in FIC only).
    fn printed_cards() -> Vec<Card> {
        use Rarity::*;
        let paper = Game::Paper as u8;
        let mut bolt = card("Lightning Bolt", "Instant", "Lightning Bolt deals 3 damage to any target.");
        bolt.printings = vec![
            printed("lea", Common, 19930805, paper),
            printed("m10", Common, 20090717, paper | Printed::REPRINT | Game::Arena as u8),
            printed("sld", Rare, 20210101, paper | Printed::REPRINT | Printed::SIDE_PRODUCT),
        ];
        let mut cobra = card("Lotus Cobra", "Creature — Snake", "Landfall — …");
        cobra.printings = vec![printed("wwk", Rare, 20100205, paper)];
        let mut cid = card("Cid, Timeless Artificer", "Legendary Creature — Human Artificer", "");
        cid.printings = vec![printed("fic", Mythic, 20250613, paper | Printed::UB)];
        vec![bolt, cobra, cid]
    }

    fn printed_hits(q: &str) -> Vec<String> {
        let mut q = Query::parse(q).unwrap_or_else(|e| panic!("{q}: {e}"));
        if q.needs_sets() {
            q.resolve_sets(&set_list()).unwrap_or_else(|e| panic!("{q}: {e}"));
        }
        printed_cards().into_iter().filter(|c| q.matches(c)).map(|c| c.name).collect()
    }

    #[test]
    fn set_keys_ask_one_printing_at_a_time() {
        assert_eq!(printed_hits("s:lea"), vec!["Lightning Bolt"]);
        assert_eq!(printed_hits("e:M10"), printed_hits("set:m10"), "codes in any case");
        assert_eq!(printed_hits("s:lea r:common"), vec!["Lightning Bolt"]);
        assert!(printed_hits("s:lea r:rare").is_empty(), "the rare printing isn't the Alpha one");
        assert_eq!(printed_hits("r>=rare"), vec!["Lightning Bolt", "Lotus Cobra", "Cid, Timeless Artificer"]);
        assert_eq!(printed_hits("st:box"), vec!["Lightning Bolt"]);
        assert_eq!(printed_hits("b:zen"), vec!["Lotus Cobra"], "Worldwake is in Zendikar block");
        assert_eq!(printed_hits("g:fin"), vec!["Cid, Timeless Artificer"], "FIC goes with FIN");
        assert_eq!(
            printed_hits("-s:lea"),
            vec!["Lightning Bolt", "Lotus Cobra", "Cid, Timeless Artificer"],
            "a printing outside Alpha"
        );
        assert_eq!(printed_hits("year<=1994"), vec!["Lightning Bolt"]);
        assert_eq!(printed_hits("year=2009 not:reprint"), Vec::<String>::new(), "M10's Bolt is a reprint");
        assert_eq!(
            printed_hits("date>zen date<fin"),
            vec!["Lightning Bolt", "Lotus Cobra"],
            "after Zendikar, before Final Fantasy"
        );
        assert_eq!(printed_hits("date>=2025-01-01"), vec!["Cid, Timeless Artificer"]);
        assert_eq!(printed_hits("is:universesbeyond"), vec!["Cid, Timeless Artificer"]);
        assert_eq!(printed_hits("is:unique"), vec!["Lotus Cobra", "Cid, Timeless Artificer"]);
        let mut sol = card("Sol Ring", "Artifact", "");
        sol.printings = vec![
            printed("fic", Rarity::Uncommon, 20250613, Printed::UB),
            printed("lea", Rarity::Uncommon, 19930805, 0),
        ];
        let is = |q: &str, c: &Card| Query::parse(q).unwrap().matches(c);
        assert!(is("is:ub", &sol) && !is("is:ubonly", &sol), "a UB reprint doesn't make a staple a UB card");
        assert!(is("is:ubonly", &printed_cards()[2]));
        assert!(is("-not:ub", &sol), "negation is per printing too: Sol Ring has a UB one");
        assert!(
            !Query::parse("s:lea").unwrap().matches(&card("Lightning Bolt", "Instant", "")),
            "a card without printings listed matches no set"
        );
    }

    #[test]
    fn in_looks_at_every_printing() {
        assert_eq!(printed_hits("-in:lea"), vec!["Lotus Cobra", "Cid, Timeless Artificer"], "never printed in Alpha");
        assert_eq!(printed_hits("in:lea in:m10"), vec!["Lightning Bolt"]);
        assert_eq!(printed_hits("in:commander"), vec!["Cid, Timeless Artificer"], "a set type");
        assert_eq!(printed_hits("in:arena"), vec!["Lightning Bolt"], "a game");
        assert_eq!(printed_hits("in:rare"), vec!["Lotus Cobra"], "a Secret Lair rare doesn't count, as on Scryfall");
        assert_eq!(printed_hits("in:common -r:common"), vec!["Lightning Bolt"], "the Secret Lair printing");
    }

    #[test]
    fn unknown_sets_are_errors_with_suggestions() {
        let resolve = |q: &str| Query::parse(q).and_then(|mut q| q.resolve_sets(&set_list()));
        let e = resolve("s:zzz").unwrap_err().to_string();
        assert!(e.contains("no set 'zzz'"), "{e}");
        let e = resolve("s:z").unwrap_err().to_string();
        assert!(e.contains("did you mean zen"), "a prefix should suggest sets: {e}");
        assert!(resolve("st:nonsense").is_err());
        assert!(resolve("date>nope").is_err());
        let e = resolve("in:ru").unwrap_err().to_string();
        assert!(e.contains("English"), "{e}");
        assert!(Query::parse("r:shiny").is_err() && Query::parse("date>2015-13-01").is_err());
        let no_relations: Vec<SetInfo> =
            set_list().into_iter().map(|s| SetInfo { block: None, parent: None, ..s }).collect();
        let e = Query::parse("b:zen").unwrap().resolve_sets(&no_relations).unwrap_err().to_string();
        assert!(e.contains("sync"), "without Scryfall's set list, b: says how to get it: {e}");
        assert!(Query::parse("s:lea").unwrap().needs_sets() && !Query::parse("in:arena r:rare").unwrap().needs_sets());
    }
}
