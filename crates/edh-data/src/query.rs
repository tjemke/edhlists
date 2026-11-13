//! Scryfall-style card search (`t:equipment`, `o:"create a treasure"`, `otag:ramp`, ...), evaluated
//! locally against the card database: no requests, works offline.
//!
//! A subset of Scryfall's syntax (<https://scryfall.com/docs/syntax>):
//!
//! | Key | Matches |
//! |---|---|
//! | `t:` `type:` | the type line (either face) contains the text; partial words are fine |
//! | `o:` `oracle:` | the rules text without reminder text contains the text; `~` is the card's own name |
//! | `fo:` `fulloracle:` | the same, reminder text included |
//! | `name:`, a bare word or `"phrase"` | the name contains the text; `!"Exact Name"` for one card |
//! | `kw:` `keyword:` | has this keyword ability |
//! | `mv` `cmc`, `usd` | numbers: `mv<=2`, `usd>10` (`:` means `=`) |
//! | `c` `color`, `id` `identity` | colours: `c:rg` is at least red and green, `id:esper` fits an Esper deck |
//! | `is:` / `not:` | `gamechanger`, `commander`, `reserved`, `historic`, `permanent`, `mdfc`, `dfc` |
//! | `otag:` `function:` | a Scryfall Tagger tag, its sub-tags included ([`Query::resolve_tags`]) |
//!
//! Text values take `"quotes"` for phrases and `/slashes/` for (case-insensitive) regular
//! expressions. Terms side by side must all match; `or` between them, `-` in front to negate,
//! `( )` to group. Anything else (sets, rarity, prices other than `usd`, ...) is an error, never
//! silently ignored: the database keeps one printing per card.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use regex::Regex;

use crate::cards::{Card, search_key};
use crate::tags::{TagIndex, tag_slug};
use crate::text::{fold, front_face, strip_reminder};

const SUPPORTED: &str = "t: o: fo: name: kw: mv cmc usd c id is: not: otag:";

/// A parsed search. [`Query::matches`] tests one card.
#[derive(Clone, Debug)]
pub struct Query {
    source: String,
    expr: Expr,
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
    /// WUBRG bits.
    Set(u8),
    Colorless,
    Multicolor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Is {
    GameChanger,
    Commander,
    Reserved,
    Historic,
    Permanent,
    Mdfc,
    Dfc,
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
    Usd(Cmp, f64),
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
        Ok(Self { source: s.trim().to_string(), expr })
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

    pub fn matches(&self, card: &Card) -> bool {
        self.expr.matches(card)
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

    fn matches(&self, card: &Card) -> bool {
        match self {
            Expr::And(v) => v.iter().all(|e| e.matches(card)),
            Expr::Or(v) => v.iter().any(|e| e.matches(card)),
            Expr::Not(e) => !e.matches(card),
            Expr::Term(t) => t.matches(card),
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

    /// Colour sets as bitmasks: `<=` is "within", `>=` is "includes".
    fn set(self, card: u8, q: u8) -> bool {
        let within = card & !q == 0;
        let includes = q & !card == 0;
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

const WUBRG: [&str; 5] = ["W", "U", "B", "R", "G"];

fn mask(colors: &[String]) -> u8 {
    WUBRG.iter().enumerate().filter(|(_, c)| colors.iter().any(|x| x == *c)).fold(0, |m, (i, _)| m | 1 << i)
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

impl Term {
    fn matches(&self, card: &Card) -> bool {
        match self {
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
            // Cards without a price never match a price comparison (as on Scryfall).
            Term::Usd(cmp, x) => card.usd > 0.0 && cmp.num(card.usd, *x),
            Term::Color { identity, cmp, colors } => {
                let have = if *identity { mask(&card.identity) } else { mask(card.colors.as_deref().unwrap_or(&[])) };
                match colors {
                    Colors::Set(q) => cmp.set(have, *q),
                    Colors::Colorless => cmp.set(have, 0),
                    Colors::Multicolor => (have.count_ones() >= 2) == (*cmp != Cmp::Ne),
                }
            }
            Term::Is(is) => match is {
                Is::GameChanger => card.game_changer,
                Is::Commander => card.can_be_commander(),
                Is::Reserved => card.reserved,
                Is::Historic => ["Legendary", "Artifact", "Saga"].iter().any(|t| card.type_line.contains(t)),
                Is::Permanent => ["Artifact", "Creature", "Enchantment", "Land", "Planeswalker", "Battle"]
                    .iter()
                    .any(|t| card.front_type().contains(t)),
                Is::Mdfc => card.layout == "modal_dfc",
                Is::Dfc => matches!(card.layout.as_str(), "transform" | "modal_dfc" | "meld"),
            },
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
    let set = |s: &str| Colors::Set(s.chars().fold(0, |m, c| m | 1 << "wubrg".find(c).expect("colour letter")));
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
        _ => return None,
    })
}

fn is_kind(value: &str) -> Result<Is> {
    Ok(match value.to_lowercase().as_str() {
        "gamechanger" | "game-changer" => Is::GameChanger,
        "commander" => Is::Commander,
        "reserved" => Is::Reserved,
        "historic" => Is::Historic,
        "permanent" => Is::Permanent,
        "mdfc" => Is::Mdfc,
        "dfc" | "doublefaced" => Is::Dfc,
        other => bail!(
            "'is:{other}' isn't supported here (use gamechanger, commander, reserved, historic, permanent, mdfc or dfc)"
        ),
    })
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
        "mv" | "cmc" | "manavalue" => Term::Mv(cmp(op), number(val)?),
        "usd" => Term::Usd(cmp(op), number(val)?),
        "c" | "color" | "colour" | "id" | "identity" | "ci" => {
            let identity = matches!(key, "id" | "identity" | "ci");
            let v = plain(val, key)?;
            let colors = colors(&v)
                .ok_or_else(|| anyhow!("'{v}' isn't a colour (use letters like rg, or names like azorius)"))?;
            // `c:rg` means "at least red and green", `id:rg` "fits a red-green deck"; `c:c` and
            // `c:m` mean colourless and multicolour outright.
            let cmp = match (op, colors) {
                (Op::Colon, Colors::Colorless | Colors::Multicolor) => Cmp::Eq,
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
            let t = Expr::Term(Term::Is(is_kind(&plain(val, key)?)?));
            return Ok(if key == "not" { Expr::Not(Box::new(t)) } else { t });
        }
        "otag" | "oracletag" | "function" => {
            text_key()?;
            Term::Tag { slug: tag_slug(&plain(val, key)?), ids: None }
        }
        other => bail!("'{other}' isn't a search key this supports (use {SUPPORTED})"),
    };
    Ok(Expr::Term(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, type_line: &str, oracle: &str) -> Card {
        Card {
            name: name.into(),
            oracle_id: name.to_lowercase(),
            type_line: type_line.into(),
            oracle: vec![oracle.into()],
            layout: "normal".into(),
            ..Default::default()
        }
    }

    fn cards() -> Vec<Card> {
        let mut bonesplitter = card("Bonesplitter", "Artifact — Equipment", "Equipped creature gets +2/+0.\nEquip {1}");
        bonesplitter.keywords = vec!["Equip".into()];
        bonesplitter.mv = 1.0;
        bonesplitter.colors = Some(vec![]);
        bonesplitter.usd = 0.5;
        let mut lizard = card(
            "Lizard Blades",
            "Artifact Creature — Equipment Lizard",
            "Double strike\nEquipped creature has double strike.\nReconfigure {2} ({2}: Attach to target creature you control; or unattach from a creature. Reconfigure only as a sorcery. While attached, this isn't a creature.)",
        );
        lizard.keywords = vec!["Double strike".into(), "Reconfigure".into()];
        lizard.mv = 2.0;
        lizard.colors = Some(vec!["R".into()]);
        lizard.identity = vec!["R".into()];
        let mut saga = card(
            "History of Benalia",
            "Enchantment — Saga",
            "(As this Saga enters and after your draw step, add a lore counter.)\nI, II — Create a 2/2 white Knight creature token with vigilance.",
        );
        saga.mv = 3.0;
        saga.colors = Some(vec!["W".into()]);
        saga.identity = vec!["W".into()];
        let mut krenko = card(
            "Krenko, Mob Boss",
            "Legendary Creature — Goblin Warrior",
            "{T}: Create X 1/1 red Goblin creature tokens, where X is the number of Goblins you control.",
        );
        krenko.mv = 4.0;
        krenko.colors = Some(vec!["R".into()]);
        krenko.identity = vec!["R".into()];
        let mut bolt = card("Lightning Bolt", "Instant", "Lightning Bolt deals 3 damage to any target.");
        bolt.mv = 1.0;
        bolt.colors = Some(vec!["R".into()]);
        bolt.identity = vec!["R".into()];
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
        mdfc.layout = "modal_dfc".into();
        mdfc.mv = 3.0;
        mdfc.colors = Some(vec!["R".into()]);
        mdfc.identity = vec!["R".into()];
        let mut rings = card(
            "Boros Charm",
            "Instant",
            "Choose one —\n• Boros Charm deals 4 damage to target player or planeswalker.",
        );
        rings.colors = Some(vec!["R".into(), "W".into()]);
        rings.identity = vec!["R".into(), "W".into()];
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
            "set:neo",
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
        let e = Query::parse("rarity:mythic").unwrap_err().to_string();
        assert!(e.contains("rarity") && e.contains("t: o:"), "{e}");
    }
}
