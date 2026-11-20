//! Deck presentation shared by the TUI and the plain output: styled detail lines and grouped lists.

use std::collections::BTreeMap;

use edh_data::cards::CardDb;
use edh_data::deck::{Bracket, Deck};
use edh_data::flags::Flag;
use edh_data::http::host_of;
use edh_data::mana::{Color as Mtg, ColorSet};
use edh_data::text::front_face;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use edh_goldfish::ManaReport;

use crate::analysis::Analysis;
use crate::filters::{Matched, short_label};
use crate::rank::{Ranked, Target};
use crate::roles::{LAND_RANGE, Role, target as role_target};

pub const GOOD: Color = Color::Green;
pub const OK: Color = Color::Yellow;
pub const BAD: Color = Color::Red;
pub const DIM: Color = Color::DarkGray;
pub const ACCENT: Color = Color::Cyan;

/// The colours that tell `-T` filters apart, by the filter's position (the first is [`GOOD`]).
const COUNTED: [Color; 6] =
    [Color::Green, Color::Yellow, Color::Magenta, Color::LightBlue, Color::LightRed, Color::LightCyan];

/// The `i`th `-T` filter's colour, for its label, its column and the marks on the cards it counted.
/// They repeat after six.
pub fn counted_color(i: usize) -> Color {
    COUNTED[i % COUNTED.len()]
}

/// A deck's colour identity: the union of its commanders' identities, in WUBRG order. Commanders
/// missing from the card database add nothing.
pub fn deck_identity(deck: &Deck, cards: &CardDb) -> ColorSet {
    deck.commanders.iter().filter_map(|c| cards.get(c)).fold(ColorSet::empty(), |id, card| id | card.identity)
}

/// Colour identity as coloured WUBRG letters (C for colourless).
pub fn identity_line(identity: ColorSet) -> Line<'static> {
    let spans: Vec<Span> = identity
        .iter()
        .map(|c| {
            let color = match c {
                Mtg::W => Color::Yellow,
                Mtg::U => Color::Blue,
                Mtg::B => Color::Magenta,
                Mtg::R => Color::Red,
                Mtg::G => Color::Green,
            };
            Span::styled(c.to_string(), Style::new().fg(color).add_modifier(Modifier::BOLD))
        })
        .collect();
    if spans.is_empty() { Line::from(Span::styled("C", Style::new().fg(DIM))) } else { Line::from(spans) }
}

/// Colour for a power level relative to the target band.
pub fn power_color(target: &Target, level: f64) -> Color {
    let (lo, hi) = target.band;
    if level > lo && level <= hi {
        GOOD
    } else if level > lo - 0.5 && level <= hi + 0.5 {
        OK
    } else {
        BAD
    }
}

/// Colour for a 0-1 "higher is better" score.
pub fn score_color(x: f64) -> Color {
    if x >= 0.66 {
        GOOD
    } else if x >= 0.33 {
        OK
    } else {
        BAD
    }
}

/// Width of the field-label column; values start right after it.
const LABEL_WIDTH: usize = 11;
/// Marks the start of each entry in a multi-entry field, so wrapped entries stay apart.
const BULLET: &str = "• ";

fn label(s: &str) -> Span<'static> {
    Span::styled(format!("{s:<LABEL_WIDTH$}"), Style::new().fg(DIM))
}

/// A 0-10 power bar, with the target band highlighted.
fn power_bar(target: &Target, level: f64, width: usize) -> Vec<Span<'static>> {
    let (lo, hi) = (target.band.0, target.band.1.min(10.0));
    (0..width)
        .map(|i| {
            let x = (i as f64 + 0.5) * 10.0 / width as f64;
            let filled = x <= level;
            let in_band = x > lo && x <= hi;
            let ch = if filled { "█" } else { "░" };
            let color = match (filled, in_band) {
                (true, true) => GOOD,
                (true, false) => OK,
                (false, true) => Color::Green,
                (false, false) => DIM,
            };
            let style = if !filled && in_band {
                Style::new().fg(color).add_modifier(Modifier::DIM)
            } else {
                Style::new().fg(color)
            };
            Span::styled(ch, style)
        })
        .collect()
}

fn meter(have: u32, want: u32) -> Vec<Span<'static>> {
    let n = want.max(1) as usize;
    let filled = (have as usize).min(n);
    let color = if have >= want {
        GOOD
    } else if have * 10 >= want * 7 {
        OK
    } else {
        BAD
    };
    vec![
        Span::styled("▰".repeat(filled), Style::new().fg(color)),
        Span::styled("▱".repeat(n - filled), Style::new().fg(DIM)),
    ]
}

fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

/// The detail pane for one deck.
pub fn detail_lines(
    deck: &Deck,
    a: &Analysis,
    target: &Target,
    ranked: Option<&Ranked>,
    show_price: bool,
    mana: ManaView,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let themes = if deck.meta.tags.is_empty() { "no themes tagged".to_string() } else { deck.meta.tags.join(" · ") };
    out.push(Line::from(vec![Span::styled(themes, Style::new().bold())]));
    let mut sub = vec![Span::styled(deck.commanders.join(" + "), Style::new().fg(ACCENT))];
    if let Some(d) = &deck.meta.savedate {
        sub.push(Span::styled(format!("  saved {d}"), Style::new().fg(DIM)));
    }
    if let Some(u) = &deck.meta.source_url {
        sub.push(Span::styled(format!("  on {}", host_of(u)), Style::new().fg(DIM)));
    }
    out.push(Line::from(sub));
    out.push(Line::default());

    // What the -T card filters counted
    if !a.matches.is_empty() {
        out.extend(a.matches.iter().enumerate().map(|(i, m)| matched_line(m, i)));
        out.push(Line::default());
    }

    // Power and bracket
    let pc = power_color(target, a.power.level);
    let mut l = vec![label("Power"), Span::styled(format!("{:<5}", a.power.label()), Style::new().fg(pc).bold())];
    l.extend(power_bar(target, a.power.level, 30));
    if let Some(r) = ranked {
        l.push(Span::styled("   fit ", Style::new().fg(DIM)));
        l.push(Span::styled(pct(r.scores.fit), Style::new().fg(score_color(r.scores.fit))));
    }
    out.push(Line::from(l));
    let mut b = vec![
        label("Bracket"),
        Span::styled(
            Bracket(a.bracket.bracket).to_string(),
            Style::new().bold().fg(if ranked.is_none_or(|r| r.fits) { GOOD } else { BAD }),
        ),
        Span::styled(
            format!("   power alone {} · rules floor {}", Bracket(a.bracket.from_level), Bracket(a.bracket.floor)),
            Style::new().fg(DIM),
        ),
    ];
    if let Some(declared) = deck.meta.declared_bracket {
        b.push(Span::styled(format!(" · owner says {declared}"), Style::new().fg(DIM)));
    }
    out.push(Line::from(b));
    let reasons = a.bracket.reasons();
    if !reasons.is_empty() {
        out.push(Line::from(vec![
            label(""),
            Span::styled(format!("floor from {}", reasons.join(", ")), Style::new().fg(OK)),
        ]));
    }
    out.push(Line::from(vec![
        label(""),
        Span::styled(
            format!(
                "impact {:.0} × efficiency {:.2} · avg MV {:.2} · {} lands",
                a.power.impact, a.power.efficiency, a.power.avg_cmc, a.power.lands
            ),
            Style::new().fg(DIM),
        ),
    ]));

    // Salt
    let mut salt = vec![label("Salt")];
    match a.salt {
        Some(s) => {
            let color = ranked.map(|r| score_color(r.scores.salt)).unwrap_or(Color::Reset);
            salt.push(Span::styled(format!("{s:.0}"), Style::new().fg(color).bold()));
            if let Some(r) = ranked {
                salt.push(Span::styled(
                    format!("   friendlier than {} of this pool", pct(r.scores.salt)),
                    Style::new().fg(DIM),
                ));
            }
        }
        None => salt.push(Span::styled("unknown", Style::new().fg(DIM))),
    }
    out.push(Line::from(salt));
    if !a.salty_cards.is_empty() {
        let names: Vec<String> = a.salty_cards.iter().take(6).map(|(n, s)| format!("{n} {s:.1}")).collect();
        out.push(Line::from(vec![label(""), Span::raw(names.join(", "))]));
    }
    out.push(Line::default());

    // Structure
    let s = &a.structure;
    out.push(Line::from(vec![
        label("Structure"),
        Span::styled(pct(s.score), Style::new().bold().fg(score_color(s.score))),
        Span::styled("   vs. 36 lands / 10 ramp / 10 draw / 8 removal / 2 wipes", Style::new().fg(DIM)),
    ]));
    let lands_ok = (LAND_RANGE.0..=LAND_RANGE.1).contains(&s.lands);
    out.push(Line::from(vec![
        label("  Lands"),
        Span::styled(format!("{:>3}", s.lands), Style::new().fg(if lands_ok { GOOD } else { OK })),
    ]));
    for &role in Role::ALL {
        let (have, want) = (s.count(role), role_target(role));
        let mut l = vec![label(&format!("  {}", role.short_label())), Span::raw(format!("{have:>3} "))];
        l.extend(meter(have, want));
        out.push(Line::from(l));
    }
    out.push(Line::default());

    let mana = mana_lines(mana);
    if !mana.is_empty() {
        out.extend(mana);
        out.push(Line::default());
    }

    // Synergy
    if let Some(syn) = &a.synergy {
        let mut l = vec![label("Synergy"), Span::styled(format!("{:.1}%", syn.average * 100.0), Style::new().bold())];
        if let Some(r) = ranked {
            l.push(Span::styled(
                format!("   more on-plan than {} of this pool", pct(r.scores.synergy)),
                Style::new().fg(DIM),
            ));
        }
        out.push(Line::from(l));
        if !syn.signature.is_empty() {
            let names: Vec<String> =
                syn.signature.iter().take(6).map(|(n, v)| format!("{n} +{:.0}%", v * 100.0)).collect();
            out.push(Line::from(vec![label(""), Span::raw(names.join(", "))]));
        }
    }

    if !a.bracket.game_changers.is_empty() {
        out.push(Line::from(vec![
            label("Game Chg."),
            Span::styled(a.bracket.game_changers.join(", "), Style::new().fg(OK)),
        ]));
    }
    if !a.combos.is_empty() {
        out.push(Line::from(vec![label("Combos"), Span::raw(format!("{} known", a.combos.len()))]));
        for c in a.combos.iter().take(6) {
            let mut l = vec![label(""), Span::styled(BULLET, Style::new().fg(DIM)), Span::raw(c.cards.join(" + "))];
            l.push(Span::styled(format!(" → {}", c.results.join("; ")), Style::new().fg(DIM)));
            let tag = match &c.vote {
                Some(v) => format!(" [{} · voted B{v}]", c.tag),
                None => format!(" [{}]", c.tag),
            };
            l.push(Span::styled(tag, Style::new().fg(OK)));
            out.push(Line::from(l));
        }
    }
    out.push(Line::default());
    if show_price {
        let price = deck.meta.price.map(|p| format!("${p:.0}")).unwrap_or_else(|| "–".into());
        out.push(Line::from(vec![label("Price"), Span::raw(format!("{price} · {} cards", a.card_count))]));
    } else {
        out.push(Line::from(vec![label("Cards"), Span::raw(a.card_count.to_string())]));
    }
    if !a.unknown.is_empty() {
        out.push(Line::from(vec![
            label("Unknown"),
            Span::styled(format!("{} (not scored)", a.unknown.join(", ")), Style::new().fg(BAD)),
        ]));
    }
    out
}

/// One `-T` card filter's count and the cards that make it up; a long filter is shown under its
/// short label (`Q1`), in the filter's colour, and spelled out before the cards. The count is green
/// within the filter's range, red (with the range) outside it, uncoloured without one.
/// The mana goldfish's part of the details.
pub enum ManaView<'a> {
    /// Turned off (`[goldfish] games = 0`).
    Off,
    /// Still playing this many games.
    Pending(u32),
    Ready(&'a ManaReport),
}

/// A share as a percentage: a decimal below 10% ("2.1%"), none above ("80%").
fn share(x: f64) -> String {
    if x < 0.1 { format!("{:.1}%", x * 100.0) } else { format!("{:.0}%", x * 100.0) }
}

/// What the mana goldfish found, in words ("commander by T4 80% · 5.3 mana on T4, +1.3 over a
/// land a turn · colours held it up 2%"), and the cards most often stuck on colours, if any were
/// more than now and then ([`edh_goldfish::STUCK_WORTH_SHOWING`]). Shared with the report.
pub fn mana_summary(m: &ManaReport) -> (String, Vec<String>) {
    let mut parts = Vec::new();
    if let Some(t4) = m.commander_by_t4 {
        parts.push(format!("commander by T4 {}", share(t4)));
    }
    parts.push(format!("{:.1} mana on T4, {:+.1} over a land a turn", m.capacity[3], m.ahead_on_t4()));
    if let Some(d) = m.colour_delayed.filter(|d| *d > edh_goldfish::STUCK_WORTH_SHOWING) {
        parts.push(format!("colours held it up {}", share(d)));
    }
    let stuck = m.stuck_often().take(4).map(|(n, s)| format!("{} {}", front_face(n), share(*s))).collect();
    (parts.join(" · "), stuck)
}

fn mana_lines(mana: ManaView) -> Vec<Line<'static>> {
    match mana {
        ManaView::Off => vec![],
        ManaView::Pending(games) => {
            vec![Line::from(vec![label("Mana"), Span::styled(format!("playing {games} games…"), Style::new().fg(DIM))])]
        }
        ManaView::Ready(m) => {
            let (summary, stuck) = mana_summary(m);
            let mut out = vec![Line::from(vec![label("Mana"), Span::raw(summary)])];
            if !stuck.is_empty() {
                out.push(Line::from(vec![
                    label(""),
                    Span::styled("stuck on colours: ", Style::new().fg(DIM)),
                    Span::styled(stuck.join(", "), Style::new().fg(OK)),
                ]));
            }
            out
        }
    }
}

pub fn matched_line(m: &Matched, i: usize) -> Line<'static> {
    let short = short_label(&m.label, i);
    let color = match (&m.wanted, m.passes) {
        (None, _) => Color::Reset,
        (Some(_), true) => GOOD,
        (Some(_), false) => BAD,
    };
    let mut l = vec![
        Span::styled(format!("{short:<LABEL_WIDTH$}"), Style::new().fg(counted_color(i))),
        Span::styled(format!("{:<4}", m.count), Style::new().bold().fg(color)),
    ];
    if let (Some(wanted), false) = (&m.wanted, m.passes) {
        l.push(Span::styled(format!("wants {wanted}  "), Style::new().fg(BAD)));
    }
    if short != m.label {
        l.push(Span::styled(format!("{}  ", m.label), Style::new().fg(DIM)));
    }
    let names: Vec<String> = m
        .cards
        .iter()
        .map(|(n, q)| if *q > 1 { format!("{q} {}", front_face(n)) } else { front_face(n).to_string() })
        .collect();
    // Legendary names have commas of their own, so entries are set apart with dots.
    l.push(Span::raw(names.join(" · ")));
    Line::from(l)
}

/// Word-wrap `lines` to `width` columns. Continuation lines of a labelled field start in the value
/// column (past a leading bullet, if any) rather than back under the label.
pub fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines.into_iter().flat_map(|l| wrap_line(l, width)).collect()
}

fn hanging_indent(line: &Line) -> usize {
    let mut spans = line.spans.iter();
    let Some(first) = spans.next() else { return 0 };
    if first.width() != LABEL_WIDTH || !first.content.ends_with(' ') {
        return 0;
    }
    let bullet = spans.next().filter(|s| s.content.starts_with(BULLET)).map_or(0, |_| Span::raw(BULLET).width());
    LABEL_WIDTH + bullet
}

fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 || line.width() <= width {
        return vec![line];
    }
    // Keep at least half the width for text; on very narrow panes, fall back to no indent.
    let indent = Some(hanging_indent(&line)).filter(|i| i * 2 <= width).unwrap_or(0);
    let mut w = Wrapper { width, indent, style: line.style, out: Vec::new(), cur: Vec::new(), col: 0, start: 0 };
    for span in &line.spans {
        // Feed alternating runs of whitespace and non-whitespace.
        let mut tok = String::new();
        for ch in span.content.chars() {
            if !tok.is_empty() && tok.ends_with(char::is_whitespace) != ch.is_whitespace() {
                w.token(&std::mem::take(&mut tok), span.style);
            }
            tok.push(ch);
        }
        if !tok.is_empty() {
            w.token(&tok, span.style);
        }
    }
    w.out.push(Line::from(w.cur).style(w.style));
    w.out
}

struct Wrapper {
    width: usize,
    indent: usize,
    style: Style,
    out: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    /// Current column, and the column where text on the current line begins.
    col: usize,
    start: usize,
}

impl Wrapper {
    fn token(&mut self, text: &str, style: Style) {
        let w = Span::raw(text).width();
        if text.starts_with(char::is_whitespace) {
            if self.col + w > self.width {
                self.newline();
            } else if self.col > self.start || self.out.is_empty() {
                self.push(text, style, w); // no leading spaces on continuation lines
            }
            return;
        }
        if self.col + w > self.width && self.col > self.start {
            self.newline();
        }
        if self.col + w <= self.width {
            self.push(text, style, w);
            return;
        }
        // A word longer than a whole line: split it wherever it runs out of room.
        for ch in text.chars() {
            let mut buf = [0; 4];
            let ch = &*ch.encode_utf8(&mut buf);
            let cw = Span::raw(ch).width();
            if self.col + cw > self.width && self.col > self.start {
                self.newline();
            }
            self.push(ch, style, cw);
        }
    }

    fn push(&mut self, text: &str, style: Style, w: usize) {
        match self.cur.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(text),
            _ => self.cur.push(Span::styled(text.to_string(), style)),
        }
        self.col += w;
    }

    fn newline(&mut self) {
        if let Some(last) = self.cur.last_mut() {
            let trimmed = last.content.trim_end().to_string();
            last.content = trimmed.into();
        }
        self.out.push(Line::from(std::mem::take(&mut self.cur)).style(self.style));
        if self.indent > 0 {
            self.cur.push(Span::raw(" ".repeat(self.indent)));
        }
        (self.col, self.start) = (self.indent, self.indent);
    }
}

/// The deck grouped by primary card type, commanders first.
pub fn grouped_list(deck: &Deck, cards: &CardDb) -> Vec<(String, Vec<(u32, String)>)> {
    const ORDER: [&str; 10] = [
        "Commander",
        "Creature",
        "Planeswalker",
        "Battle",
        "Instant",
        "Sorcery",
        "Artifact",
        "Enchantment",
        "Land",
        "Other",
    ];
    let mut groups: BTreeMap<usize, Vec<(u32, String)>> = BTreeMap::new();
    for c in &deck.commanders {
        groups.entry(0).or_default().push((1, c.clone()));
    }
    for (name, qty) in &deck.cards {
        let t = cards.get(name).and_then(|c| c.primary_type()).map_or("Other", |t| t.code());
        let idx = ORDER.iter().position(|o| *o == t).unwrap_or(ORDER.len() - 1);
        groups.entry(idx).or_default().push((*qty, name.clone()));
    }
    groups.into_iter().map(|(i, v)| (ORDER[i].to_string(), v)).collect()
}

/// Render styled lines as ANSI text (or plain text when `color` is false).
pub fn to_ansi(lines: &[Line], color: bool) -> String {
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            if !color || span.style == Style::default() {
                out.push_str(&span.content);
                continue;
            }
            let mut codes: Vec<String> = Vec::new();
            if span.style.add_modifier.contains(Modifier::BOLD) {
                codes.push("1".into());
            }
            if span.style.add_modifier.contains(Modifier::DIM) {
                codes.push("2".into());
            }
            if let Some(fg) = span.style.fg {
                let c = match fg {
                    Color::Red => "31",
                    Color::Green => "32",
                    Color::Yellow => "33",
                    Color::Blue => "34",
                    Color::Magenta => "35",
                    Color::Cyan => "36",
                    Color::Gray => "37",
                    Color::DarkGray => "90",
                    Color::LightRed => "91",
                    Color::LightGreen => "92",
                    Color::LightYellow => "93",
                    Color::LightBlue => "94",
                    Color::LightMagenta => "95",
                    Color::LightCyan => "96",
                    _ => "",
                };
                if !c.is_empty() {
                    codes.push(c.into());
                }
            }
            if codes.is_empty() {
                out.push_str(&span.content);
            } else {
                out.push_str(&format!("\x1b[{}m{}\x1b[0m", codes.join(";"), span.content));
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn mana_mentions_cards_stuck_on_colours_only_when_it_happens_more_than_now_and_then() {
        let report = ManaReport {
            games: 5000,
            mulligan: 0.3,
            commander_by_t4: Some(0.8),
            colour_delayed: Some(0.004),
            capacity: [1.0, 2.1, 3.2, 5.3, 6.0, 7.0, 8.0, 9.0],
            spent_by_t8: 30.0,
            stuck: vec![("Reanimate".into(), 0.032), ("Entomb".into(), 0.006), ("Careful Study".into(), 0.005)],
        };
        let (summary, stuck) = mana_summary(&report);
        assert_eq!(summary, "commander by T4 80% · 5.3 mana on T4, +1.3 over a land a turn");
        assert_eq!(stuck, ["Reanimate 3.2%", "Entomb 0.6%"], "0.5% and less isn't worth a mention");
        let held_up = ManaReport { colour_delayed: Some(0.15), stuck: vec![("Careful Study".into(), 0.005)], ..report };
        let (summary, stuck) = mana_summary(&held_up);
        assert!(summary.ends_with("· colours held it up 15%"), "{summary}");
        assert!(stuck.is_empty());
        assert!(text(&mana_lines(ManaView::Ready(&held_up))).len() == 1, "no stuck line without stuck cards");
        assert!(mana_lines(ManaView::Off).is_empty());
    }

    #[test]
    fn wrapped_fields_continue_in_the_value_column() {
        let line =
            Line::from(vec![label("Synergy"), Span::raw("Brainstorm +51%, Proteus Staff +51%, Serra's Emissary +41%")]);
        assert_eq!(
            text(&wrap_lines(vec![line], 40)),
            ["Synergy    Brainstorm +51%, Proteus", "           Staff +51%, Serra's Emissary", "           +41%"]
        );
    }

    #[test]
    fn wrapped_bullets_continue_past_the_bullet() {
        let line = Line::from(vec![
            label(""),
            Span::styled(BULLET, Style::new().fg(DIM)),
            Span::raw("Jace, Cunning Castaway + Ichormoon Gauntlet"),
            Span::styled(" → Infinite creature tokens", Style::new().fg(DIM)),
        ]);
        assert_eq!(
            text(&wrap_lines(vec![line], 40)),
            [
                "           • Jace, Cunning Castaway +",
                "             Ichormoon Gauntlet →",
                "             Infinite creature tokens",
            ]
        );
    }

    #[test]
    fn deck_identity_is_the_commanders_union_in_wubrg_order() {
        let card = |name: &str, id: &str| edh_data::cards::Card {
            name: name.into(),
            identity: ColorSet::from_letters(id).unwrap(),
            ..Default::default()
        };
        let cards = CardDb::new(vec![card("Green One", "G"), card("Esper One", "UWB"), card("Rock", "")]);
        let deck = |cs: &[&str]| Deck { commanders: cs.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        assert_eq!(deck_identity(&deck(&["Green One", "Esper One"]), &cards).to_string(), "WUBG");
        assert!(deck_identity(&deck(&["Rock", "Unknown"]), &cards).is_empty());
        assert_eq!(text(&[identity_line(deck_identity(&deck(&["Rock"]), &cards))]), ["C"]);
        assert_eq!(text(&[identity_line(deck_identity(&deck(&["Green One", "Esper One"]), &cards))]), ["WUBG"]);
    }

    #[test]
    fn matched_cards_wrap_under_their_count() {
        let m = Matched {
            label: "equipment".into(),
            count: 3,
            cards: vec![("Bonesplitter".into(), 1), ("Sword of Fire and Ice".into(), 1), ("Lizard Blades".into(), 1)],
            wanted: Some("2+".into()),
            passes: true,
        };
        assert_eq!(
            text(&wrap_lines(vec![matched_line(&m, 0)], 40)),
            ["equipment  3   Bonesplitter · Sword of", "           Fire and Ice · Lizard Blades"]
        );
        let q = Matched {
            label: r#"o:"create a treasure""#.into(),
            count: 12,
            cards: vec![("Mountain".into(), 12)],
            wanted: None,
            passes: true,
        };
        assert_eq!(text(&[matched_line(&q, 1)]), [r#"Q2         12  o:"create a treasure"  12 Mountain"#]);
        let short = Matched { label: "ramp".into(), count: 7, wanted: Some("10+".into()), ..Default::default() };
        assert_eq!(text(&[matched_line(&short, 0)]), ["ramp       7   wants 10+  "]);
    }

    #[test]
    fn unlabelled_and_short_lines_are_left_alone() {
        let short = Line::from(vec![label("Cards"), Span::raw("100")]);
        let header = Line::from("Artifacts · Planeswalkers · Wheels");
        assert_eq!(
            text(&wrap_lines(vec![short, header], 20)),
            ["Cards      100", "Artifacts ·", "Planeswalkers ·", "Wheels"]
        );
    }
}
