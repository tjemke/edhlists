//! The interactive deck browser. Opens at once with the decks already on disk; decks fetched in
//! the background slot into the ranking as they arrive.

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
use crate::filters::{CountColumn, short_label};
use crate::rank::{RankMode, Ranked, Target, Weights, hoist, mix, rank};
use crate::scout::{Entry, Feed, FeedEvent, FetchEvent, Pool};
use crate::view::{
    ACCENT, BAD, DIM, GOOD, OK, counted_color, deck_identity, detail_lines, grouped_list, identity_line, power_color,
    score_color, wrap_lines,
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
    /// What the header says about where the decks came from, instead of the EDHREC counts (a
    /// local list, the favorites).
    pub source: Option<String>,
    /// Shown in the status line on opening (a `-T` that only counts though it looks like a
    /// filter: [`filters::count_only_notice`](crate::filters::count_only_notice)).
    pub notice: Option<String>,
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
    favorites_only: bool,
    /// How many of the pool's decks are favorites (they lead `ranking`), for the header.
    favorite_count: usize,
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
        let status = settings.notice.clone().map(|n| (n, true));
        let mut app = Self {
            settings,
            pool,
            feed,
            http,
            ranking: Vec::new(),
            visible: Vec::new(),
            details: None,
            fits_only: false,
            favorites_only: false,
            favorite_count: 0,
            pane: Pane::Details,
            state: TableState::default(),
            scroll: 0,
            status,
            opened: Instant::now(),
        };
        app.rerank(false);
        app
    }

    /// Re-rank, favorites first. With `keep`, the cursor stays on the deck it was on (decks
    /// arriving in the background reshuffle the list; the user's place shouldn't move).
    fn rerank(&mut self, keep: bool) {
        let keep_deck = if keep { self.selected().map(|(_, r)| r.index) } else { None };
        self.ranking = rank(&self.pool.scores(), &self.settings.target, self.settings.mode, &self.settings.weights);
        if self.mixing() {
            let (group, weights) = self.pool.commander_groups();
            self.ranking = mix(std::mem::take(&mut self.ranking), &group, &weights);
        }
        self.ranking = hoist(std::mem::take(&mut self.ranking), |i| self.pool.is_favorite(i));
        self.favorite_count = self.ranking.iter().take_while(|r| self.pool.is_favorite(r.index)).count();
        self.refresh_visible();
        self.reselect(keep_deck, 0);
        if !keep {
            self.scroll = 0;
        }
    }

    /// Put the cursor on pool deck `deck` if it's on show, or else on row `fallback` (or the last
    /// row, if there are fewer).
    fn reselect(&mut self, deck: Option<usize>, fallback: usize) {
        let pos = deck
            .and_then(|d| self.visible.iter().position(|(_, r)| r.index == d))
            .or_else(|| self.visible.len().checked_sub(1).map(|last| fallback.min(last)));
        self.state.select(pos);
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
                let summary = self.settings.source.clone().unwrap_or_else(|| self.pool.stats.summary());
                self.status = Some((format!("done: {summary}"), false));
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
        let (fits_only, favorites_only, pool) = (self.fits_only, self.favorites_only, &self.pool);
        self.visible = self
            .ranking
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, r)| (!fits_only || r.fits) && (!favorites_only || pool.is_favorite(r.index)))
            .collect();
    }

    /// Show or hide decks after a filter changed, keeping the cursor on its deck if it's still
    /// on show, or else at the top.
    fn refilter(&mut self) {
        let keep = self.selected().map(|(_, r)| r.index);
        self.refresh_visible();
        self.reselect(keep, 0);
    }

    /// Mark the selected deck as a favorite, or clear the mark. A favorite moves to the top with
    /// the cursor on it; one that leaves the favorites-only list hands its row to the next deck.
    fn toggle_favorite(&mut self) {
        let Some((_, r)) = self.selected() else { return };
        let row = self.state.selected().unwrap_or(0);
        self.status = Some(match self.pool.toggle_favorite(r.index) {
            Ok(now) => {
                self.rerank(true);
                if !self.visible.iter().any(|(_, v)| v.index == r.index) {
                    self.reselect(None, row);
                }
                let msg = if now { "★ added to favorites (they're listed first)" } else { "removed from favorites" };
                (msg.into(), false)
            }
            Err(e) => (format!("couldn't change favorites: {e:#}"), true),
        });
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
                self.fits_only = !self.fits_only;
                self.refilter();
                self.status = Some((self.filter_message(), false));
            }
            KeyCode::Char('*') if self.pool.keeps_favorites() => self.toggle_favorite(),
            KeyCode::Char('F') if self.pool.keeps_favorites() => {
                self.favorites_only = !self.favorites_only;
                self.refilter();
                let none = self.favorites_only && self.visible.is_empty() && !self.ranking.is_empty();
                self.status = Some(if none && self.favorite_count > 0 {
                    ("none of the favorites is in the target: f shows them all".into(), true)
                } else if none {
                    ("no favorites among these decks yet: F shows them all again, * marks one".into(), true)
                } else {
                    (self.filter_message(), false)
                });
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
                    favorites: self.pool.favorites(),
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

    /// What the list shows, after `f` or `F`.
    fn filter_message(&self) -> String {
        match (self.fits_only, self.favorites_only) {
            (false, false) => "showing all decks",
            (true, false) => "showing only decks in the target",
            (false, true) => "showing only favorites",
            (true, true) => "showing only favorites in the target",
        }
        .into()
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
        let summary = self.settings.source.clone().unwrap_or_else(|| st.summary());
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
                // Without deck tables (the favorites), what loads is each commander's own data.
                let what = if self.settings.source.is_some() { "loading" } else { "scanning" };
                format!(" {spin} {what} commanders {}/{} ", st.commanders_scanned, st.commanders_total)
            } else {
                format!(" {spin} fetching {}/{} ", st.fetched + st.failed, st.queued)
            };
            spans.push(Span::styled(progress, Style::new().bold().fg(OK)));
        }
        let favorites = if self.favorite_count > 0 { format!(", {} ★", self.favorite_count) } else { String::new() };
        spans.push(Span::styled(
            format!(" {} decks, {fits} fit{favorites} · {summary}", self.pool.decks.len()),
            Style::new().fg(DIM),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect) {
        let show_price = self.settings.show_price;
        let show_commander = self.settings.show_commander;
        let counts = &self.settings.count_columns;
        let mut headers: Vec<String> = ["", "#", "Power", "B", "Salt", "Struct", "Syn"].map(String::from).to_vec();
        if show_price {
            headers.push("Price".into());
        }
        if show_commander {
            headers.extend(["Color".into(), "Commander".into()]);
        }
        let first_count = headers.len();
        headers.extend(counts.iter().map(|c| c.label.clone()));
        headers.push("Themes".into());
        // Only the rows that fit are built: formatting every deck's row each frame cost 15 ms for
        // 11k decks. The table then renders that window, at our own scroll offset.
        let height = usize::from(area.height.saturating_sub(3)).max(1);
        let selected = self.state.selected();
        let offset = window_offset(self.state.offset(), selected, height, self.visible.len());
        *self.state.offset_mut() = offset;
        let end = (offset + height).min(self.visible.len());
        // `#` has room for 3 digits, and grows only once the positions on screen need more.
        let num_w = number_width(self.visible[offset..end].iter().map(|(pos, _)| pos + 1));
        headers[1] = format!("{:>num_w$}", "#");
        // Each count column's header in its filter's colour, as in the details and the decklist.
        let header = Row::new(headers.into_iter().enumerate().map(|(i, h)| {
            let count = i.checked_sub(first_count).and_then(|c| counts.get(c));
            Cell::from(h).style(count.map_or(Style::new(), |c| Style::new().fg(counted_color(c.filter))))
        }))
        .style(Style::new().bold().fg(DIM));
        let rows: Vec<Row> = self.visible[offset..end]
            .iter()
            .map(|(pos, r)| {
                let Entry { deck, score: s, matches } = &self.pool.decks[r.index];
                let base = if r.fits { Style::new() } else { Style::new().fg(DIM) };
                let themes = if deck.meta.tags.is_empty() { "–".to_string() } else { deck.meta.tags.join(", ") };
                // Styled only with a star in it: the selected row is drawn reversed, so a colour
                // on an empty cell would show as a block of it.
                let star = if self.pool.is_favorite(r.index) {
                    Cell::from("★").style(Style::new().fg(OK))
                } else {
                    Cell::from("")
                };
                let mut cells = vec![
                    star,
                    Cell::from(format!("{:>num_w$}", pos + 1)),
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
                    cells.push(Cell::from(identity_line(deck_identity(deck, self.pool.cards()))));
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
            Constraint::Length(1),
            Constraint::Length(num_w as u16),
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
        let title = match (self.fits_only, self.favorites_only) {
            (false, false) => " Decks ",
            (true, false) => " Decks in target ",
            (false, true) => " Favorites ",
            (true, true) => " Favorites in target ",
        };
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
        let star = if self.pool.is_favorite(r.index) { "★ " } else { "" };
        Some((format!(" {star}#{} · {} ", pos + 1, deck.id), deck))
    }

    fn draw_details(&self, frame: &mut Frame, area: Rect) {
        let Some((title, _)) = self.pane_title() else {
            let msg = if self.favorites_only && self.fits_only && self.favorite_count > 0 {
                "None of the favorites is in the target: f shows them all."
            } else if self.favorites_only && !self.ranking.is_empty() {
                "None of these decks is a favorite yet: F shows them all again, and * marks the selected deck."
            } else if self.feed.is_some() {
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
        // Cards a -T filter counted are marked, in the filter's colour: one slot per filter that
        // counted any card here, so a card several count has several marks, always in the same order.
        let matches = &self.pool.decks[r.index].matches;
        let slots: Vec<usize> = (0..matches.len()).filter(|&i| !matches[i].cards.is_empty()).collect();
        let counted_by = |name: &str| -> Vec<bool> {
            slots.iter().map(|&i| matches[i].cards.iter().any(|(n, _)| n == name)).collect()
        };
        let mut hint: Vec<Span> = vec![Span::raw(" ")];
        for &i in &slots {
            let style = Style::new().fg(counted_color(i));
            hint.extend([
                Span::styled("▸ ", style),
                Span::styled(short_label(&matches[i].label, i), style),
                " ".into(),
            ]);
        }
        if !slots.is_empty() {
            hint.push("· ".into());
        }
        hint.push("⏎ details ".into());
        let block = Block::bordered().title(title.bold()).title_bottom(Line::from(hint).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let mut lines: Vec<Line> = Vec::new();
        for (group, cards) in grouped_list(deck, self.pool.cards()) {
            let n: u32 = cards.iter().map(|(q, _)| q).sum();
            lines.push(Line::from(Span::styled(format!("{group} ({n})"), Style::new().bold().fg(ACCENT))));
            for (q, name) in cards {
                let by = counted_by(&name);
                let mut spans: Vec<Span> = slots
                    .iter()
                    .zip(&by)
                    .map(
                        |(&i, &yes)| {
                            if yes { Span::styled("▸", Style::new().fg(counted_color(i))) } else { " ".into() }
                        },
                    )
                    .collect();
                if spans.is_empty() {
                    spans.push(" ".into());
                }
                // The name in the colour of the first filter that counted it.
                let first = slots.iter().zip(&by).find(|(_, yes)| **yes).map(|(&i, _)| i);
                let style = first.map_or(Style::new(), |i| Style::new().fg(counted_color(i)));
                spans.push(Span::styled(format!("{q} {name}"), style));
                lines.push(Line::from(spans));
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
                let keys =
                    footer_keys(usize::from(area.width), self.pool.keeps_favorites(), self.settings.show_commander);
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

/// The key hints for a footer `width` columns wide, in display order. The arrow keys go without
/// saying; when the rest don't fit, the least needed ones are left out, and `q quit` always
/// stays. `favorites`: `*` and `F` work; `mix`: so does `m` (across commanders).
fn footer_keys(width: usize, favorites: bool, mix: bool) -> Vec<(&'static str, &'static str)> {
    // (key, what, how readily it's left out: lowest first, 0 never)
    let mut keys: Vec<(&str, &str, u8)> = vec![
        ("⏎", "list/details", 2),
        ("J/K", "scroll", 13),
        ("r", "rank", 4),
        ("f", "fits only", 5),
        ("o", "EDHREC", 10),
        ("s", "source", 12),
        ("p", "EDHPowerLevel", 11),
        ("e", "export deck", 6),
        ("E", "export report", 9),
        ("q", "quit", 0),
    ];
    if favorites {
        keys.insert(4, ("*", "favorite", 3));
        keys.insert(5, ("F", "★ only", 7));
    }
    if mix {
        keys.insert(4, ("m", "mix", 8));
    }
    // Most needed first, each if it still fits: a leading space, then "k what  " per key.
    let mut by_need: Vec<usize> = (0..keys.len()).collect();
    by_need.sort_by_key(|&i| keys[i].2);
    let mut used = 1;
    let mut shown = vec![false; keys.len()];
    for i in by_need {
        let (k, what, rank) = keys[i];
        let w = k.chars().count() + what.chars().count() + 3;
        if rank == 0 || used + w <= width {
            used += w;
            shown[i] = true;
        }
    }
    keys.into_iter().zip(shown).filter(|(_, s)| *s).map(|((k, w, _), _)| (k, w)).collect()
}

/// The width of the `#` column: 3, or the digits of the largest of these positions if it has more.
/// Up to 999 the column stays put while scrolling.
fn number_width(positions: impl Iterator<Item = usize>) -> usize {
    positions.max().map_or(0, |n| n.max(1).ilog10() as usize + 1).max(3)
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
mod scenarios;

#[cfg(test)]
mod tests {
    use super::{footer_keys, number_width, window_offset};

    #[test]
    fn the_number_column_fits_the_positions_on_screen() {
        // Three digits' room whatever is on screen, so scrolling up to 999 never shifts the table.
        assert_eq!(number_width([].into_iter()), 3);
        assert_eq!(number_width([1, 2, 9].into_iter()), 3);
        assert_eq!(number_width([999].into_iter()), 3);
        // Past 999 it grows (the old fixed width cut row 11066 to "110").
        assert_eq!(number_width([998, 1000].into_iter()), 4);
        assert_eq!(number_width([11030, 11066].into_iter()), 5);
    }

    #[test]
    fn the_footer_fits_its_width_and_keeps_quit() {
        let width = |keys: &[(&str, &str)]| {
            1 + keys.iter().map(|(k, w)| k.chars().count() + w.chars().count() + 3).sum::<usize>()
        };
        // Every key fits 160 columns, one commander or many.
        for (favorites, mix) in [(true, false), (true, true), (false, false)] {
            let all = footer_keys(usize::MAX, favorites, mix);
            assert_eq!(footer_keys(160, favorites, mix), all);
            assert!(width(&all) <= 160, "{}", width(&all));
        }
        // Narrower, the most needed stay, in their places, and quit stays last; a short hint
        // further down takes room a long one left.
        let narrow = footer_keys(80, true, true);
        assert!(width(&narrow) <= 80);
        let names: Vec<&str> = narrow.iter().map(|(k, _)| *k).collect();
        assert_eq!(names, vec!["⏎", "r", "f", "m", "*", "e", "q"]);
        let names: Vec<&str> = footer_keys(90, true, false).iter().map(|(k, _)| *k).collect();
        assert_eq!(names, vec!["⏎", "r", "f", "*", "F", "e", "q"]);
        assert_eq!(footer_keys(0, true, true), vec![("q", "quit")]);
    }

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
