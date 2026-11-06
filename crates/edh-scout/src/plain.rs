//! Non-interactive output: a ranked table for `scout --plain` (or when stdout isn't a terminal).

use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::{Attribute, Cell, CellAlignment, Color, ContentArrangement, Table};

use crate::analysis::Analysis;
use crate::rank::{RankMode, Ranked, Target};
use crate::view;
use edh_data::deck::Deck;

fn tc(c: ratatui::style::Color) -> Color {
    match c {
        ratatui::style::Color::Green => Color::Green,
        ratatui::style::Color::Yellow => Color::Yellow,
        ratatui::style::Color::Red => Color::Red,
        ratatui::style::Color::Cyan => Color::Cyan,
        _ => Color::DarkGrey,
    }
}

pub fn ranked_table(decks: &[(Deck, Analysis)], ranking: &[Ranked], target: &Target, top: usize) -> Table {
    let mut t = Table::new();
    t.load_style(UTF8_FULL_CONDENSED.with_rounded_corners())
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(
            ["#", "Power", "Bracket", "Salt", "Structure", "Synergy", "Price", "Themes", "EDHREC"]
                .map(|h| Cell::new(h).add_attribute(Attribute::Bold)),
        );
    for (pos, r) in ranking.iter().take(top).enumerate() {
        let (deck, a) = &decks[r.index];
        let dim = |c: Cell| if r.fits { c } else { c.fg(Color::DarkGrey) };
        let b = format!("B{}", a.bracket.bracket);
        t.add_row(vec![
            dim(Cell::new(pos + 1)),
            Cell::new(a.power.label()).fg(tc(view::power_color(target, a.power.level))).add_attribute(Attribute::Bold),
            if r.fits { Cell::new(b).fg(Color::Green) } else { Cell::new(b).fg(Color::Red) },
            Cell::new(a.salt.map(|s| format!("{s:.0}")).unwrap_or("–".into())).fg(tc(view::score_color(r.scores.salt))),
            Cell::new(format!("{:.0}%", a.structure.score * 100.0)).fg(tc(view::score_color(a.structure.score))),
            Cell::new(a.synergy.as_ref().map(|s| format!("{:.1}%", s.average * 100.0)).unwrap_or("–".into()))
                .fg(tc(view::score_color(r.scores.synergy))),
            dim(Cell::new(deck.meta.price.map(|p| format!("${p:.0}")).unwrap_or("–".into()))),
            dim(Cell::new(if deck.meta.tags.is_empty() { "–".into() } else { deck.meta.tags.join(", ") })),
            dim(Cell::new(deck.meta.edhrec_url.clone().unwrap_or_default())),
        ]);
    }
    for i in [0usize, 1, 3, 4, 5, 6] {
        if let Some(col) = t.column_mut(i) {
            col.set_cell_alignment(CellAlignment::Right);
        }
    }
    t
}

pub fn legend(target: &Target, mode: RankMode) -> String {
    format!(
        "Ranked by {} · target {}. Power and bracket follow EDHPowerLevel; grey rows are outside the target. \
         Salt: EDHREC deck salt (lower is friendlier). Structure: closeness to a 36 land / 10 ramp / 10 draw / \
         8 removal / 2 wipe template. Synergy: average EDHREC synergy with the commander.",
        mode.label(),
        target.describe()
    )
}
