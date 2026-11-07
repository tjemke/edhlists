//! Interactive commander picker for ambiguous or unknown names.
//!
//! Opens inline below the prompt, prefilled with what was typed; every keystroke re-filters all
//! commander-eligible cards (most popular first). Enter picks, Esc cancels.

use std::io::IsTerminal;

use anyhow::Result;
use edh_data::cards::{Card, CardDb, matches_terms, search_key, search_terms};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};
use ratatui::{Frame, TerminalOptions, Viewport};

use crate::view::{ACCENT, DIM, identity_line};

/// Picker height: room for the initial matches, within these bounds.
const MIN_HEIGHT: u16 = 8;
const MAX_HEIGHT: u16 = 16;

/// Can we ask the user? (Both ends must be a terminal.)
pub fn can_pick() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub struct Picker<'a> {
    role: String,
    pool: Vec<(&'a Card, String)>,
    query: String,
    matches: Vec<usize>,
    state: TableState,
}

impl<'a> Picker<'a> {
    pub fn new(cards: &'a CardDb, role: &str, query: &str) -> Self {
        let pool = cards.commanders().into_iter().map(|c| (c, search_key(&c.name))).collect();
        let mut p = Self {
            role: role.to_string(),
            pool,
            query: query.to_string(),
            matches: vec![],
            state: TableState::default(),
        };
        p.refilter();
        p
    }

    fn refilter(&mut self) {
        let terms = search_terms(&self.query);
        self.matches = (0..self.pool.len()).filter(|&i| matches_terms(&self.pool[i].1, &terms)).collect();
        self.state.select(if self.matches.is_empty() { None } else { Some(0) });
    }

    /// Names currently shown, best first.
    pub fn shown(&self) -> Vec<&str> {
        self.matches.iter().map(|&i| self.pool[i].0.name.as_str()).collect()
    }

    pub fn type_str(&mut self, s: &str) {
        self.query.push_str(s);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    fn move_by(&mut self, d: isize) {
        if self.matches.is_empty() {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        self.state.select(Some((cur + d).clamp(0, self.matches.len() as isize - 1) as usize));
    }

    pub fn selected(&self) -> Option<&'a Card> {
        self.state.selected().and_then(|i| self.matches.get(i)).map(|&i| self.pool[i].0)
    }

    fn draw(&mut self, frame: &mut Frame) {
        let block = Block::bordered()
            .title(Line::from(vec![
                Span::styled(format!(" Which {}? ", self.role), Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("{} match{} ", self.matches.len(), if self.matches.len() == 1 { "" } else { "es" }),
                    Style::new().fg(DIM),
                ),
            ]))
            .title_bottom(Line::from(" type to filter · ↑↓ choose · ⏎ pick · esc cancel ").style(Style::new().fg(DIM)));
        let inner = block.inner(frame.area());
        frame.render_widget(block, frame.area());
        let [input, list] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("› ", Style::new().fg(ACCENT)),
                Span::raw(self.query.clone()),
                Span::styled("▏", Style::new().fg(ACCENT)),
            ])),
            input,
        );
        if self.matches.is_empty() {
            frame.render_widget(Paragraph::new(Span::styled("  no commander matches", Style::new().fg(DIM))), list);
            return;
        }
        let rows: Vec<Row> = self
            .matches
            .iter()
            .map(|&i| {
                let c = self.pool[i].0;
                Row::new(vec![
                    Cell::from(Span::styled(c.name.clone(), Style::new().add_modifier(Modifier::BOLD))),
                    Cell::from(identity_line(&c.identity)),
                    Cell::from(Span::styled(
                        c.rank.map(|r| format!("#{r}")).unwrap_or_else(|| "–".into()),
                        Style::new().fg(DIM),
                    )),
                    Cell::from(Span::styled(
                        c.type_line.split(" // ").next().unwrap_or("").to_string(),
                        Style::new().fg(DIM),
                    )),
                ])
            })
            .collect();
        let widths = [Constraint::Fill(3), Constraint::Length(6), Constraint::Length(7), Constraint::Fill(2)];
        let header = Row::new(["", "colors", "EDHREC", "type"]).style(Style::new().fg(DIM));
        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(table, list, &mut self.state);
    }
}

/// Ask the user to pick a commander. `Ok(None)` if they cancel.
pub fn pick<'a>(cards: &'a CardDb, role: &str, query: &str) -> Result<Option<&'a Card>> {
    let mut picker = Picker::new(cards, role, query);
    // borders + input line + table header = 4 rows of chrome
    let height = (picker.matches.len() as u16).saturating_add(4).clamp(MIN_HEIGHT, MAX_HEIGHT);
    let mut terminal = ratatui::try_init_with_options(TerminalOptions { viewport: Viewport::Inline(height) })?;
    let result = (|| -> Result<Option<&'a Card>> {
        loop {
            terminal.draw(|f| picker.draw(f))?;
            let Event::Key(key) = event::read()? else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Char('c') if ctrl => return Ok(None),
                KeyCode::Enter => {
                    if let Some(c) = picker.selected() {
                        return Ok(Some(c));
                    }
                }
                KeyCode::Down => picker.move_by(1),
                KeyCode::Char('n') if ctrl => picker.move_by(1),
                KeyCode::Up => picker.move_by(-1),
                KeyCode::Char('p') if ctrl => picker.move_by(-1),
                KeyCode::PageDown => picker.move_by(10),
                KeyCode::PageUp => picker.move_by(-10),
                KeyCode::Backspace => picker.backspace(),
                KeyCode::Char('u') if ctrl => {
                    picker.query.clear();
                    picker.refilter();
                }
                KeyCode::Char(ch) if !ctrl => picker.type_str(&ch.to_string()),
                _ => {}
            }
        }
    })();
    // Wipe the picker and leave the cursor where it started, so later output lines up.
    let origin = terminal.get_frame().area();
    let _ = terminal.clear();
    let _ = terminal.set_cursor_position(ratatui::layout::Position::new(0, origin.y));
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmdr(name: &str, rank: u32) -> Card {
        Card {
            name: name.into(),
            type_line: "Legendary Creature — Human".into(),
            oracle: vec![String::new()],
            layout: "normal".into(),
            rank: Some(rank),
            ..Default::default()
        }
    }

    #[test]
    fn typing_refilters_live() {
        let db = CardDb::new(vec![
            cmdr("Krenko, Mob Boss", 50),
            cmdr("Krenko, Tin Street Kingpin", 900),
            cmdr("Atraxa, Praetors' Voice", 1),
        ]);
        let mut p = Picker::new(&db, "commander", "krenko");
        assert_eq!(p.shown(), vec!["Krenko, Mob Boss", "Krenko, Tin Street Kingpin"]);
        p.type_str(" tin");
        assert_eq!(p.shown(), vec!["Krenko, Tin Street Kingpin"]);
        assert_eq!(p.selected().unwrap().name, "Krenko, Tin Street Kingpin");
        for _ in 0..10 {
            p.backspace();
        }
        assert_eq!(p.shown()[0], "Atraxa, Praetors' Voice"); // empty query: everything, most popular first
        p.type_str("zzz");
        assert!(p.selected().is_none());
    }
}
