//! Non-interactive output: a ranked table for `scout --plain` (or when stdout isn't a terminal).

use std::collections::HashSet;

use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::{Attribute, Cell, CellAlignment, Color, ContentArrangement, Table};

use crate::filters::CountColumn;
use crate::rank::{RankMode, Ranked, Target};
use crate::scout::Entry;
use crate::view;
use edh_data::cards::CardDb;
use edh_data::text::front_face;

fn tc(c: ratatui::style::Color) -> Color {
    match c {
        ratatui::style::Color::Green => Color::Green,
        ratatui::style::Color::Yellow => Color::Yellow,
        ratatui::style::Color::Red => Color::Red,
        ratatui::style::Color::Cyan => Color::Cyan,
        _ => Color::DarkGrey,
    }
}

/// Optional columns.
#[derive(Clone, Debug, Default)]
pub struct Columns {
    pub price: bool,
    pub commander: bool,
    /// The `-T` card filters' count columns.
    pub counts: Vec<CountColumn>,
}

/// The top `top` decks of `ranking`; those whose id is in `favorites` are starred.
pub fn ranked_table(
    decks: &[Entry],
    cards: &CardDb,
    ranking: &[Ranked],
    favorites: &HashSet<String>,
    target: &Target,
    top: usize,
    columns: &Columns,
) -> Table {
    let show_price = columns.price;
    let mut headers: Vec<String> = ["#", "Power", "Bracket", "Salt", "Structure", "Synergy"].map(String::from).to_vec();
    if show_price {
        headers.push("Price".into());
    }
    if columns.commander {
        headers.extend(["Colors".into(), "Commander".into()]);
    }
    let first_count = headers.len();
    headers.extend(columns.counts.iter().map(|c| c.label.clone()));
    headers.extend(["Themes".into(), "EDHREC".into()]);
    let mut t = Table::new();
    t.load_style(UTF8_FULL_CONDENSED.with_rounded_corners())
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(headers.into_iter().map(|h| Cell::new(h).add_attribute(Attribute::Bold)));
    for (pos, r) in ranking.iter().take(top).enumerate() {
        let Entry { deck, score: s, matches } = &decks[r.index];
        let dim = |c: Cell| if r.fits { c } else { c.fg(Color::DarkGrey) };
        let b = s.bracket.to_string();
        let mut row = vec![
            dim(Cell::new(if favorites.contains(&deck.id) {
                format!("★ {}", pos + 1)
            } else {
                (pos + 1).to_string()
            })),
            Cell::new(s.power_label()).fg(tc(view::power_color(target, s.power))).add_attribute(Attribute::Bold),
            if r.fits { Cell::new(b).fg(Color::Green) } else { Cell::new(b).fg(Color::Red) },
            Cell::new(s.salt.map(|s| format!("{s:.0}")).unwrap_or("–".into())).fg(tc(view::score_color(r.scores.salt))),
            Cell::new(format!("{:.0}%", s.structure * 100.0)).fg(tc(view::score_color(s.structure))),
            Cell::new(s.synergy.map(|s| format!("{:.1}%", s * 100.0)).unwrap_or("–".into()))
                .fg(tc(view::score_color(r.scores.synergy))),
        ];
        if show_price {
            row.push(dim(Cell::new(deck.meta.price.map(|p| format!("${p:.0}")).unwrap_or("–".into()))));
        }
        if columns.commander {
            let identity = view::deck_identity(deck, cards);
            row.push(Cell::new(if identity.is_empty() { "C".to_string() } else { identity.to_string() }));
            let names: Vec<&str> = deck.commanders.iter().map(|c| front_face(c)).collect();
            row.push(Cell::new(names.join(" + ")).fg(Color::Cyan));
        }
        for c in &columns.counts {
            row.push(dim(Cell::new(c.cell(matches))));
        }
        row.push(dim(Cell::new(if deck.meta.tags.is_empty() { "–".into() } else { deck.meta.tags.join(", ") })));
        row.push(dim(Cell::new(deck.meta.edhrec_url.clone().unwrap_or_default())));
        t.add_row(row);
    }
    let mut numeric: Vec<usize> = if show_price { vec![0, 1, 3, 4, 5, 6] } else { vec![0, 1, 3, 4, 5] };
    numeric.extend(first_count..first_count + columns.counts.len());
    for i in numeric {
        if let Some(col) = t.column_mut(i) {
            col.set_cell_alignment(CellAlignment::Right);
        }
    }
    t
}

pub fn legend(target: &Target, mode: RankMode) -> String {
    format!(
        "Ranked by {} · target {}, favorites (★) first. Power and bracket follow EDHPowerLevel; grey rows are \
         outside the target. \
         Salt: EDHREC deck salt (lower is friendlier). Structure: closeness to a 36 land / 10 ramp / 10 draw / \
         8 removal / 2 wipe template. Synergy: average EDHREC synergy with the commander.",
        mode.describe(),
        target.describe()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Score;
    use crate::rank::{Weights, rank};
    use edh_data::deck::{Deck, DeckMeta};

    fn entry(id: &str) -> Entry {
        let meta = DeckMeta { edhrec_url: Some(format!("https://edhrec.com/deckpreview/{id}")), ..Default::default() };
        let deck = Deck { id: id.into(), commanders: vec!["Alpha One".into()], meta, ..Default::default() };
        Entry {
            deck,
            score: Score { power: 6.0, bracket: edh_data::deck::Bracket(3), ..Default::default() },
            matches: Vec::new(),
        }
    }

    #[test]
    fn favorites_are_starred_in_the_plain_table() {
        let decks = vec![entry("aaa"), entry("bbb")];
        let target = Target::new(edh_data::deck::BracketRange::new(2, 3), None);
        let ranking =
            rank(&decks.iter().map(|e| &e.score).collect::<Vec<_>>(), &target, RankMode::Fit, &Weights::default());
        let favorites = HashSet::from(["bbb".to_string()]);
        let mut table =
            ranked_table(&decks, &CardDb::new(vec![]), &ranking, &favorites, &target, 10, &Columns::default());
        // As wide as it needs, whatever terminal the tests run in (a build sandbox's is tiny).
        let table = table.set_width(400).to_string();
        let row =
            |id: &str| table.lines().find(|l| l.contains(id)).unwrap_or_else(|| panic!("no row for {id}:\n{table}"));
        assert!(row("bbb").contains('★'), "the favorite's row should be starred:\n{table}");
        assert!(!row("aaa").contains('★'), "only the favorite's row should be starred:\n{table}");
    }
}
