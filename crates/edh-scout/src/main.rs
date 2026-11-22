use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use edh_data::cards::CardDb;
use edh_data::combos::ComboDb;
use edh_data::config::Paths;
use edh_data::deck::{BracketRange, Deck, parse_decklist};
use edh_data::edhrec::{self, ListedCommander};
use edh_data::forge::ForgeSource;
use edh_data::http::{Http, OfflineMiss, summarize_counts};
use edh_data::progress::{Lines, Progress};
use edh_data::sync::{SyncWhat, ensure_databases, sync};
use edh_data::tags::TagIndex;
use edh_data::text::{commander_slug, date_days_ago, front_face, slugify};
use edh_scout::analysis::score_epoch;
use edh_scout::config::{Config, PaneLayout};
use edh_scout::export::{self, ExportFormat, ReportInput};
use edh_scout::filters::{self, CardFilter, CompiledFilter, CountColumn, Filter};
use edh_scout::rank::{RankMode, Target, Weights, hoist, mix, parse_range, rank};
use edh_scout::scout::{
    Feed, Fetcher, Pool, PoolStats, Prepared, ScoutOptions, SearchOptions, load_commander_sources, load_salt, prepare,
};
use edh_scout::select::SelectOptions;
use edh_scout::status::Status;
use edh_scout::{mana, picker, plain, tui, view};
use edh_store::{DeckStore, Retired};

/// Find well-rounded, fun Commander decklists on EDHREC.
///
/// Decks are scored with EDHPowerLevel's power level and bracket (computed locally), supplemented
/// by EDHREC's salt score, a deckbuilding-template check and synergy with the commander.
#[derive(Parser)]
#[command(name = "edh-scout", version, max_term_width = 100)]
struct Cli {
    /// Never touch the network: use only cached pages and the local databases.
    #[arg(long, global = true)]
    offline: bool,
    /// Where downloads and the cache live [default: $XDG_CACHE_HOME/edh-scout].
    #[arg(long, global = true, value_name = "DIR")]
    cache_dir: Option<PathBuf>,
    /// Where fetched decklists are kept [default: $XDG_STATE_HOME/edh-scout].
    #[arg(long, global = true, value_name = "DIR")]
    state_dir: Option<PathBuf>,
    /// Config file [default: $XDG_CONFIG_HOME/edh-scout/config.toml, or $EDH_SCOUT_CONFIG].
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once at startup
enum Command {
    /// Pull EDHREC decklists for a commander and rank them for your table.
    Scout(ScoutArgs),
    /// Browse the decks you marked as favorites (`*` in the browser), across commanders.
    Favorites(FavoritesArgs),
    /// Score a local decklist file.
    Check(CheckArgs),
    /// Download/refresh the card (Scryfall) and combo (Commander Spellbook) databases.
    Sync(SyncArgs),
}

#[derive(Args)]
struct TargetArgs {
    /// Target Commander brackets, e.g. 3 or 2-3.
    #[arg(short, long, default_value = "2-3", value_name = "RANGE")]
    brackets: String,
    /// Target EDHPowerLevel score band, e.g. 5.5-7 [default: derived from --brackets].
    #[arg(short, long, value_name = "RANGE")]
    power: Option<String>,
}

#[derive(Args)]
struct ScoutArgs {
    /// Commander name. Partial names work; if several commanders match you get to pick.
    /// May be left out when -T filters are given: then decks are searched across commanders.
    #[arg(required_unless_present = "types")]
    commander: Option<String>,
    /// Second commander (partner, background, ...).
    #[arg(long, requires = "commander")]
    partner: Option<String>,
    /// Override the EDHREC page slug, e.g. kraum-ludevics-opus-tymna-the-weaver.
    #[arg(long, requires = "commander")]
    slug: Option<String>,
    /// How many cards of a kind, repeatable; all must match. EDHREC's counts are free and checked
    /// before fetching: creatures, instants, sorceries, spells, artifacts, enchantments,
    /// planeswalkers, battles, lands (planeswalkers=10+, creatures=..5). Anything else is counted
    /// in the 99 once a deck is fetched: a type-line word (equipment=10+, legendary=25+), a role
    /// (ramp=15+, draw, removal, wipes) or a Scryfall search ('o:"create a treasure"=6+',
    /// 'otag:sweeper=4+'). Ranges: 10+, ..5, 25-40, 36; without one (-T equipment) a decklist
    /// count only adds a column (so does c=w, whose = is the search's: c=w=1+ filters). With a commander it narrows that commander's decks; without one
    /// it searches across commanders.
    #[arg(short = 'T', long = "type", value_name = "WHAT[=RANGE]")]
    types: Vec<String>,
    /// Searching across commanders: how many commanders' deck tables to scan first (1 request
    /// each). More are scanned, a few at a time, when too few of them have matching decks.
    #[arg(long, default_value_t = 30, value_name = "N")]
    commanders: usize,
    /// Searching across commanders: how many of a commander's decks may fail the card filters,
    /// none passing, before it counts as a miss; too many misses widen the search. Higher gives
    /// each commander longer and widens later [default: 3, or the config's search.miss_after].
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    miss_after: Option<u32>,
    /// Searching across commanders: how many decks to fetch between one widening of the search
    /// and the next; higher widens more slowly, 0 never [default: 5, or the config's
    /// search.widen_every].
    #[arg(long, value_name = "N")]
    widen_every: Option<usize>,
    #[command(flatten)]
    target: TargetArgs,
    /// How to rank decks.
    #[arg(short, long, value_enum, default_value_t = RankMode::Fit)]
    rank: RankMode,
    /// Searching across commanders: rank by score alone, instead of interleaving commanders so
    /// one with many decks doesn't fill the list (`m` toggles this in the browser).
    #[arg(long)]
    no_mix: bool,
    /// Weights for --rank combined.
    #[arg(long, default_value = "fit=0.5,salt=0.2,structure=0.15,synergy=0.15")]
    weights: String,
    /// New decklists to fetch this run (decks fetched before are reused for free).
    #[arg(short = 'n', long, default_value_t = 60, value_name = "N")]
    fetch: usize,
    /// Seconds between deck page requests to EDHREC (minimum 2).
    #[arg(long, default_value_t = 2.0, value_name = "SECS")]
    deck_delay: f64,
    /// Skip decks priced above this (USD, EDHREC's estimate).
    #[arg(long, value_name = "USD")]
    max_price: Option<f64>,
    /// Skip decks with an EDHREC salt score above this.
    #[arg(long, value_name = "SALT")]
    max_salt: Option<f64>,
    /// Only decks tagged with this EDHREC theme (repeatable).
    #[arg(short, long = "tag", value_name = "THEME")]
    tags: Vec<String>,
    /// Only decks whose owner declared a bracket in the target range.
    #[arg(long)]
    declared_only: bool,
    /// Only decks saved since this date (YYYY-MM-DD) or within this many days.
    #[arg(long, value_name = "DATE|DAYS")]
    since: Option<String>,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Args)]
struct OutputArgs {
    /// Print a table instead of opening the interactive browser.
    #[arg(long)]
    plain: bool,
    /// Decks to list in plain output and in exported reports.
    #[arg(long, default_value_t = 20, value_name = "N")]
    top: usize,
    /// Write report.md and the top decklists here (in the browser: where `e`/`E` export to).
    #[arg(short, long, value_name = "DIR")]
    export: Option<PathBuf>,
    /// Decklist file format for exports.
    #[arg(long, value_enum, default_value_t = ExportFormat::Plain)]
    format: ExportFormat,
    /// Show EDHREC's deck prices, and let price help pick which decks to fetch. Off by default:
    /// a deck's price is usually one or two cards and says little about how it plays.
    #[arg(long)]
    price: bool,
}

#[derive(Args)]
struct FavoritesArgs {
    #[command(flatten)]
    target: TargetArgs,
    /// How to rank decks.
    #[arg(short, long, value_enum, default_value_t = RankMode::Fit)]
    rank: RankMode,
    /// Interleave commanders so one with many favorites doesn't fill the list (`m` toggles this
    /// in the browser).
    #[arg(long)]
    mix: bool,
    /// Weights for --rank combined.
    #[arg(long, default_value = "fit=0.5,salt=0.2,structure=0.15,synergy=0.15")]
    weights: String,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Args)]
struct CheckArgs {
    /// Decklist file (plain, MTGA, Moxfield or Archidekt text export).
    file: PathBuf,
    /// Commander, if the file doesn't mark one (repeat for partners).
    #[arg(short, long = "commander")]
    commanders: Vec<String>,
    /// Count cards of a kind in the list, repeatable: equipment, ramp, 'otag:sweeper',
    /// 'o:"create a treasure"' (as for scout -T). With a range (equipment=10+) the count is
    /// checked against it.
    #[arg(short = 'T', long = "type", value_name = "WHAT[=RANGE]")]
    types: Vec<String>,
    #[command(flatten)]
    target: TargetArgs,
    /// Print instead of opening the interactive browser.
    #[arg(long)]
    plain: bool,
    /// Show EDHREC's deck price.
    #[arg(long)]
    price: bool,
}

#[derive(Args)]
struct SyncArgs {
    /// Only the card database.
    #[arg(long, conflicts_with = "combos")]
    cards: bool,
    /// Only the combo database.
    #[arg(long, conflicts_with = "forge")]
    combos: bool,
    /// Only Forge's card scripts, for the mana goldfish (skipped when a directory of them is
    /// configured: `[goldfish] forge_cards` or EDH_SCOUT_FORGE_CARDS).
    #[arg(long, conflicts_with = "cards")]
    forge: bool,
    /// Rebuild from the downloads already on disk instead of fetching new ones.
    #[arg(long)]
    rebuild: bool,
}

fn target_from(args: &TargetArgs) -> Result<Target> {
    let brackets = BracketRange::parse(&args.brackets).context("--brackets")?;
    let band = args.power.as_deref().map(parse_range::<f64>).transpose().context("--power")?;
    Ok(Target::new(brackets, band))
}

fn interactive(plain: bool) -> bool {
    !plain && std::io::stdout().is_terminal()
}

/// Columns to wrap plain output to: the terminal's, or none when stdout is piped.
fn terminal_width() -> Option<usize> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    ratatui::crossterm::terminal::size().ok().map(|(w, _)| w as usize)
}

fn color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// An exact or unambiguous name resolves directly; otherwise the user picks interactively (or, when
/// there's no terminal to ask on, gets the candidates in the error).
fn resolve_commander(cards: &CardDb, query: &str, role: &str, status: &Status) -> Result<String> {
    let candidates = match cards.find_commander(query) {
        Ok(c) => return Ok(c.name.clone()),
        Err(candidates) => candidates,
    };
    if picker::can_pick() {
        return match status.suspend(|| picker::pick(cards, role, query))? {
            Some(c) => {
                status.note(&format!("{role}: {}", c.name));
                Ok(c.name.clone())
            }
            None => bail!("no {role} picked"),
        };
    }
    if candidates.is_empty() {
        bail!("no {role} matches '{query}'");
    }
    let names: Vec<&str> = candidates.iter().take(15).map(|c| c.name.as_str()).collect();
    let more = candidates.len().saturating_sub(names.len());
    bail!(
        "'{query}' matches {} {role}s; run in a terminal to pick one, or be more specific:\n  {}{}",
        candidates.len(),
        names.join("\n  "),
        if more > 0 { format!("\n  … and {more} more") } else { String::new() }
    )
}

/// How long the browser waits for stored decks to be scored (after a `sync`, or the first time)
/// before it opens with the decks that are ready; the rest join the list as they're done.
const START_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// A scout in progress: decks so far, and the background work still filling them in.
struct Session<'a> {
    title: String,
    /// Default export directory name (`edh-scout-<name>`).
    name: String,
    pool: Pool<'a>,
    fetcher: Option<Fetcher>,
    /// Stored decks still to be scored, best first.
    backlog: Vec<Deck>,
    show_commander: bool,
    /// Columns for the `-T` card filters' counts.
    count_columns: Vec<CountColumn>,
    /// Where the decks came from, when that's not EDHREC's deck tables (see `tui::Settings`).
    source: Option<String>,
}

/// How a session is ranked and shown: what `scout` and `favorites` share.
struct Show<'a> {
    rank: RankMode,
    weights: Weights,
    /// Interleave commanders (when there are several).
    mix: bool,
    output: &'a OutputArgs,
    layout: PaneLayout,
    /// For the browser's status line on opening (see [`filters::count_only_notice`]).
    notice: Option<String>,
    /// Games the mana goldfish plays per deck (`[goldfish] games`).
    goldfish_games: u32,
}

fn run_scout(http: &Arc<Http>, paths: &Paths, config: &Config, args: ScoutArgs) -> Result<()> {
    let target = target_from(&args.target)?;
    // Only the browser keeps fetching past the budget: plain output waits for the fetcher.
    let keep_fetching = config.tui.keep_fetching && interactive(args.output.plain) && !http.offline();
    let weights: Weights = args.weights.parse().context("--weights")?;
    let since = match &args.since {
        Some(s) if s.chars().all(|c| c.is_ascii_digit()) => Some(date_days_ago(s.parse()?)),
        Some(s) if s.len() == 10 && s.as_bytes()[4] == b'-' => Some(s.clone()),
        Some(s) => bail!("--since takes YYYY-MM-DD or a number of days, got '{s}'"),
        None => None,
    };
    let parsed: Vec<Filter> = args.types.iter().map(|t| t.parse()).collect::<Result<_>>().context("-T")?;
    let (types, card_filters) = filters::split(parsed);
    let notice = filters::count_only_notice(&card_filters);
    let status = Status::new(Some(http));
    if let Some(n) = &notice {
        status.note(&format!("note: {n}"));
    }
    let (cards, combos) = ensure_databases(http, paths, &status)?;
    let card_filters = compile_filters(card_filters, &cards, paths, &status)?;
    http.set_min_gap("edhrec.com", std::time::Duration::from_secs_f64(args.deck_delay.max(2.0)));
    let select = SelectOptions {
        brackets: target.brackets,
        declared_only: args.declared_only,
        max_price: args.max_price,
        price_ceiling: args.output.price,
        max_salt: args.max_salt,
        tags: args.tags.clone(),
        since,
        types,
        priors: card_filters.iter().filter_map(|f| f.prior(&cards)).collect(),
        budget: args.fetch,
    };
    let store = open_store(paths, &status)?;
    status.step("loading EDHREC's salt scores");
    let salt = load_salt(http);
    let session = match &args.commander {
        Some(name) => start_single(
            http,
            &store,
            &cards,
            &combos,
            salt,
            &args,
            name,
            &target,
            &weights,
            select,
            card_filters,
            keep_fetching,
            &status,
        )?,
        None => start_search(
            http,
            &store,
            &cards,
            &combos,
            salt,
            &target,
            select,
            card_filters,
            SearchOptions {
                scan: args.commanders.max(1),
                miss_after: args.miss_after.map_or(config.search.miss_after, |n| n as usize),
                widen_every: args.widen_every.unwrap_or(config.search.widen_every),
                keep_going: keep_fetching,
            },
            &status,
        )?,
    };
    drop(status);
    let show = Show {
        rank: args.rank,
        weights,
        mix: !args.no_mix,
        output: &args.output,
        layout: config.tui.layout,
        notice,
        goldfish_games: config.goldfish.games,
    };
    finish(http, session, &show, target)
}

/// The deck database. On the first run after upgrading, the deck pages kept in the state
/// directory are read into it and moved to the cache; if the database is ever deleted, it is
/// rebuilt from the pages still in the cache.
fn open_store(paths: &Paths, status: &Status) -> Result<DeckStore> {
    status.step("opening the deck database");
    let store = DeckStore::open(&paths.decks_db(), paths.pages()).context("opening the deck database")?;
    let old = paths.old_decks();
    let from = if old.exists() {
        Some(old.clone())
    } else if store.db().is_empty() {
        Some(paths.pages())
    } else {
        None
    };
    let Some(from) = from else {
        store.reparse(status)?;
        return Ok(store);
    };
    let done = store.import(&from, status).context("reading stored deck pages into the deck database")?;
    if done.decks > 0 {
        status.note(&format!("  read {} stored deck pages into the deck database (once)", done.decks));
    }
    if done.failed > 0 {
        status.note(&format!(
            "  note: {} stored deck pages couldn't be read ({})",
            done.failed,
            done.first_error.unwrap_or_default()
        ));
    }
    if from == old {
        match store.retire_old_pages(&old)? {
            Retired::Moved => status.note(&format!("  moved the deck pages to {}", store.pages().display())),
            Retired::LeftInPlace { bytes } => status.note(&format!(
                "  note: {:.1} GB of deck pages in {} are no longer needed (every deck is in {}); delete them, or \
                 move them to {} to keep them as a cache",
                bytes as f64 / 1e9,
                old.display(),
                paths.decks_db().display(),
                store.pages().display()
            )),
            Retired::Nothing => {}
        }
    }
    store.reparse(status)?;
    Ok(store)
}

/// Check the `-T` card filters against the card database (and, for `otag:`, Scryfall Tagger's
/// tag tree, read from the raw file `sync` keeps).
fn compile_filters(
    filters: Vec<CardFilter>,
    cards: &CardDb,
    paths: &Paths,
    progress: &dyn Progress,
) -> Result<Vec<CompiledFilter>> {
    let tags = if filters.iter().any(CardFilter::needs_tags) {
        let path = paths.scryfall_tags_raw();
        if !path.exists() {
            bail!("otag: needs Scryfall Tagger's tag file; run `edh-scout sync --cards` once (~6 MB)");
        }
        progress.step("reading Scryfall Tagger's tags");
        Some(TagIndex::load(&path).context("reading Scryfall Tagger's tags")?)
    } else {
        None
    };
    filters.into_iter().map(|f| f.compile(cards, tags.as_ref())).collect::<Result<_>>().context("-T")
}

/// One commander (optionally narrowed by `-T` filters): its table and stored decks now, the
/// rest in the background.
#[allow(clippy::too_many_arguments)]
fn start_single<'a>(
    http: &Arc<Http>,
    store: &DeckStore,
    cards: &'a CardDb,
    combos: &'a ComboDb,
    salt: std::collections::HashMap<String, f64>,
    args: &ScoutArgs,
    name: &str,
    target: &Target,
    weights: &Weights,
    select: SelectOptions,
    card_filters: Vec<CompiledFilter>,
    keep_fetching: bool,
    status: &Status,
) -> Result<Session<'a>> {
    let mut commanders = vec![resolve_commander(cards, name, "commander", status)?];
    if let Some(p) = &args.partner {
        commanders.push(resolve_commander(cards, p, "partner", status)?);
    }
    let slug = args.slug.clone().unwrap_or_else(|| commander_slug(&commanders));
    let mut title = commanders.iter().map(|c| front_face(c)).collect::<Vec<_>>().join(" + ");
    if !select.types.is_empty() || !card_filters.is_empty() {
        title += &format!(" · {}", filters::describe(&select.types, &card_filters));
    }
    status.note(&format!("scouting {title} (edhrec.com/commanders/{slug}) for {} ...", target.describe()));
    let opts = ScoutOptions { commanders, slug: slug.clone(), select };
    let Prepared { decks, sources, mut stats, mut to_fetch, more } = prepare(http, store, &opts, status)?;
    if let Some(c) = stats.ceiling {
        let price = if args.output.price { format!(" / ${:.0}", c.price) } else { String::new() };
        status.note(&format!(
            "  decks capped at salt {:.0}{price} (90th percentile of {} decks declared {})",
            c.salt,
            c.from_decks,
            target.describe().split(" · ").next().unwrap_or("")
        ));
    }
    let count_columns = filters::count_columns(&card_filters);
    if keep_fetching {
        stats.queued = to_fetch.len() + more.len();
    }
    let mut pool = Pool::new(cards, combos, salt, stats)
        .with_filters(card_filters)
        .with_store(store.clone(), score_epoch(cards, combos))
        .with_favorites(store.db().favorites().context("reading the favorites")?);
    pool.add_commander(&slug, sources);
    status.step(&format!("scoring {} stored decks", decks.len()));
    // In the browser, decks that still need scoring after a short wait join the list as they're
    // done; plain output waits for all of them.
    let budget = interactive(args.output.plain).then_some(START_BUDGET);
    let backlog = pool.add_stored(decks, budget, &|s| rank(s, target, args.rank, weights));
    if pool.decks.is_empty() && backlog.is_empty() && to_fetch.is_empty() {
        bail!(if http.offline() {
            "no stored decks to show (offline)".to_string()
        } else if pool.stats.unmatched > 0 {
            format!(
                "none of the {} stored decks passes the card filters, and none are left to fetch",
                pool.stats.unmatched
            )
        } else {
            "no decks matched; loosen the filters".to_string()
        });
    }
    let later = if keep_fetching && !more.is_empty() {
        format!(", then {} more while the browser is open", more.len())
    } else {
        String::new()
    };
    let skipped = if pool.stats.unmatched > 0 {
        format!(" ({} stored ones fail the card filters)", pool.stats.unmatched)
    } else {
        String::new()
    };
    let scoring = if backlog.is_empty() { String::new() } else { format!(", {} more being scored", backlog.len()) };
    status.note(&format!(
        "  {} stored deck(s) ready{skipped}{scoring}, {} to fetch in the background{later} · {}",
        pool.decks.len(),
        to_fetch.len(),
        summarize_counts(&http.request_counts())
    ));
    if keep_fetching {
        to_fetch.extend(more);
    }
    let fetcher = (!to_fetch.is_empty())
        .then(|| Fetcher::spawn(Arc::clone(http), store.clone(), to_fetch, opts.commanders.clone()));
    Ok(Session { title, name: slug, pool, fetcher, backlog, show_commander: false, count_columns, source: None })
}

/// No commander: find decks matching the `-T` filters across the commanders EDHREC lists for the
/// matching themes (or its most-built commanders). Deck tables load in the background too.
#[allow(clippy::too_many_arguments)]
fn start_search<'a>(
    http: &Arc<Http>,
    store: &DeckStore,
    cards: &'a CardDb,
    combos: &'a ComboDb,
    salt: std::collections::HashMap<String, f64>,
    target: &Target,
    select: SelectOptions,
    card_filters: Vec<CompiledFilter>,
    how: SearchOptions,
    status: &Status,
) -> Result<Session<'a>> {
    if select.types.is_empty() && !card_filters.iter().any(|f| f.filter.has_range()) {
        bail!("searching across commanders needs a -T with a range, like equipment=10+ (or name a commander)");
    }
    let mut themes: Vec<String> = filters::themes(&select.types).into_iter().map(String::from).collect();
    if card_filters.iter().any(|f| f.filter.wants_many()) {
        // Card filters name themes by their words (equipment, sagas, legends, ramp): one request
        // for EDHREC's theme list, cached for a week.
        status.step("loading EDHREC's theme list");
        match edhrec::themes(http) {
            Ok(known) => {
                for t in filters::card_themes(&card_filters, &known) {
                    if !themes.contains(&t) {
                        themes.push(t);
                    }
                }
            }
            Err(e) => status.note(&format!("  note: couldn't load EDHREC's theme list ({e:#})")),
        }
    }
    let mut suggested: Vec<ListedCommander> = Vec::new();
    for theme in &themes {
        status.step(&format!("loading EDHREC's '{theme}' theme"));
        match edhrec::theme_commanders(http, theme) {
            Ok(list) => suggested.extend(list),
            Err(e) => status.note(&format!("  note: couldn't load EDHREC's '{theme}' theme ({e:#})")),
        }
    }
    let limit = how.scan;
    let mut seen = std::collections::HashSet::new();
    suggested.retain(|c| seen.insert(c.slug.clone()));
    suggested.sort_by_key(|c| std::cmp::Reverse(c.num_decks));
    let themed_slugs: std::collections::HashSet<String> = seen.clone();
    // Then commanders whose own text mentions what the card filters count (free, from the card
    // database): they fill the slots the themes leave, and stand in for the most-built fallback.
    let mentions = filters::mention_texts(&card_filters);
    // More than `limit`: the ones past it wait in reserve, in case the search needs to widen.
    let mentioning = filters::mentioning_commanders(&mentions, cards, 3 * limit);
    suggested.extend(mentioning.into_iter().filter(|c| seen.insert(c.slug.clone())));
    // A filter that asks for one particular card (`name:aetherspark=1`): EDHREC's page for that
    // card lists the commanders that play it most (one request each, cached for a week).
    let mut sole_cards: Vec<&str> = Vec::new();
    for c in card_filters.iter().filter_map(|f| f.sole_card(cards)) {
        if !sole_cards.contains(&c.name.as_str()) {
            sole_cards.push(&c.name);
        }
    }
    let mut pages = Vec::new();
    let mut paged_cards = Vec::new();
    for name in sole_cards {
        status.step(&format!("loading EDHREC's page for {name}"));
        match edhrec::card_commanders(http, name) {
            Ok(list) => {
                pages.push(list);
                paged_cards.push(name);
            }
            Err(e) => status.note(&format!("  note: couldn't load EDHREC's page for {name} ({e:#})")),
        }
    }
    let playing_slugs: std::collections::HashSet<&str> = pages.iter().flatten().map(|c| c.slug.as_str()).collect();
    // In order: the first `limit` are scanned, the rest kept in reserve.
    let mut candidates = filters::merge_candidates(&pages, &suggested, usize::MAX);
    let first = &candidates[..limit.min(candidates.len())];
    let from_cards = first.iter().filter(|c| playing_slugs.contains(c.slug.as_str())).count();
    let from_themes =
        first.iter().filter(|c| !playing_slugs.contains(c.slug.as_str()) && themed_slugs.contains(&c.slug)).count();
    let from_text = first.len() - from_cards - from_themes;
    let mentioned = mentions.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" or ");
    let parts: Vec<(usize, String)> = [
        (from_cards, format!("that EDHREC lists as playing {}", paged_cards.join(" or "))),
        (from_themes, format!("from EDHREC theme{} {}", if themes.len() > 1 { "s" } else { "" }, themes.join(", "))),
        (from_text, format!("whose text mentions {mentioned}")),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .collect();
    let source = match parts.as_slice() {
        [] => {
            status.step("loading EDHREC's most-built commanders");
            candidates = edhrec::top_commanders(http).context("EDHREC's top commanders")?;
            candidates.retain(|c| seen.insert(c.slug.clone()));
            candidates.sort_by_key(|c| std::cmp::Reverse(c.num_decks));
            "from EDHREC's most-built commanders".to_string()
        }
        [(_, only)] => only.clone(),
        // "from EDHREC theme equipment, plus 1 whose text mentions "equipment","; the card page's
        // part gets its count too, so it doesn't read as all of them: "— 24 that EDHREC lists ..."
        [(n, first), rest @ ..] => {
            let rest = rest.iter().map(|(n, what)| format!(", plus {n} {what}")).collect::<String>();
            if from_cards > 0 { format!("— {n} {first}{rest},") } else { format!("{first}{rest},") }
        }
    };
    let wanted = filters::describe(&select.types, &card_filters);
    let count_columns = filters::count_columns(&card_filters);
    let scan = limit.min(candidates.len());
    let reserve = candidates.len() - scan;
    let reserve = if reserve > 0 && how.widen_every > 0 {
        format!("; {reserve} more in reserve, if too few have matching decks")
    } else {
        String::new()
    };
    status.note(&format!(
        "searching {scan} commanders {source} for decks with {wanted}, {}{reserve} ...",
        target.describe()
    ));
    let stats = PoolStats { commanders_total: scan, ..Default::default() };
    let pool = Pool::new(cards, combos, salt, stats)
        .with_filters(card_filters)
        .with_store(store.clone(), score_epoch(cards, combos))
        .with_favorites(store.db().favorites().context("reading the favorites")?);
    let fetcher =
        Fetcher::spawn_search(Arc::clone(http), store.clone(), candidates, select, SearchOptions { scan, ..how });
    Ok(Session {
        title: format!("decks with {wanted}"),
        name: format!("search-{}", slugify(&wanted)),
        pool,
        fetcher: Some(fetcher),
        backlog: Vec::new(),
        show_commander: true,
        count_columns,
        source: None,
    })
}

/// The favorites, whatever their commander: stored decks only, no deck tables and no fetching.
/// Each commander's synergy and combo votes load in the background (cached for a week).
fn run_favorites(http: &Arc<Http>, paths: &Paths, config: &Config, args: FavoritesArgs) -> Result<()> {
    let target = target_from(&args.target)?;
    let weights: Weights = args.weights.parse().context("--weights")?;
    let status = Status::new(Some(http));
    let (cards, combos) = ensure_databases(http, paths, &status)?;
    let store = open_store(paths, &status)?;
    let ids = store.db().favorites().context("reading the favorites")?;
    if ids.is_empty() {
        bail!("no favorites yet: press * on a deck in the `scout` browser to mark it");
    }
    let found = store.get_many(&ids.iter().map(String::as_str).collect::<Vec<_>>())?;
    let missing = found.iter().filter(|d| d.is_none()).count();
    if missing > 0 {
        status.note(&format!("  note: {missing} favorite(s) are no longer in the deck database"));
    }
    // By commander, in the order each was first marked.
    let mut groups: Vec<(String, Vec<Deck>)> = Vec::new();
    for deck in found.into_iter().flatten() {
        let slug = commander_slug(&deck.commanders);
        match groups.iter_mut().find(|(s, _)| *s == slug) {
            Some((_, decks)) => decks.push(deck),
            None => groups.push((slug, vec![deck])),
        }
    }
    if groups.is_empty() {
        bail!("none of the favorites is in the deck database any more");
    }
    status.step("loading EDHREC's salt scores");
    let salt = load_salt(http);
    let stats = PoolStats { commanders_total: groups.len(), ..Default::default() };
    let pool = Pool::new(&cards, &combos, salt, stats)
        .with_store(store.clone(), score_epoch(&cards, &combos))
        .with_favorites(ids);
    let commanders = groups.len();
    let session = Session {
        title: "favorite decks".into(),
        name: "favorites".into(),
        pool,
        fetcher: Some(Fetcher::spawn_stored(Arc::clone(http), groups)),
        backlog: Vec::new(),
        show_commander: true,
        count_columns: Vec::new(),
        source: Some(format!("favorites across {commanders} commander{}", if commanders == 1 { "" } else { "s" })),
    };
    drop(status);
    let show = Show {
        rank: args.rank,
        weights,
        mix: args.mix,
        output: &args.output,
        layout: config.tui.layout,
        notice: None,
        goldfish_games: config.goldfish.games,
    };
    finish(http, session, &show, target)
}

/// Browse (or print) the session, then report what it cost.
fn finish(http: &Arc<Http>, session: Session, show: &Show, target: Target) -> Result<()> {
    let Session { title, name, mut pool, fetcher, backlog, show_commander, count_columns, source } = session;
    let output = show.output;
    let export_dir = output.export.clone().unwrap_or_else(|| PathBuf::from(format!("edh-scout-{name}")));
    let scorer = pool.scorer();
    // Decks are scored on a thread of their own (in this scope, so it can borrow the databases),
    // and the browser or the plain table only takes in finished results.
    std::thread::scope(|scope| -> Result<()> {
        let feed = (fetcher.is_some() || !backlog.is_empty()).then(|| Feed::start(scope, scorer, backlog, fetcher));
        if interactive(output.plain) {
            let settings = tui::Settings {
                title,
                target,
                mode: show.rank,
                weights: show.weights,
                export_dir,
                format: output.format,
                report_top: output.top,
                show_price: output.price,
                show_commander,
                mix: show.mix,
                count_columns,
                layout: show.layout,
                source: source.clone(),
                notice: show.notice.clone(),
                goldfish_games: show.goldfish_games,
            };
            let mut app = tui::App::new(settings, pool, feed, Some(http));
            tui::run(&mut app)?;
            let stats = app.stats();
            if let Some(why) = &stats.stopped {
                eprintln!("  stopped fetching early: {why}");
            }
            let summary = source.unwrap_or_else(|| stats.summary());
            eprintln!("  {summary} · {}", summarize_counts(&http.request_counts()));
            return Ok(());
        }
        if let Some(f) = feed {
            pool.drain(f);
        }
        let columns = plain::Columns { price: output.price, commander: show_commander, counts: count_columns };
        print_plain(http, &pool, show, &title, &target, &columns, source.as_deref(), &export_dir)
    })
}

/// The plain table (and with `--export`, the report) for a finished session. `source` says where
/// the decks came from, instead of the EDHREC counts.
#[allow(clippy::too_many_arguments)]
fn print_plain(
    http: &Http,
    pool: &Pool,
    show: &Show,
    title: &str,
    target: &Target,
    columns: &plain::Columns,
    source: Option<&str>,
    export_dir: &std::path::Path,
) -> Result<()> {
    let output = show.output;
    if let Some(why) = &pool.stats.stopped {
        eprintln!("  stopped fetching early: {why}");
    }
    let summary = source.map_or_else(|| pool.stats.summary(), String::from);
    eprintln!("  {summary} · {}", summarize_counts(&http.request_counts()));
    let checked_after = "-T card filters are checked once a deck is fetched; run again to look at more decks \
                         (decks already fetched cost nothing)";
    if pool.decks.is_empty() {
        if pool.stats.unmatched > 0 {
            bail!("none of the {} decks looked at passes the card filters; {checked_after}", pool.stats.unmatched);
        }
        bail!("no decks matched");
    }
    if pool.stats.unmatched > 0 && pool.decks.len() < output.top {
        eprintln!("  {checked_after}");
    }
    let mut ranking = rank(&pool.scores(), target, show.rank, &show.weights);
    if columns.commander && show.mix {
        let (group, weights) = pool.commander_groups();
        ranking = mix(ranking, &group, &weights);
    }
    let ranking = hoist(ranking, |i| pool.is_favorite(i));
    let favorites = pool.favorites();
    println!("{}", plain::ranked_table(&pool.decks, pool.cards(), &ranking, favorites, target, output.top, columns));
    println!("{}", plain::legend(target, show.rank));
    if output.export.is_some() {
        let input = ReportInput {
            title,
            target,
            mode: show.rank,
            decks: &pool.decks,
            analyze: &|i| pool.details(i),
            mana: &|i| mana::report(&pool.decks[i].deck, pool.cards(), show.goldfish_games),
            ranking: &ranking,
            favorites,
            limit: output.top,
            format: output.format,
            show_price: output.price,
            count_columns: &columns.counts,
        };
        let path = export::write_report(export_dir, &input)?;
        eprintln!("wrote {} and the top {} decklists", path.display(), output.top.min(ranking.len()));
    }
    Ok(())
}

fn run_check(http: &Http, paths: &Paths, config: &Config, args: CheckArgs) -> Result<()> {
    let target = target_from(&args.target)?;
    let text = std::fs::read_to_string(&args.file).with_context(|| format!("reading {}", args.file.display()))?;
    let parsed: Vec<Filter> = args.types.iter().map(|t| t.parse()).collect::<Result<_>>().context("-T")?;
    let (table, card_filters) = filters::split(parsed);
    if let Some(t) = table.first() {
        bail!(
            "-T {}: that's EDHREC's own count, which a local list doesn't have; count the type line instead, \
             like -T '{}={}'",
            t.field.name(),
            t.field.type_query(),
            t.range_arg()
        );
    }
    let notice = filters::count_only_notice(&card_filters);
    let status = Status::new(Some(http));
    if let Some(n) = &notice {
        status.note(&format!("note: {n}"));
    }
    let (cards, combos) = ensure_databases(http, paths, &status)?;
    let card_filters = compile_filters(card_filters, &cards, paths, &status)?;
    let mut deck = parse_decklist(&text, &args.commanders);
    deck.id = args.file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "deck".into());
    if deck.commanders.is_empty() {
        status.note("warning: no commander found (use --commander, a 'Commander' section or *CMDR*)");
    }
    let canonical: Vec<String> = deck.commanders.iter().map(|c| cards.canonical(c).unwrap_or(c).to_string()).collect();
    let slug = (!canonical.is_empty()).then(|| commander_slug(&canonical));
    let title = format!("{} ({})", deck.id, deck.commanders.join(" + "));
    status.step("loading EDHREC's salt scores");
    let mut pool = Pool::new(&cards, &combos, load_salt(http), PoolStats::default());
    if let Some(slug) = &slug {
        status.step("loading EDHREC's synergy and combo votes");
        pool.add_commander(slug, load_commander_sources(http, slug));
    }
    status.step("scoring the deck");
    pool.add(deck);
    if let Some(e) = pool.decks.first_mut() {
        e.matches = card_filters.iter().map(|f| f.count(&e.deck, &cards)).collect();
    }
    drop(status);
    if interactive(args.plain) {
        let settings = tui::Settings {
            title,
            target,
            mode: RankMode::Fit,
            weights: Weights::default(),
            export_dir: PathBuf::from("."),
            format: ExportFormat::Plain,
            report_top: 1,
            show_price: args.price,
            show_commander: false,
            mix: false,
            count_columns: filters::count_columns(&card_filters),
            layout: config.tui.layout,
            source: Some("local decklist".into()),
            notice,
            goldfish_games: config.goldfish.games,
        };
        tui::run(&mut tui::App::new(settings, pool, None, None))?;
    } else {
        let (deck, a) = (&pool.decks[0].deck, pool.details(0));
        let fits = target.fits(&pool.decks[0].score);
        let report = mana::report(deck, &cards, config.goldfish.games);
        let mana = report.as_ref().map_or(view::ManaView::Off, view::ManaView::Ready);
        let mut lines = view::detail_lines(deck, &a, &target, None, args.price, mana);
        if let Some(width) = terminal_width() {
            lines = view::wrap_lines(lines, width);
        }
        println!("{}", view::to_ansi(&lines, color()));
        println!("{} target {}", if fits { "✓ fits" } else { "✗ outside" }, target.describe());
        println!("EDHPowerLevel: {}", edhpl::edhpowerlevel_url(&deck.sectioned_lines()));
    }
    Ok(())
}

/// Read the config file, and size the thread pool for bulk work by it.
fn load_config(path: Option<PathBuf>) -> Result<Config> {
    let config = Config::load(path)?;
    let threads = config.performance.threads();
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("edh-scout-{i}"))
        .build_global()
        .context("starting the thread pool")?;
    Ok(config)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = (|| -> Result<()> {
        let paths = Paths::new(cli.cache_dir.clone(), cli.state_dir.clone())?;
        let http = Arc::new(Http::new(paths.http(), cli.offline));
        match cli.command {
            Command::Scout(args) => run_scout(&http, &paths, &load_config(cli.config.clone())?, args),
            Command::Favorites(args) => run_favorites(&http, &paths, &load_config(cli.config.clone())?, args),
            Command::Check(args) => run_check(&http, &paths, &load_config(cli.config.clone())?, args),
            Command::Sync(args) => {
                let all = !args.cards && !args.combos && !args.forge;
                let config = load_config(cli.config.clone())?;
                let forge = match ForgeSource::find(config.goldfish.forge_cards.as_deref()) {
                    ForgeSource::Download => all || args.forge,
                    ForgeSource::Dir(dir) => {
                        if all || args.forge {
                            eprintln!("Forge's card scripts are read from {}: nothing to download", dir.display());
                        }
                        false
                    }
                };
                let what = SyncWhat {
                    cards: all || args.cards,
                    combos: all || args.combos,
                    forge,
                    use_existing_raw: args.rebuild,
                };
                sync(&http, &paths, what, &Lines)?;
                eprintln!(
                    "{} · cache in {} · decks in {}",
                    summarize_counts(&http.request_counts()),
                    paths.root.display(),
                    paths.decks_db().display()
                );
                Ok(())
            }
        }
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if let Some(m) = e.downcast_ref::<OfflineMiss>() {
                eprintln!("error: {m}");
            } else {
                eprintln!("error: {e:#}");
            }
            ExitCode::FAILURE
        }
    }
}
