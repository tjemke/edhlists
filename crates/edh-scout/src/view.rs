//! Deck presentation shared by the TUI and the plain output: styled detail lines and grouped lists.

use std::collections::BTreeMap;

use edh_data::cards::CardDb;
use edh_data::deck::Deck;
use edh_data::http::host_of;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::analysis::Analysis;
use crate::rank::{Ranked, Target};
use crate::roles::{LAND_RANGE, Role, target as role_target};

pub const GOOD: Color = Color::Green;
pub const OK: Color = Color::Yellow;
pub const BAD: Color = Color::Red;
pub const DIM: Color = Color::DarkGray;
pub const ACCENT: Color = Color::Cyan;

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

fn label(s: &str) -> Span<'static> {
    Span::styled(format!("{s:<11}"), Style::new().fg(DIM))
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

    // Power and bracket
    let pc = power_color(target, a.power.level);
    let mut l = vec![label("Power"), Span::styled(format!("{:<5}", a.power.label()), Style::new().fg(pc).bold())];
    l.extend(power_bar(target, a.power.level, 30));
    out.push(Line::from(l));
    let mut b = vec![
        label("Bracket"),
        Span::styled(
            format!("B{}", a.bracket.bracket),
            Style::new().bold().fg(if ranked.is_none_or(|r| r.fits) { GOOD } else { BAD }),
        ),
        Span::styled(
            format!("   power alone B{} · rules floor B{}", a.bracket.from_level, a.bracket.floor),
            Style::new().fg(DIM),
        ),
    ];
    if let Some(declared) = deck.meta.declared_bracket {
        b.push(Span::styled(format!(" · owner says B{declared}"), Style::new().fg(DIM)));
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
    for role in Role::ALL {
        let (have, want) = (s.count(role), role_target(role));
        let mut l = vec![label(&format!("  {}", role.short_label())), Span::raw(format!("{have:>3} "))];
        l.extend(meter(have, want));
        out.push(Line::from(l));
    }
    out.push(Line::default());

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
            let mut l = vec![label(""), Span::raw(c.cards.join(" + "))];
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
        let t = cards.get(name).map(|c| c.primary_type()).unwrap_or("Other");
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
