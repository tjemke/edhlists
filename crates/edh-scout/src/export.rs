//! Writing decklists (for proxies / importing elsewhere) and a markdown report.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::analysis::Analysis;
use crate::filters::CountColumn;
use crate::rank::{RankMode, Ranked, Target};
use edh_data::deck::Deck;
use edh_data::text::front_face;
use edhpl::edhpowerlevel_url;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportFormat {
    /// `1 Card Name` per line, commander(s) first, nothing else. Works with MPCFill and most proxy
    /// printers, and imports cleanly into Moxfield and Archidekt.
    #[default]
    Plain,
    /// `Commander` / `Deck` sections (MTGA style; what EDHPowerLevel's own paste box expects).
    Sectioned,
}

pub fn decklist_text(deck: &Deck, format: ExportFormat) -> String {
    let mut lines: Vec<String> = match format {
        ExportFormat::Plain => deck
            .commanders
            .iter()
            .map(|c| format!("1 {c}"))
            .chain(deck.cards.iter().map(|(n, q)| format!("{q} {n}")))
            .collect(),
        ExportFormat::Sectioned => deck.sectioned_lines(),
    };
    lines.push(String::new());
    lines.join("\n")
}

pub fn deck_filename(position: usize, deck: &Deck) -> String {
    let id: String = deck.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    format!("{position:02}-{id}.txt")
}

/// Write one decklist into `dir/decks/`, returning its path.
pub fn write_deck(dir: &Path, position: usize, deck: &Deck, format: ExportFormat) -> Result<PathBuf> {
    let decks = dir.join("decks");
    fs::create_dir_all(&decks)?;
    let path = decks.join(deck_filename(position, deck));
    fs::write(&path, decklist_text(deck, format))?;
    Ok(path)
}

fn fmt_opt(v: Option<f64>, prec: usize) -> String {
    v.map(|x| format!("{x:.prec$}")).unwrap_or_else(|| "–".into())
}

pub struct ReportInput<'a> {
    pub title: &'a str,
    pub target: &'a Target,
    pub mode: RankMode,
    pub decks: &'a [(Deck, Analysis)],
    pub ranking: &'a [Ranked],
    /// Only the first `limit` ranked decks get a section and an exported list.
    pub limit: usize,
    pub format: ExportFormat,
    pub show_price: bool,
    /// Columns for the `-T` card filters' counts.
    pub count_columns: &'a [CountColumn],
}

/// Write `report.md` plus the decklists of the top `limit` decks. Returns the report path.
pub fn write_report(dir: &Path, input: &ReportInput) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut md = String::new();
    writeln!(md, "# {} — deck scouting report\n", input.title)?;
    writeln!(
        md,
        "Target **{}**, ranked by **{}**. Generated {} by edh-scout {}.\n",
        input.target.describe(),
        input.mode.describe(),
        edh_data::text::today(),
        env!("CARGO_PKG_VERSION")
    )?;
    writeln!(
        md,
        "Power level and bracket follow [EDHPowerLevel](https://edhpowerlevel.com)'s scoring, computed locally — \
         each deck links to the site to double-check. Salt is EDHREC's deck salt (sum of card salt; lower is \
         friendlier). Structure compares the deck to a ~36 land / 10 ramp / 10 draw / 8 removal / 2 wipe template. \
         Synergy is the average EDHREC synergy of the deck's cards with this commander.\n"
    )?;
    let commanders_of = |d: &Deck| d.commanders.iter().map(|c| front_face(c)).collect::<Vec<_>>().join(" + ");
    let shown: Vec<&Ranked> = input.ranking.iter().take(input.limit).collect();
    let many_commanders =
        shown.iter().map(|r| commanders_of(&input.decks[r.index].0)).collect::<std::collections::HashSet<_>>().len()
            > 1;
    let price_col = if input.show_price { " price |" } else { "" };
    let cmdr_col = if many_commanders { " commander |" } else { "" };
    let count_cols: String =
        input.count_columns.iter().map(|c| format!(" {} |", c.label.replace('|', "\\|"))).collect();
    writeln!(
        md,
        "| # | fits | power | bracket | salt | structure | synergy |{price_col}{cmdr_col}{count_cols} themes | list |"
    )?;
    writeln!(
        md,
        "|---|---|---|---|---|---|---|{}{}{}---|---|",
        if input.show_price { "---|" } else { "" },
        if many_commanders { "---|" } else { "" },
        "---|".repeat(input.count_columns.len())
    )?;
    for (pos, r) in input.ranking.iter().take(input.limit).enumerate() {
        let (deck, a) = &input.decks[r.index];
        let file = write_deck(dir, pos + 1, deck, input.format)?;
        let rel = file.strip_prefix(dir).unwrap_or(&file).display().to_string();
        writeln!(
            md,
            "| [{n}](#deck-{n}) | {fit} | {pl} | B{b} | {salt} | {st:.0}% | {syn} |{price}{cmdr}{counts} {tags} | [{rel}]({rel}) |",
            n = pos + 1,
            fit = if r.fits { "✓" } else { "✗" },
            pl = a.power.label(),
            b = a.bracket.bracket,
            salt = fmt_opt(a.salt, 0),
            st = a.structure.score * 100.0,
            syn = fmt_opt(a.synergy.as_ref().map(|s| s.average * 100.0), 1),
            price = if input.show_price { format!(" ${} |", fmt_opt(deck.meta.price, 0)) } else { String::new() },
            cmdr = if many_commanders { format!(" {} |", commanders_of(deck)) } else { String::new() },
            counts = input.count_columns.iter().map(|c| format!(" {} |", c.cell(&a.matches))).collect::<String>(),
            tags = if deck.meta.tags.is_empty() { "–".into() } else { deck.meta.tags.join(", ") },
        )?;
    }
    writeln!(md)?;
    for (pos, r) in input.ranking.iter().take(input.limit).enumerate() {
        let (deck, a) = &input.decks[r.index];
        writeln!(md, "## Deck {} · {}\n", pos + 1, commanders_of(deck))?;
        let mut links = Vec::new();
        if let Some(u) = &deck.meta.edhrec_url {
            links.push(format!("[EDHREC]({u})"));
        }
        if let Some(u) = &deck.meta.source_url {
            links.push(format!("[source]({u})"));
        }
        links.push(format!("[EDHPowerLevel]({})", edhpowerlevel_url(&deck.sectioned_lines())));
        writeln!(md, "{}\n", links.join(" · "))?;
        writeln!(
            md,
            "- **Power {}** (impact {:.0} × efficiency {:.2}) → **bracket {}** (rules floor B{}, power alone B{})",
            a.power.label(),
            a.power.impact,
            a.power.efficiency,
            a.bracket.bracket,
            a.bracket.floor,
            a.bracket.from_level
        )?;
        let reasons = a.bracket.reasons();
        if !reasons.is_empty() {
            writeln!(md, "- Floor raised by: {}", reasons.join(", "))?;
        }
        if !a.bracket.game_changers.is_empty() {
            writeln!(md, "- Game Changers: {}", a.bracket.game_changers.join(", "))?;
        }
        writeln!(
            md,
            "- Salt {}{}",
            fmt_opt(a.salt, 1),
            if a.salty_cards.is_empty() {
                String::new()
            } else {
                format!(
                    " — saltiest: {}",
                    a.salty_cards.iter().take(5).map(|(n, s)| format!("{n} ({s:.1})")).collect::<Vec<_>>().join(", ")
                )
            }
        )?;
        for m in &a.matches {
            let names: Vec<String> = m
                .cards
                .iter()
                .map(|(n, q)| if *q > 1 { format!("{q} {}", front_face(n)) } else { front_face(n).to_string() })
                .collect();
            let list = if names.is_empty() { String::new() } else { format!(" — {}", names.join(" · ")) };
            writeln!(md, "- `{}`: {}{list}", m.label, m.count)?;
        }
        let s = &a.structure;
        writeln!(
            md,
            "- Structure {:.0}%: {} lands, {} ramp, {} draw, {} removal, {} wipes{}",
            s.score * 100.0,
            s.lands,
            s.count(crate::roles::Role::Ramp),
            s.count(crate::roles::Role::Draw),
            s.count(crate::roles::Role::Removal),
            s.count(crate::roles::Role::Wipe),
            if s.gaps().is_empty() { String::new() } else { format!(" ({})", s.gaps().join(", ")) }
        )?;
        if let Some(syn) = &a.synergy {
            let sig: Vec<String> =
                syn.signature.iter().take(6).map(|(n, v)| format!("{n} (+{:.0}%)", v * 100.0)).collect();
            writeln!(
                md,
                "- Synergy {:.1}%{}",
                syn.average * 100.0,
                if sig.is_empty() { String::new() } else { format!(" — signature cards: {}", sig.join(", ")) }
            )?;
        }
        for c in a.combos.iter().take(5) {
            writeln!(
                md,
                "- Combo [{}{}]: {} → {}",
                c.tag,
                c.vote.as_ref().map(|v| format!(", voted B{v}")).unwrap_or_default(),
                c.cards.join(" + "),
                c.results.join("; ")
            )?;
        }
        if a.combos.len() > 5 {
            writeln!(md, "- … {} more combos", a.combos.len() - 5)?;
        }
        if !deck.meta.tags.is_empty() {
            writeln!(md, "- Themes: {}", deck.meta.tags.join(", "))?;
        }
        if !a.unknown.is_empty() {
            writeln!(md, "- Not in the card database (unscored): {}", a.unknown.join(", "))?;
        }
        writeln!(md, "- List: `decks/{}`\n", deck_filename(pos + 1, deck))?;
    }
    let path = dir.join("report.md");
    fs::write(&path, md)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edh_data::deck::parse_decklist;

    #[test]
    fn plain_export_is_proxy_friendly() {
        let mut d = parse_decklist("Commander\n1 Krenko, Mob Boss\nDeck\n10 Mountain\n1 Sol Ring\n", &[]);
        d.id = "abc/../x".into();
        assert_eq!(decklist_text(&d, ExportFormat::Plain), "1 Krenko, Mob Boss\n10 Mountain\n1 Sol Ring\n");
        assert!(decklist_text(&d, ExportFormat::Sectioned).starts_with("Commander\n1 Krenko, Mob Boss\n\nDeck\n"));
        assert_eq!(deck_filename(3, &d), "03-abcx.txt");
    }
}
