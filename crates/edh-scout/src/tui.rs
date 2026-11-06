//! The interactive deck browser.

use std::path::PathBuf;

use anyhow::Result;
use edh_data::cards::CardDb;
use edh_data::deck::Deck;
use edhpl::edhpowerlevel_url;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::analysis::Analysis;
use crate::export::{self, ExportFormat, ReportInput};
use crate::rank::{RankMode, Ranked, Target, Weights, rank};
use crate::view::{ACCENT, BAD, DIM, GOOD, detail_lines, grouped_list, power_color, score_color};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Details,
    Decklist,
}

pub struct App<'a> {
    pub title: String,
    pub summary: String,
    pub target: Target,
    pub mode: RankMode,
    pub weights: Weights,
    pub decks: &'a [(Deck, Analysis)],
    pub cards: &'a CardDb,
    pub export_dir: PathBuf,
    pub format: ExportFormat,
    pub report_top: usize,
    ranking: Vec<Ranked>,
    fits_only: bool,
    pane: Pane,
    state: TableState,
    scroll: u16,
    status: Option<(String, bool)>,
}

impl<'a> App<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: String,
        summary: String,
        target: Target,
        mode: RankMode,
        weights: Weights,
        decks: &'a [(Deck, Analysis)],
        cards: &'a CardDb,
        export_dir: PathBuf,
        format: ExportFormat,
        report_top: usize,
    ) -> Self {
        let mut app = Self {
            title,
            summary,
            target,
            mode,
            weights,
            decks,
            cards,
            export_dir,
            format,
            report_top,
            ranking: Vec::new(),
            fits_only: false,
            pane: Pane::Details,
            state: TableState::default(),
            scroll: 0,
            status: None,
        };
        app.rerank();
        app
    }

    fn rerank(&mut self) {
        let refs: Vec<&Analysis> = self.decks.iter().map(|(_, a)| a).collect();
        self.ranking = rank(&refs, &self.target, self.mode, &self.weights);
        self.state.select(if self.visible().is_empty() { None } else { Some(0) });
        self.scroll = 0;
    }

    fn visible(&self) -> Vec<(usize, Ranked)> {
        self.ranking.iter().copied().enumerate().filter(|(_, r)| !self.fits_only || r.fits).collect()
    }

    fn selected(&self) -> Option<(usize, Ranked)> {
        self.state.selected().and_then(|i| self.visible().get(i).copied())
    }

    fn move_by(&mut self, delta: isize) {
        let n = self.visible().len();
        if n == 0 {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        self.state.select(Some((cur + delta).clamp(0, n as isize - 1) as usize));
        self.scroll = 0;
    }

    fn open(&mut self, what: &str, url: Option<String>) {
        self.status = Some(match url {
            Some(u) => match open::that_detached(&u) {
                Ok(()) => (format!("opened {what} in your browser"), false),
                Err(e) => (format!("couldn't open a browser ({e}): {u}"), true),
            },
            None => (format!("this deck has no {what} link"), true),
        });
    }

    fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        self.status = None;
        match key.code {
            KeyCode::Char('q') => return false,
            KeyCode::Char('c') if ctrl => return false,
            KeyCode::Esc if self.pane == Pane::Decklist => self.pane = Pane::Details,
            KeyCode::Esc => return false,
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(-100_000),
            KeyCode::End | KeyCode::Char('G') => self.move_by(100_000),
            KeyCode::Char('J') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Char('K') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.scroll = self.scroll.saturating_add(10),
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') => {
                self.pane = if self.pane == Pane::Details { Pane::Decklist } else { Pane::Details };
                self.scroll = 0;
            }
            KeyCode::Char('r') => {
                self.mode = self.mode.next();
                self.rerank();
                self.status = Some((format!("ranked by {}", self.mode.label()), false));
            }
            KeyCode::Char('f') => {
                self.fits_only = !self.fits_only;
                self.state.select(if self.visible().is_empty() { None } else { Some(0) });
                let msg = if self.fits_only { "showing only decks in the target" } else { "showing all decks" };
                self.status = Some((msg.into(), false));
            }
            KeyCode::Char('o') => {
                let url = self.selected().and_then(|(_, r)| self.decks[r.index].0.meta.edhrec_url.clone());
                self.open("EDHREC", url);
            }
            KeyCode::Char('s') => {
                let url = self.selected().and_then(|(_, r)| self.decks[r.index].0.meta.source_url.clone());
                self.open("source", url);
            }
            KeyCode::Char('p') => {
                let url = self.selected().map(|(_, r)| edhpowerlevel_url(&self.decks[r.index].0.sectioned_lines()));
                self.open("EDHPowerLevel", url);
            }
            KeyCode::Char('e') => {
                if let Some((pos, r)) = self.selected() {
                    self.status = Some(
                        match export::write_deck(&self.export_dir, pos + 1, &self.decks[r.index].0, self.format) {
                            Ok(p) => (format!("wrote {}", p.display()), false),
                            Err(e) => (format!("export failed: {e:#}"), true),
                        },
                    );
                }
            }
            KeyCode::Char('E') => {
                let ranking: Vec<Ranked> = self.visible().into_iter().map(|(_, r)| r).collect();
                let input = ReportInput {
                    title: &self.title,
                    target: &self.target,
                    mode: self.mode,
                    decks: self.decks,
                    ranking: &ranking,
                    limit: self.report_top,
                    format: self.format,
                };
                self.status = Some(match export::write_report(&self.export_dir, &input) {
                    Ok(p) => {
                        (format!("wrote {} and {} decklists", p.display(), ranking.len().min(self.report_top)), false)
                    }
                    Err(e) => (format!("export failed: {e:#}"), true),
                });
            }
            _ => {}
        }
        true
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header, body, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
        self.draw_header(frame, header);
        let wide = body.width >= 110;
        let [left, right] = if wide {
            Layout::horizontal([Constraint::Percentage(47), Constraint::Fill(1)]).areas(body)
        } else {
            Layout::vertical([Constraint::Percentage(40), Constraint::Fill(1)]).areas(body)
        };
        self.draw_table(frame, left);
        match self.pane {
            Pane::Details => self.draw_details(frame, right),
            Pane::Decklist => self.draw_decklist(frame, right),
        }
        self.draw_footer(frame, footer);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let fits = self.ranking.iter().filter(|r| r.fits).count();
        let line = Line::from(vec![
            Span::styled(" edh-scout ", Style::new().reversed().bold().fg(ACCENT)),
            Span::styled(format!(" {} ", self.title), Style::new().bold()),
            Span::styled(format!(" target {} ", self.target.describe()), Style::new().fg(GOOD)),
            Span::styled(format!(" rank: {} ", self.mode.label()), Style::new().fg(ACCENT)),
            Span::styled(format!(" {} decks, {fits} fit · {}", self.decks.len(), self.summary), Style::new().fg(DIM)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect) {
        let header = Row::new(["#", "Power", "B", "Salt", "Struct", "Syn", "Price", "Themes"])
            .style(Style::new().bold().fg(DIM));
        let rows: Vec<Row> = self
            .visible()
            .iter()
            .map(|(pos, r)| {
                let (deck, a) = &self.decks[r.index];
                let base = if r.fits { Style::new() } else { Style::new().fg(DIM) };
                let themes = if deck.meta.tags.is_empty() { "–".to_string() } else { deck.meta.tags.join(", ") };
                Row::new(vec![
                    Cell::from(format!("{:>2}", pos + 1)),
                    Cell::from(format!("{:>5}", a.power.label()))
                        .style(Style::new().fg(power_color(&self.target, a.power.level)).bold()),
                    Cell::from(format!("B{}", a.bracket.bracket)).style(Style::new().fg(if r.fits {
                        GOOD
                    } else {
                        BAD
                    })),
                    Cell::from(a.salt.map(|s| format!("{s:>4.0}")).unwrap_or("   –".into()))
                        .style(Style::new().fg(score_color(r.scores.salt))),
                    Cell::from(format!("{:>5.0}%", a.structure.score * 100.0))
                        .style(Style::new().fg(score_color(a.structure.score))),
                    Cell::from(
                        a.synergy.as_ref().map(|s| format!("{:>4.1}%", s.average * 100.0)).unwrap_or("–".into()),
                    )
                    .style(Style::new().fg(score_color(r.scores.synergy))),
                    Cell::from(deck.meta.price.map(|p| format!("{:>6}", format!("${p:.0}"))).unwrap_or("–".into())),
                    Cell::from(themes),
                ])
                .style(base)
            })
            .collect();
        let widths = [
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(2),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Fill(1),
        ];
        let title = if self.fits_only { " Decks in target " } else { " Decks " };
        let table = Table::new(rows, widths)
            .header(header)
            .block(Block::bordered().title(title.bold()))
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(table, area, &mut self.state);
    }

    fn pane_title(&self) -> Option<(String, &Deck)> {
        let (pos, r) = self.selected()?;
        let deck = &self.decks[r.index].0;
        Some((format!(" #{} · {} ", pos + 1, deck.id), deck))
    }

    fn draw_details(&self, frame: &mut Frame, area: Rect) {
        let Some((title, _)) = self.pane_title() else {
            frame.render_widget(Paragraph::new("No decks to show.").block(Block::bordered()), area);
            return;
        };
        let (_, r) = self.selected().expect("selected");
        let (deck, a) = &self.decks[r.index];
        let lines = detail_lines(deck, a, &self.target, Some(&r));
        let p = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0))
            .block(Block::bordered().title(title.bold()).title_bottom(Line::from(" ⏎ decklist ").right_aligned()));
        frame.render_widget(p, area);
    }

    fn draw_decklist(&self, frame: &mut Frame, area: Rect) {
        let Some((title, deck)) = self.pane_title() else { return };
        let block = Block::bordered().title(title.bold()).title_bottom(Line::from(" ⏎ details ").right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let mut lines: Vec<Line> = Vec::new();
        for (group, cards) in grouped_list(deck, self.cards) {
            let n: u32 = cards.iter().map(|(q, _)| q).sum();
            lines.push(Line::from(Span::styled(format!("{group} ({n})"), Style::new().bold().fg(ACCENT))));
            for (q, name) in cards {
                lines.push(Line::from(format!(" {q} {name}")));
            }
            lines.push(Line::default());
        }
        let cols = ((inner.width / 34).max(1) as usize).min(4);
        let per = lines.len().div_ceil(cols);
        let areas = Layout::horizontal(vec![Constraint::Fill(1); cols]).split(inner);
        for (i, chunk) in lines.chunks(per.max(1)).enumerate() {
            if let Some(a) = areas.get(i) {
                frame.render_widget(Paragraph::new(chunk.to_vec()).scroll((self.scroll, 0)), *a);
            }
        }
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let line = match &self.status {
            Some((msg, err)) => {
                Line::from(Span::styled(format!(" {msg}"), Style::new().fg(if *err { BAD } else { GOOD })))
            }
            None => {
                let keys = [
                    ("↑↓", "move"),
                    ("⏎", "list/details"),
                    ("J/K", "scroll"),
                    ("r", "rank"),
                    ("f", "fits only"),
                    ("o", "EDHREC"),
                    ("s", "source"),
                    ("p", "EDHPowerLevel"),
                    ("e", "export deck"),
                    ("E", "export report"),
                    ("q", "quit"),
                ];
                let mut spans = vec![Span::raw(" ")];
                for (k, what) in keys {
                    spans.push(Span::styled(k, Style::new().bold().fg(ACCENT)));
                    spans.push(Span::styled(format!(" {what}  "), Style::new().fg(DIM)));
                }
                Line::from(spans)
            }
        };
        frame.render_widget(Paragraph::new(line), area);
    }
}

pub fn run(mut app: App) -> Result<()> {
    let mut terminal: DefaultTerminal = ratatui::init();
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|f| app.draw(f))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && !app.handle(key)
            {
                return Ok(());
            }
        }
    })();
    ratatui::restore();
    result
}
