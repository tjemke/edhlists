//! The browser as a user drives it: keys in, what the list shows out.
//!
//! Scenarios talk to the browser only through [`Browser`] (`press`, `arrive`, `ids`, `starred`,
//! `selected`, `status`, `count_columns`, `marks`, `text`) and never compare whole screens. When the browser's internals
//! change, fix the driver, not the scenarios; when its look changes, nothing here should break.
//! What any screen must get right, at any size, is checked by
//! `the_screen_holds_together_at_every_size`. See "When a test fails" in AGENTS.md.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use edh_data::cards::{Card, CardDb};
use edh_data::combos::ComboDb;
use edh_data::deck::{Deck, DeckMeta};
use edh_store::DeckStore;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;

use super::{App, Settings};
use crate::analysis::Score;
use crate::config::PaneLayout;
use crate::export::ExportFormat;
use crate::filters::{self, CompiledFilter, Filter};
use crate::rank::{RankMode, Target, Weights};
use crate::scout::{Entry, Feed, FeedEvent, Origin, Pool, PoolStats};

/// What the browsers in a test share: the card and combo databases, and a deck database that
/// outlives any one browser (favorites are kept there).
struct World {
    cards: CardDb,
    combos: ComboDb,
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let card = |name: &str, type_line: &str| Card {
            name: name.into(),
            type_line: type_line.into(),
            oracle: vec![String::new()],
            ..Default::default()
        };
        Self {
            cards: CardDb::new(vec![
                card("Alpha One", "Legendary Creature — Human"),
                card("Beta Two", "Legendary Creature — Elf"),
                card("Grizzly Bears", "Creature — Bear"),
                card("Bonesplitter", "Artifact — Equipment"),
                card("Lizard Blades", "Artifact Creature — Equipment Lizard"),
                card("Sol Ring", "Artifact"),
                card("Jace Beleren", "Legendary Planeswalker — Jace"),
            ]),
            combos: ComboDb::empty(),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn store(&self) -> DeckStore {
        DeckStore::open(&self.dir.path().join("decks.sqlite"), self.dir.path().join("pages")).unwrap()
    }
}

/// A deck called `id` for `commander`, scoring `power`: in the default target (B2–3) up to 7.7,
/// outside it above that.
fn scored(id: &str, power: f64, commander: &str) -> (Deck, Score) {
    scored_with(id, power, commander, &["Grizzly Bears"])
}

/// [`scored`], running these cards (one of each).
fn scored_with(id: &str, power: f64, commander: &str, cards: &[&str]) -> (Deck, Score) {
    let deck = Deck {
        id: id.into(),
        commanders: vec![commander.into()],
        cards: cards.iter().map(|c| (c.to_string(), 1)).collect(),
        meta: DeckMeta { edhrec_url: Some(format!("https://edhrec.com/deckpreview/{id}")), ..Default::default() },
    };
    let bracket = if power > 7.7 { 5 } else { 3 };
    (
        deck,
        Score {
            power,
            bracket: edh_data::deck::Bracket(bracket),
            floor: edh_data::deck::Bracket(1),
            structure: 0.8,
            synergy: None,
            salt: Some(30.0),
        },
    )
}

/// The browser, driven by keys and read back as the list it shows.
struct Browser<'a> {
    app: App<'a>,
    /// Where decks "arriving in the background" come from; `None` for a local list.
    arrivals: Option<Sender<FeedEvent>>,
    /// The `-T` counts, for decks that arrive.
    counts: Vec<CompiledFilter>,
}

/// `-T` arguments, as `scout` takes them in: every one counted in the decklist, and the columns
/// for those counts.
fn counting(world: &World, args: &[&str]) -> Vec<CompiledFilter> {
    let parsed: Vec<Filter> = args.iter().map(|a| a.parse().unwrap()).collect();
    let (_, cards) = filters::split(parsed);
    cards.into_iter().map(|f| f.compile(&world.cards, None).unwrap()).collect()
}

impl<'a> Browser<'a> {
    /// One commander's decks, `(id, power)`, with a deck database (so favorites work) and more
    /// decks still arriving.
    fn open(world: &'a World, decks: &[(&str, f64)]) -> Self {
        let decks = decks.iter().map(|(id, power)| scored(id, *power, "Alpha One")).collect();
        Self::build(world, decks, true, false, &[])
    }

    /// One commander's decks, `(id, cards)`, scouted with these `-T` arguments (`equipment=1+`).
    /// The decks are taken as already picked: EDHREC's counts aren't checked against them.
    fn open_counting(world: &'a World, args: &[&str], decks: &[(&str, &[&str])]) -> Self {
        let decks = decks.iter().map(|(id, cards)| scored_with(id, 6.0, "Alpha One", cards)).collect();
        Self::build(world, decks, true, false, args)
    }

    /// A search across commanders: `(id, power, commander)`.
    fn open_search(world: &'a World, decks: &[(&str, f64, &str)]) -> Self {
        let decks = decks.iter().map(|(id, power, commander)| scored(id, *power, commander)).collect();
        Self::build(world, decks, true, true, &[])
    }

    /// A local list, as `check` shows it: no deck database, nothing arriving.
    fn open_local(world: &'a World, decks: &[(&str, f64)]) -> Self {
        let decks = decks.iter().map(|(id, power)| scored(id, *power, "Alpha One")).collect();
        Self::build(world, decks, false, false, &[])
    }

    fn build(world: &'a World, decks: Vec<(Deck, Score)>, stored: bool, across: bool, args: &[&str]) -> Self {
        let counts = counting(world, args);
        let count_columns = filters::count_columns(&counts);
        let mut pool =
            Pool::new(&world.cards, &world.combos, HashMap::new(), PoolStats::default()).with_filters(counts.clone());
        if stored {
            let store = world.store();
            let favorites = store.db().favorites().unwrap();
            pool = pool.with_store(store, 1).with_favorites(favorites);
        }
        for (deck, score) in decks {
            let matches = counts.iter().map(|f| f.count(&deck, &world.cards)).collect();
            pool.decks.push(Entry { deck, score, matches });
        }
        let settings = Settings {
            title: "test".into(),
            target: Target::new(edh_data::deck::BracketRange::new(2, 3), None),
            mode: RankMode::Fit,
            weights: Weights::default(),
            export_dir: world.dir.path().join("export"),
            format: ExportFormat::Plain,
            report_top: 5,
            show_price: false,
            show_commander: across,
            mix: across,
            count_columns,
            layout: PaneLayout::Auto,
            source: (!stored).then(|| "local decklist".to_string()),
            notice: filters::count_only_notice(counts.iter().map(|f| &f.filter)),
            // Enough to play every deck, few enough to be quick.
            goldfish_games: 200,
        };
        let (arrivals, feed) = if stored {
            let (tx, feed) = Feed::for_test();
            (Some(tx), Some(feed))
        } else {
            (None, None)
        };
        Self { app: App::new(settings, pool, feed, None), arrivals, counts }
    }

    /// Press keys, separated by spaces: a character (`*`, `F`, `j`), or `down`, `up`, `enter`,
    /// `esc`, `home`, `end`.
    fn press(&mut self, keys: &str) -> &mut Self {
        for key in keys.split_whitespace() {
            let code = match key {
                "down" => KeyCode::Down,
                "up" => KeyCode::Up,
                "enter" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                c if c.chars().count() == 1 => KeyCode::Char(c.chars().next().unwrap()),
                other => panic!("no such key in the driver: {other}"),
            };
            assert!(self.app.handle(KeyEvent::new(code, KeyModifiers::NONE)), "`{key}` closed the browser");
        }
        self
    }

    /// A deck arrives in the background, already scored, and the browser takes it in.
    fn arrive(&mut self, id: &str, power: f64) -> &mut Self {
        let (deck, score) = scored(id, power, "Alpha One");
        let matches = self.counts.iter().map(|f| f.count(&deck, self.app.pool.cards())).collect();
        let event = FeedEvent::Scored { deck: Box::new(deck), scored: Some((score, matches)), from: Origin::Fetched };
        self.arrivals.as_ref().expect("a local list has nothing arriving").send(event).unwrap();
        self.app.pump();
        self
    }

    /// The decks the list shows, in order.
    fn ids(&self) -> Vec<String> {
        self.app.visible.iter().map(|(_, r)| self.app.pool.decks[r.index].deck.id.clone()).collect()
    }

    /// The decks the list shows with a ★, in order.
    fn starred(&self) -> Vec<String> {
        let shown = self.app.visible.iter().filter(|(_, r)| self.app.pool.is_favorite(r.index));
        shown.map(|(_, r)| self.app.pool.decks[r.index].deck.id.clone()).collect()
    }

    /// The deck the cursor is on.
    fn selected(&self) -> Option<String> {
        self.app.selected().map(|(_, r)| self.app.pool.decks[r.index].deck.id.clone())
    }

    /// The selected deck's position in the ranking, as its `#` column shows it.
    fn selected_position(&self) -> Option<usize> {
        self.app.selected().map(|(pos, _)| pos + 1)
    }

    /// The headers of the columns the `-T` arguments added to the list, left to right.
    fn count_columns(&mut self) -> Vec<String> {
        let text = self.text(240, 30);
        let header = text.iter().find(|l| l.contains("Power") && l.contains("Struct")).expect("the list's header");
        let list: Vec<String> = header.split('│').nth(1).unwrap().split_whitespace().map(String::from).collect();
        let after = |name: &str| list.iter().position(|c| c == name).unwrap_or_else(|| panic!("no {name} column"));
        list[after("Syn") + 1..after("Themes")].to_vec()
    }

    /// The colours of the marks in front of `card` in the decklist (open it first), left to right.
    fn marks(&mut self, card: &str) -> Vec<Color> {
        let buf = self.screen(240, 30);
        let text = lines(&buf);
        let (y, line) = text.iter().enumerate().find(|(_, l)| l.contains(card)).expect("the card in the decklist");
        // Every cell here holds one character, so a character's index is its column.
        let start = line[..line.find(card).unwrap()].chars().count();
        let quantity = line.chars().take(start - 1).collect::<Vec<_>>();
        let gutter = quantity.iter().rposition(|c| !c.is_ascii_digit()).map_or(0, |x| x + 1);
        let mut marks = Vec::new();
        for x in (0..gutter).rev() {
            match buf[(x as u16, y as u16)].symbol() {
                "▸" => marks.push(buf[(x as u16, y as u16)].fg),
                " " => {}
                _ => break,
            }
        }
        marks.reverse();
        marks
    }

    /// What the details say about the selected deck's mana, once the goldfish has played it: the
    /// lines from "Mana" on, as one string.
    fn mana(&mut self) -> String {
        self.app.refresh_details();
        if let Some(m) = self.app.mana.as_mut() {
            m.wait();
        }
        let text = self.text(240, 60);
        let at = text.iter().position(|l| l.contains("Mana ")).expect("a Mana line in the details");
        let mut out = vec![text[at].clone()];
        out.extend(text[at + 1..].iter().take_while(|l| l.contains("stuck on colours")).cloned());
        out.iter()
            .map(|l| l.split('│').filter(|s| s.contains("Mana") || s.contains("stuck")).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The message in the status line, if there is one.
    fn status(&self) -> String {
        self.app.status.as_ref().map(|(msg, _)| msg.clone()).unwrap_or_default()
    }

    /// The screen at `width`×`height`.
    fn screen(&mut self, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| self.app.draw(f)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The screen's text at `width`×`height`, a string per line.
    fn text(&mut self, width: u16, height: u16) -> Vec<String> {
        lines(&self.screen(width, height))
    }
}

fn lines(buf: &Buffer) -> Vec<String> {
    let area = buf.area;
    (0..area.height).map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect()).collect()
}

fn shows(text: &[String], what: &str) -> bool {
    text.iter().any(|l| l.contains(what))
}

#[test]
fn a_deck_marked_as_favorite_leads_the_list_with_the_cursor_on_it() {
    let w = World::new();
    let mut b = Browser::open(&w, &[("a", 6.2), ("b", 6.0), ("c", 5.8), ("d", 5.6), ("x", 9.5)]);
    assert_eq!(b.ids(), ["a", "b", "c", "d", "x"]);
    b.press("j j j *");
    assert_eq!(b.ids(), ["d", "a", "b", "c", "x"], "the marked deck should lead, the rest keep their order");
    assert_eq!(b.selected().as_deref(), Some("d"), "the cursor should move up with the marked deck");
    assert_eq!(b.starred(), ["d"]);
    assert!(b.status().contains("favorites"), "status: {}", b.status());
    // A favorite outside the target still comes before the other decks, after the favorites in it.
    b.press("end *");
    assert_eq!(b.ids(), ["d", "x", "a", "b", "c"]);
    // Unmarked, a deck goes back to its place, and the cursor goes with it.
    b.press("home *");
    assert_eq!(b.ids(), ["x", "a", "b", "c", "d"]);
    assert_eq!(b.selected().as_deref(), Some("d"));
    assert!(b.status().contains("removed"), "status: {}", b.status());
}

#[test]
fn the_details_say_how_the_selected_decks_mana_develops() {
    let w = World::new();
    let mut b = Browser::open(&w, &[("a", 6.2), ("b", 6.0)]);
    let a = b.mana();
    assert!(a.contains("commander by T4") && a.contains("mana on T4"), "deck a's details: {a}");
    b.press("j");
    let next = b.mana();
    assert!(next.contains("mana on T4"), "the next deck gets its own: {next}");
}

#[test]
fn favorites_are_still_marked_the_next_time_the_browser_opens() {
    let w = World::new();
    let decks = [("a", 6.2), ("b", 6.0)];
    Browser::open(&w, &decks).press("j *");
    let b = Browser::open(&w, &decks);
    assert_eq!(b.ids(), ["b", "a"]);
    assert_eq!(b.starred(), ["b"]);
}

#[test]
fn capital_f_shows_only_favorites_and_f_narrows_them_to_the_target() {
    let w = World::new();
    let mut b = Browser::open(&w, &[("a", 6.2), ("b", 6.0), ("c", 5.8), ("x", 9.5)]);
    b.press("F");
    assert!(b.ids().is_empty());
    assert!(b.status().contains("no favorites"), "status: {}", b.status());
    b.press("F j * end *");
    assert_eq!(b.starred(), ["b", "x"]);
    b.press("F");
    assert_eq!(b.ids(), ["b", "x"]);
    b.press("f");
    assert_eq!(b.ids(), ["b"]);
    b.press("f F");
    assert_eq!(b.ids(), ["b", "x", "a", "c"]);
}

#[test]
fn unmarking_a_deck_while_only_favorites_show_hands_its_row_to_the_next() {
    let w = World::new();
    let mut b = Browser::open(&w, &[("a", 6.2), ("b", 6.0), ("c", 5.8)]);
    b.press("j * end * F home");
    assert_eq!(b.ids(), ["b", "c"]);
    b.press("*");
    assert_eq!(b.ids(), ["c"]);
    assert_eq!(b.selected().as_deref(), Some("c"));
    b.press("*");
    assert!(b.ids().is_empty() && b.selected().is_none());
    assert!(shows(&b.text(160, 30), "None of these decks is a favorite yet"), "the empty list should say why");
}

#[test]
fn when_no_favorite_is_in_the_target_the_list_says_so() {
    let w = World::new();
    let mut b = Browser::open(&w, &[("a", 6.2), ("x", 9.5)]);
    b.press("end * F f");
    assert!(b.ids().is_empty());
    assert!(shows(&b.text(160, 30), "None of the favorites is in the target"));
}

#[test]
fn a_local_list_has_no_favorites() {
    let w = World::new();
    let mut b = Browser::open_local(&w, &[("mine", 6.2)]);
    b.press("* F");
    assert!(b.starred().is_empty());
    assert_eq!(b.ids(), ["mine"]);
    let footer = b.text(200, 20).pop().unwrap();
    assert!(!footer.contains("favorite"), "the key hints shouldn't offer favorites: {footer}");
}

#[test]
fn the_cursor_stays_on_its_deck_while_decks_arrive_and_the_ranking_changes() {
    let w = World::new();
    let mut b = Browser::open(&w, &[("a", 6.2), ("b", 6.0), ("c", 5.8)]);
    b.press("j");
    b.arrive("n", 6.25);
    assert_eq!(b.ids(), ["a", "n", "b", "c"], "the arrival should slot in by its score");
    assert_eq!(b.selected().as_deref(), Some("b"));
    b.press("r");
    assert!(b.status().contains("power"), "status: {}", b.status());
    assert_eq!(b.ids(), ["n", "a", "b", "c"], "ranked by power, highest first");
    assert_eq!(b.selected().as_deref(), Some("b"));
}

#[test]
fn f_shows_only_decks_in_the_target_keeping_the_cursor_where_it_can() {
    let w = World::new();
    let mut b = Browser::open(&w, &[("a", 6.2), ("x", 9.5), ("b", 6.0)]);
    assert_eq!(b.ids(), ["a", "b", "x"]);
    b.press("j f");
    assert_eq!(b.ids(), ["a", "b"]);
    assert_eq!(b.selected().as_deref(), Some("b"));
    // The deck the cursor was on is hidden: the cursor goes to the top.
    b.press("f end f");
    assert_eq!(b.selected().as_deref(), Some("a"));
}

#[test]
fn enter_switches_between_the_details_and_the_decklist() {
    let w = World::new();
    let mut b = Browser::open(&w, &[("a", 6.2)]);
    assert!(!shows(&b.text(160, 30), "1 Grizzly Bears"));
    b.press("enter");
    assert!(shows(&b.text(160, 30), "1 Grizzly Bears"), "the decklist should show the deck's cards");
    b.press("esc");
    assert!(!shows(&b.text(160, 30), "1 Grizzly Bears"), "esc should go back to the details");
}

#[test]
fn m_switches_between_mixing_commanders_and_ranking_by_score_keeping_the_cursor() {
    let w = World::new();
    let decks = [("a1", 6.2, "Alpha One"), ("a2", 6.1, "Alpha One"), ("a3", 6.0, "Alpha One"), ("b1", 5.0, "Beta Two")];
    let mut b = Browser::open_search(&w, &decks);
    assert_eq!(b.ids(), ["a1", "b1", "a2", "a3"], "mixed: Beta Two's deck comes early although it scores lower");
    b.press("j j m");
    assert_eq!(b.ids(), ["a1", "a2", "a3", "b1"]);
    assert_eq!(b.selected().as_deref(), Some("a2"));
}

#[test]
fn every_t_gets_a_count_column_in_the_order_given() {
    let w = World::new();
    let deck: &[&str] = &["Bonesplitter", "Lizard Blades", "Jace Beleren", "Grizzly Bears"];
    let args = ["equipment=1+", "planeswalker=1+", "creatures=..5", "ramp", "bear=1", "lands=0"];
    let mut b = Browser::open_counting(&w, &args, &[("a", deck)]);
    assert_eq!(
        b.count_columns(),
        ["equipment", "walkers", "creatures", "ramp"],
        "EDHREC's counts (planeswalker, creatures) need a column like any other -T; only those that \
         let a single count through (bear=1, lands=0) go without, since every deck shown has it"
    );
    let details = b.text(240, 30);
    assert!(
        shows(&details, "walkers    1   planeswalkers  Jace Beleren"),
        "the details should list the planeswalkers counted: {details:#?}"
    );
    assert!(
        shows(&details, "creatures  2   Grizzly Bears · Lizard Blades"),
        "counted as EDHREC files cards, an artifact creature is a creature: {details:#?}"
    );
}

#[test]
fn a_t_that_only_counts_though_it_looks_like_a_filter_says_so() {
    let w = World::new();
    let deck: &[&str] = &["Bonesplitter", "Grizzly Bears"];
    let mut b = Browser::open_counting(&w, &["mv<=2"], &[("a", deck)]);
    assert!(
        b.status().starts_with("-T mv<=2 only counts"),
        "-T mv<=2 has no range (its = is part of the search), so it lets every deck through; opening \
         should say so, not leave it to the title's \"any\": {:?}",
        b.status()
    );
    let screen = b.text(80, 20);
    assert!(shows(&screen, "-T mv<=2 only counts"), "the status line should show it: {screen:#?}");
    for args in [&["equipment"][..], &["mv<=2=1+"][..]] {
        let b = Browser::open_counting(&w, args, &[("a", deck)]);
        assert_eq!(b.status(), "", "-T {args:?} needs no note");
    }
}

#[test]
fn the_decklist_marks_each_filters_cards_in_a_colour_of_its_own() {
    let w = World::new();
    let deck: &[&str] = &["Bonesplitter", "Lizard Blades", "Sol Ring", "Grizzly Bears"];
    let mut b = Browser::open_counting(&w, &["equipment=1+", "artifacts=1+"], &[("a", deck)]);
    b.press("enter");
    let (equipment, artifact) = (b.marks("Lizard Blades"), b.marks("Sol Ring"));
    assert_eq!((equipment.len(), artifact.len()), (1, 1), "a card one filter counted has one mark");
    assert_ne!(equipment, artifact, "the two filters' cards should be told apart by colour");
    assert_eq!(
        b.marks("Bonesplitter"),
        [equipment[0], artifact[0]],
        "a card both filters count should carry both marks, in the filters' order"
    );
    assert!(b.marks("Grizzly Bears").is_empty(), "a card no filter counts has no mark");
}

/// On the selected row, a stretch of blank cells styled unlike the row is a block of colour; it's
/// only right as the padding of text styled the same next to it (like a right-aligned number).
fn colour_without_text(buf: &Buffer, y: u16) -> Option<String> {
    let width = buf.area.width;
    let start = (0..width).find(|&x| buf[(x, y)].symbol() == "▶")?;
    let end = (start + 1..width).find(|&x| buf[(x, y)].symbol() == "│").unwrap_or(width);
    let row = buf[(start + 1, y)].style();
    let blank = |x: u16| buf[(x, y)].symbol().trim().is_empty();
    let mut x = start + 1;
    while x < end {
        let style = buf[(x, y)].style();
        if !blank(x) || style == row {
            x += 1;
            continue;
        }
        let from = x;
        while x < end && blank(x) && buf[(x, y)].style() == style {
            x += 1;
        }
        let text_beside = |n: u16| n > start && n < end && !blank(n) && buf[(n, y)].style() == style;
        if !text_beside(from - 1) && !text_beside(x) {
            return Some(format!("columns {from}..{x} are coloured ({style:?}) with nothing in them"));
        }
    }
    None
}

#[test]
fn the_screen_holds_together_at_every_size() {
    let w = World::new();
    // Enough decks for four-digit positions, some outside the target, two favorites.
    let ids: Vec<String> = (0..1200).map(|i| format!("d{i:04}")).collect();
    let decks: Vec<(&str, f64)> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), if i % 7 == 0 { 9.0 } else { 4.8 + (i % 29) as f64 * 0.1 }))
        .collect();
    Browser::open(&w, &decks).press("j * j j *");
    // Each state starts from a fresh browser; `x` (no key) clears the status line, so the key
    // hints show.
    let states = ["", "j j j", "end", "enter", "F", "f", "F f", "end enter", "F end"];
    // Narrower than this, the list's columns don't fit (today from 35 columns) and get squeezed:
    // there the screen only has to draw and offer quit.
    const LIST_FITS: u16 = 40;
    let sizes = [12, 30, 40, 60, 80, 109, 110, 160, 240].into_iter().flat_map(|w| [3, 5, 12, 24, 60].map(|h| (w, h)));
    let mut problems = Vec::new();
    for keys in states {
        let mut b = Browser::open(&w, &decks);
        b.press(&format!("{keys} x"));
        for (width, height) in sizes.clone() {
            let at = format!("after `{keys}` at {width}x{height}");
            let buf = b.screen(width, height);
            let text = lines(&buf);
            if !text.last().unwrap().contains("q quit") {
                problems.push(format!("{at}: the key hints don't offer quit"));
            }
            if width < LIST_FITS {
                continue;
            }
            let Some(y) = text.iter().position(|l| l.contains('▶')) else { continue };
            let shown = text[y].split('▶').nth(1).unwrap();
            let first_number = shown.split(|c: char| !c.is_ascii_digit()).find(|t| !t.is_empty());
            let position = b.selected_position().unwrap().to_string();
            if first_number != Some(position.as_str()) {
                problems.push(format!("{at}: the selected row shows {first_number:?} for position {position}"));
            }
            if let Some(problem) = colour_without_text(&buf, y as u16) {
                problems.push(format!("{at}: on the selected row, {problem}"));
            }
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}
