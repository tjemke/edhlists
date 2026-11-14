//! The interactive deck browser. Opens at once with the decks already on disk; decks fetched in
//! the background slot into the ranking as they arrive.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use edh_data::deck::Deck;
use edh_data::http::Http;
use edh_data::text::front_face;
use edhpl::edhpowerlevel_url;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::analysis::Analysis;
use crate::config::PaneLayout;
use crate::export::{self, ExportFormat, ReportInput};
use crate::filters::CountColumn;
use crate::rank::{RankMode, Ranked, Target, Weights, mix, rank};
use crate::scout::{Entry, Feed, FeedEvent, FetchEvent, Pool};
use crate::view::{
    ACCENT, BAD, DIM, GOOD, OK, deck_identity, detail_lines, grouped_list, identity_line, power_color, score_color,
    wrap_lines,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Details,
    Decklist,
}

/// Everything the browser shows that isn't the decks themselves.
pub struct Settings {
    pub title: String,
    pub target: Target,
    pub mode: RankMode,
    pub weights: Weights,
    pub export_dir: PathBuf,
    pub format: ExportFormat,
    pub report_top: usize,
    /// Show EDHREC's deck price (off by default: it's usually one or two cards).
    pub show_price: bool,
    /// Searching across commanders: show which commander each deck is for.
    pub show_commander: bool,
    /// Searching across commanders: interleave commanders in the ranking (`m` toggles).
    pub mix: bool,
    /// Columns for the `-T` card filters' counts.
    pub count_columns: Vec<CountColumn>,
    /// Deck list beside the details, or above them.
    pub layout: PaneLayout,
}

pub struct App<'a> {
    settings: Settings,
    pool: Pool<'a>,
    /// Decks still being scored or fetched in the background.
    feed: Option<Feed>,
    /// Shared HTTP client, to surface its notices (retries, back-offs) in the status line.
    http: Option<&'a Http>,
    ranking: Vec<Ranked>,
    /// The part of `ranking` on show (all of it, or only the fits), with each deck's position in
    /// the full ranking. Rebuilt when the ranking or the filter changes, not every frame.
    visible: Vec<(usize, Ranked)>,
    /// The selected deck's full analysis (by pool index), worked out when the selection moves.
    details: Option<(usize, Analysis)>,
    fits_only: bool,
    pane: Pane,
    state: TableState,
    scroll: u16,
    status: Option<(String, bool)>,
    /// When the browser opened; the spinner's frame follows the clock, not the loop.
    opened: Instant,
}

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
/// How often the event loop wakes up when nothing happens, to pick up arriving decks.
const TICK: Duration = Duration::from_millis(120);

impl<'a> App<'a> {
    pub fn new(settings: Settings, pool: Pool<'a>, feed: Option<Feed>, http: Option<&'a Http>) -> Self {
        let mut app = Self {
            settings,
            pool,
            feed,
            http,
            ranking: Vec::new(),
            visible: Vec::new(),
            details: None,
            fits_only: false,
            pane: Pane::Details,
            state: TableState::default(),
            scroll: 0,
            status: None,
            opened: Instant::now(),
        };
        app.rerank(false);
        app
    }

    /// Re-rank. With `keep`, the cursor stays on the deck it was on (decks arriving in the
    /// background reshuffle the list; the user's place shouldn't move).
    fn rerank(&mut self, keep: bool) {
        let keep_deck = if keep { self.selected().map(|(_, r)| r.index) } else { None };
        self.ranking = rank(&self.pool.scores(), &self.settings.target, self.settings.mode, &self.settings.weights);
        if self.mixing() {
            let (group, weights) = self.pool.commander_groups();
            self.ranking = mix(std::mem::take(&mut self.ranking), &group, &weights);
        }
        self.refresh_visible();
        let pos = keep_deck
            .and_then(|d| self.visible.iter().position(|(_, r)| r.index == d))
            .or(if self.visible.is_empty() { None } else { Some(0) });
        self.state.select(pos);
        if !keep {
            self.scroll = 0;
        }
    }

    fn mixing(&self) -> bool {
        self.settings.show_commander && self.settings.mix
    }

    /// Counts for the end-of-run summary.
    pub fn stats(&self) -> &crate::scout::PoolStats {
        &self.pool.stats
    }

    /// Take in what the feed has delivered since the last frame: decks already scored, so this
    /// is quick however many there are. Returns whether anything changed.
    fn pump(&mut self) -> bool {
        let mut arrived = false;
        let mut finished = false;
        let mut changed = false;
        if let Some(f) = &self.feed {
            while let Some(event) = f.try_next() {
                changed = true;
                finished |= matches!(event, FeedEvent::Fetch(FetchEvent::Done | FetchEvent::Stopped(_)));
                arrived |= matches!(event, FeedEvent::Scored { .. });
                if let Some(msg) = self.pool.apply(event) {
                    self.status = Some((msg, true));
                }
            }
        }
        if finished {
            self.feed = None;
            if self.status.is_none() {
                self.status = Some((format!("done: {}", self.pool.stats.summary()), false));
            }
        }
        if arrived {
            self.rerank(true);
        }
        if let Some(n) = self.http.and_then(|h| h.take_notices().pop()) {
            self.status = Some((n, true));
            changed = true;
        }
        changed
    }

    /// The spinner's frame: it follows the clock, not the loop.
    fn spinner_frame(&self) -> usize {
        (self.opened.elapsed().as_millis() / TICK.as_millis()) as usize
    }

    fn refresh_visible(&mut self) {
        self.visible = self.ranking.iter().copied().enumerate().filter(|(_, r)| !self.fits_only || r.fits).collect();
    }

    fn selected(&self) -> Option<(usize, Ranked)> {
        self.state.selected().and_then(|i| self.visible.get(i).copied())
    }

    /// Work out the selected deck's full analysis if the selection moved (about a millisecond).
    fn refresh_details(&mut self) {
        let want = self.selected().map(|(_, r)| r.index);
        if want != self.details.as_ref().map(|(i, _)| *i) {
            self.details = want.map(|i| (i, self.pool.details(i)));
        }
    }

    fn move_by(&mut self, delta: isize) {
        let n = self.visible.len();
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
                self.settings.mode = self.settings.mode.next();
                self.rerank(true);
                self.status = Some((format!("ranked by {}", self.settings.mode.label()), false));
            }
            KeyCode::Char('m') if self.settings.show_commander => {
                self.settings.mix = !self.settings.mix;
                self.rerank(true);
                let msg = if self.settings.mix { "mixing commanders" } else { "ranking by score alone" };
                self.status = Some((msg.into(), false));
            }
            KeyCode::Char('f') => {
                let keep = self.selected().map(|(_, r)| r.index);
                self.fits_only = !self.fits_only;
                self.refresh_visible();
                let pos = keep.and_then(|d| self.visible.iter().position(|(_, r)| r.index == d));
                self.state.select(pos.or(if self.visible.is_empty() { None } else { Some(0) }));
                let msg = if self.fits_only { "showing only decks in the target" } else { "showing all decks" };
                self.status = Some((msg.into(), false));
            }
            KeyCode::Char('o') => {
                let url = self.selected().and_then(|(_, r)| self.pool.decks[r.index].deck.meta.edhrec_url.clone());
                self.open("EDHREC", url);
            }
            KeyCode::Char('s') => {
                let url = self.selected().and_then(|(_, r)| self.pool.decks[r.index].deck.meta.source_url.clone());
                self.open("source", url);
            }
            KeyCode::Char('p') => {
                let url =
                    self.selected().map(|(_, r)| edhpowerlevel_url(&self.pool.decks[r.index].deck.sectioned_lines()));
                self.open("EDHPowerLevel", url);
            }
            KeyCode::Char('e') => {
                if let Some((pos, r)) = self.selected() {
                    self.status = Some(
                        match export::write_deck(
                            &self.settings.export_dir,
                            pos + 1,
                            &self.pool.decks[r.index].deck,
                            self.settings.format,
                        ) {
                            Ok(p) => (format!("wrote {}", p.display()), false),
                            Err(e) => (format!("export failed: {e:#}"), true),
                        },
                    );
                }
            }
            KeyCode::Char('E') => {
                let ranking: Vec<Ranked> = self.visible.iter().map(|(_, r)| *r).collect();
                let input = ReportInput {
                    title: &self.settings.title,
                    target: &self.settings.target,
                    mode: self.settings.mode,
                    decks: &self.pool.decks,
                    analyze: &|i| self.pool.details(i),
                    ranking: &ranking,
                    limit: self.settings.report_top,
                    format: self.settings.format,
                    show_price: self.settings.show_price,
                    count_columns: &self.settings.count_columns,
                };
                self.status = Some(match export::write_report(&self.settings.export_dir, &input) {
                    Ok(p) => (
                        format!("wrote {} and {} decklists", p.display(), ranking.len().min(self.settings.report_top)),
                        false,
                    ),
                    Err(e) => (format!("export failed: {e:#}"), true),
                });
            }
            _ => {}
        }
        true
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.refresh_details();
        let [header, body, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
        self.draw_header(frame, header);
        let size = frame.area();
        let [left, right] = if self.settings.layout.side_by_side(size.width, size.height) {
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
        let st = &self.pool.stats;
        let summary = if st.listed == 0 { "local decklist".to_string() } else { st.summary() };
        let mut spans = vec![
            Span::styled(" edh-scout ", Style::new().reversed().bold().fg(ACCENT)),
            Span::styled(format!(" {} ", self.settings.title), Style::new().bold()),
            Span::styled(format!(" target {} ", self.settings.target.describe()), Style::new().fg(GOOD)),
            Span::styled(
                format!(" rank: {}{} ", self.settings.mode.label(), if self.mixing() { ", mixed" } else { "" }),
                Style::new().fg(ACCENT),
            ),
        ];
        if self.feed.is_some() {
            let spin = SPINNER[self.spinner_frame() % SPINNER.len()];
            let progress = if st.scoring > 0 {
                format!(" {spin} scoring {} more stored decks ", st.scoring)
            } else if st.scanning() {
                format!(" {spin} scanning commanders {}/{} ", st.commanders_scanned, st.commanders_total)
            } else {
                format!(" {spin} fetching {}/{} ", st.fetched + st.failed, st.queued)
            };
            spans.push(Span::styled(progress, Style::new().bold().fg(OK)));
        }
        spans.push(Span::styled(
            format!(" {} decks, {fits} fit · {summary}", self.pool.decks.len()),
            Style::new().fg(DIM),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect) {
        let show_price = self.settings.show_price;
        let show_commander = self.settings.show_commander;
        let counts = &self.settings.count_columns;
        let mut headers: Vec<String> = ["#", "Power", "B", "Salt", "Struct", "Syn"].map(String::from).to_vec();
        if show_price {
            headers.push("Price".into());
        }
        if show_commander {
            headers.extend(["Color".into(), "Commander".into()]);
        }
        headers.extend(counts.iter().map(|c| c.label.clone()));
        headers.push("Themes".into());
        let header = Row::new(headers).style(Style::new().bold().fg(DIM));
        // Only the rows that fit are built: formatting every deck's row each frame cost 15 ms for
        // 11k decks. The table then renders that window, at our own scroll offset.
        let height = usize::from(area.height.saturating_sub(3)).max(1);
        let selected = self.state.selected();
        let offset = window_offset(self.state.offset(), selected, height, self.visible.len());
        *self.state.offset_mut() = offset;
        let end = (offset + height).min(self.visible.len());
        let rows: Vec<Row> = self.visible[offset..end]
            .iter()
            .map(|(pos, r)| {
                let Entry { deck, score: s, matches } = &self.pool.decks[r.index];
                let base = if r.fits { Style::new() } else { Style::new().fg(DIM) };
                let themes = if deck.meta.tags.is_empty() { "–".to_string() } else { deck.meta.tags.join(", ") };
                let mut cells = vec![
                    Cell::from(format!("{:>2}", pos + 1)),
                    Cell::from(format!("{:>5}", s.power_label()))
                        .style(Style::new().fg(power_color(&self.settings.target, s.power)).bold()),
                    Cell::from(format!("B{}", s.bracket)).style(Style::new().fg(if r.fits { GOOD } else { BAD })),
                    Cell::from(s.salt.map(|s| format!("{s:>4.0}")).unwrap_or("   –".into()))
                        .style(Style::new().fg(score_color(r.scores.salt))),
                    Cell::from(format!("{:>5.0}%", s.structure * 100.0))
                        .style(Style::new().fg(score_color(s.structure))),
                    Cell::from(s.synergy.map(|s| format!("{:>4.1}%", s * 100.0)).unwrap_or("–".into()))
                        .style(Style::new().fg(score_color(r.scores.synergy))),
                ];
                if show_price {
                    cells.push(Cell::from(
                        deck.meta.price.map(|p| format!("{:>6}", format!("${p:.0}"))).unwrap_or("–".into()),
                    ));
                }
                if show_commander {
                    cells.push(Cell::from(identity_line(&deck_identity(deck, self.pool.cards()))));
                    let names: Vec<&str> = deck.commanders.iter().map(|c| front_face(c)).collect();
                    cells.push(Cell::from(names.join(" + ")).style(Style::new().fg(ACCENT)));
                }
                for c in counts {
                    let n = c.cell(matches);
                    cells.push(Cell::from(format!("{n:>w$}", w = c.label.chars().count().max(3))));
                }
                cells.push(Cell::from(themes));
                Row::new(cells).style(base)
            })
            .collect();
        let mut widths = vec![
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(2),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(5),
        ];
        if show_price {
            widths.push(Constraint::Length(6));
        }
        if show_commander {
            widths.push(Constraint::Length(5));
            widths.push(Constraint::Fill(1));
        }
        widths.extend(counts.iter().map(|c| Constraint::Length(c.label.chars().count().max(3) as u16)));
        widths.push(Constraint::Fill(1));
        let title = if self.fits_only { " Decks in target " } else { " Decks " };
        let table = Table::new(rows, widths)
            .header(header)
            .block(Block::bordered().title(title.bold()))
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");
        let mut window = TableState::default().with_selected(selected.map(|s| s - offset));
        frame.render_stateful_widget(table, area, &mut window);
    }

    fn pane_title(&self) -> Option<(String, &Deck)> {
        let (pos, r) = self.selected()?;
        let deck = &self.pool.decks[r.index].deck;
        Some((format!(" #{} · {} ", pos + 1, deck.id), deck))
    }

    fn draw_details(&self, frame: &mut Frame, area: Rect) {
        let Some((title, _)) = self.pane_title() else {
            let msg = if self.feed.is_some() {
                "Fetching decklists in the background — they show up here as they arrive (one every few seconds)."
            } else {
                "No decks to show."
            };
            let p = Paragraph::new(Span::styled(msg, Style::new().fg(DIM))).wrap(Wrap { trim: false });
            frame.render_widget(p.block(Block::bordered()), area);
            return;
        };
        let (_, r) = self.selected().expect("selected");
        let Some((_, a)) = &self.details else { return };
        let deck = &self.pool.decks[r.index].deck;
        let lines = detail_lines(deck, a, &self.settings.target, Some(&r), self.settings.show_price);
        let lines = wrap_lines(lines, area.width.saturating_sub(2) as usize);
        let p = Paragraph::new(lines)
            .scroll((self.scroll, 0))
            .block(Block::bordered().title(title.bold()).title_bottom(Line::from(" ⏎ decklist ").right_aligned()));
        frame.render_widget(p, area);
    }

    fn draw_decklist(&self, frame: &mut Frame, area: Rect) {
        let Some((title, deck)) = self.pane_title() else { return };
        let Some((_, r)) = self.selected() else { return };
        // Cards a -T card filter counted are marked.
        let counted: HashSet<&str> =
            self.pool.decks[r.index].matches.iter().flat_map(|m| m.cards.iter().map(|(n, _)| n.as_str())).collect();
        let hint = if counted.is_empty() { " ⏎ details " } else { " ▸ counted by -T · ⏎ details " };
        let block = Block::bordered().title(title.bold()).title_bottom(Line::from(hint).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let mut lines: Vec<Line> = Vec::new();
        for (group, cards) in grouped_list(deck, self.pool.cards()) {
            let n: u32 = cards.iter().map(|(q, _)| q).sum();
            lines.push(Line::from(Span::styled(format!("{group} ({n})"), Style::new().bold().fg(ACCENT))));
            for (q, name) in cards {
                lines.push(if counted.contains(name.as_str()) {
                    Line::from(vec![
                        Span::styled("▸", Style::new().fg(GOOD)),
                        Span::styled(format!("{q} {name}"), Style::new().fg(GOOD)),
                    ])
                } else {
                    Line::from(format!(" {q} {name}"))
                });
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
                let mut keys = vec![
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
                if self.settings.show_commander {
                    keys.insert(5, ("m", "mix commanders"));
                }
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

/// The first row to show in a window of `height` rows over `len`: the previous one, moved just
/// enough to keep the selection in view, and never past the end.
fn window_offset(previous: usize, selected: Option<usize>, height: usize, len: usize) -> usize {
    let mut offset = previous.min(len.saturating_sub(height));
    if let Some(sel) = selected {
        if sel < offset {
            offset = sel;
        } else if sel >= offset + height {
            offset = sel + 1 - height;
        }
    }
    offset
}

pub fn run(app: &mut App) -> Result<()> {
    if let Some(h) = app.http {
        h.set_quiet(true);
    }
    let mut terminal: DefaultTerminal = ratatui::init();
    let result = (|| -> Result<()> {
        // Draw only when something changed: a key, a resize, decks arriving, or (while work goes
        // on in the background) the spinner moving on.
        let mut dirty = true;
        let mut spun = usize::MAX;
        loop {
            dirty |= app.pump();
            if app.feed.is_some() && app.spinner_frame() != spun {
                spun = app.spinner_frame();
                dirty = true;
            }
            if dirty {
                terminal.draw(|f| app.draw(f))?;
                dirty = false;
            }
            // Wake up regularly to pick up decks arriving in the background.
            if event::poll(TICK)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if !app.handle(key) {
                            return Ok(());
                        }
                        dirty = true;
                    }
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
            }
        }
    })();
    ratatui::restore();
    if let Some(h) = app.http {
        h.set_quiet(false);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::window_offset;

    #[test]
    fn the_window_follows_the_selection_and_stays_in_range() {
        // Moving within the window keeps it still; leaving it scrolls just enough.
        assert_eq!(window_offset(0, Some(5), 10, 100), 0);
        assert_eq!(window_offset(0, Some(10), 10, 100), 1);
        assert_eq!(window_offset(20, Some(19), 10, 100), 19);
        assert_eq!(window_offset(20, Some(25), 10, 100), 20);
        // A list that shrank (the fits filter) pulls the window back.
        assert_eq!(window_offset(50, Some(3), 10, 8), 0);
        assert_eq!(window_offset(50, None, 10, 30), 20);
        assert_eq!(window_offset(0, None, 10, 0), 0);
    }
}
