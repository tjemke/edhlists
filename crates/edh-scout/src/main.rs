use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use edh_data::cards::CardDb;
use edh_data::combos::ComboDb;
use edh_data::config::Paths;
use edh_data::deck::parse_decklist;
use edh_data::edhrec::DeckStore;
use edh_data::edhrec::{self, ListedCommander};
use edh_data::http::{Http, OfflineMiss, summarize_counts};
use edh_data::sync::{SyncWhat, ensure_databases, sync};
use edh_data::text::{commander_slug, date_days_ago, front_face, slugify};
use edh_scout::config::Config;
use edh_scout::export::{self, ExportFormat, ReportInput};
use edh_scout::filters::{self, TypeFilter};
use edh_scout::rank::{RankMode, Target, Weights, parse_range, rank};
use edh_scout::scout::{Fetcher, Pool, PoolStats, Prepared, ScoutOptions, load_commander_sources, load_salt, prepare};
use edh_scout::select::SelectOptions;
use edh_scout::{picker, plain, tui, view};

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
    /// Card-type count filter, repeatable: planeswalkers=10+, creatures=..5, spells=25-40, lands=36
    /// (spells = instants + sorceries). With a commander it narrows that commander's decks; without
    /// one it searches across commanders.
    #[arg(short = 'T', long = "type", value_name = "TYPE=RANGE")]
    types: Vec<String>,
    /// Searching across commanders: how many commanders' deck tables to scan (1 request each).
    #[arg(long, default_value_t = 30, value_name = "N")]
    commanders: usize,
    #[command(flatten)]
    target: TargetArgs,
    /// How to rank decks.
    #[arg(short, long, value_enum, default_value_t = RankMode::Fit)]
    rank: RankMode,
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
struct CheckArgs {
    /// Decklist file (plain, MTGA, Moxfield or Archidekt text export).
    file: PathBuf,
    /// Commander, if the file doesn't mark one (repeat for partners).
    #[arg(short, long = "commander")]
    commanders: Vec<String>,
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
    #[arg(long)]
    combos: bool,
    /// Rebuild from the downloads already on disk instead of fetching new ones.
    #[arg(long)]
    rebuild: bool,
}

fn target_from(args: &TargetArgs) -> Result<Target> {
    let brackets: (u8, u8) = parse_range(&args.brackets).context("--brackets")?;
    if brackets.0 < 1 || brackets.1 > 5 {
        bail!("brackets run from 1 to 5");
    }
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
fn resolve_commander(cards: &CardDb, query: &str, role: &str) -> Result<String> {
    let candidates = match cards.find_commander(query) {
        Ok(c) => return Ok(c.name.clone()),
        Err(candidates) => candidates,
    };
    if picker::can_pick() {
        return match picker::pick(cards, role, query)? {
            Some(c) => {
                eprintln!("{role}: {}", c.name);
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

/// A scout in progress: decks so far, and the background work still filling them in.
struct Session<'a> {
    title: String,
    /// Default export directory name (`edh-scout-<name>`).
    name: String,
    pool: Pool<'a>,
    fetcher: Option<Fetcher>,
    show_commander: bool,
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
    let types: Vec<TypeFilter> = args.types.iter().map(|t| t.parse()).collect::<Result<_>>().context("-T")?;
    let (cards, combos) = ensure_databases(http, paths)?;
    http.set_min_gap("edhrec.com", std::time::Duration::from_secs_f64(args.deck_delay.max(2.0)));
    let select = SelectOptions {
        brackets: target.brackets,
        declared_only: args.declared_only,
        max_price: args.max_price,
        price_ceiling: args.output.price,
        max_salt: args.max_salt,
        tags: args.tags.clone(),
        since,
        types: types.clone(),
        budget: args.fetch,
    };
    let store = DeckStore::new(paths.decks());
    let salt = load_salt(http);
    let session = match &args.commander {
        Some(name) => start_single(http, &store, &cards, &combos, salt, &args, name, &target, select, keep_fetching)?,
        None => start_search(http, &store, &cards, &combos, salt, &args, &types, &target, select, keep_fetching)?,
    };
    finish(http, session, &args, target, weights)
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
    select: SelectOptions,
    keep_fetching: bool,
) -> Result<Session<'a>> {
    let mut commanders = vec![resolve_commander(cards, name, "commander")?];
    if let Some(p) = &args.partner {
        commanders.push(resolve_commander(cards, p, "partner")?);
    }
    let slug = args.slug.clone().unwrap_or_else(|| commander_slug(&commanders));
    let mut title = commanders.iter().map(|c| front_face(c)).collect::<Vec<_>>().join(" + ");
    if !select.types.is_empty() {
        title += &format!(" · {}", filters::describe(&select.types));
    }
    eprintln!("scouting {title} (edhrec.com/commanders/{slug}) for {} ...", target.describe());
    let opts = ScoutOptions { commanders, slug: slug.clone(), select };
    let Prepared { decks, sources, mut stats, mut to_fetch, more } = prepare(http, store, &opts)?;
    if let Some(c) = stats.ceiling {
        let price = if args.output.price { format!(" / ${:.0}", c.price) } else { String::new() };
        eprintln!(
            "  decks capped at salt {:.0}{price} (90th percentile of {} decks declared {})",
            c.salt,
            c.from_decks,
            target.describe().split(" · ").next().unwrap_or("")
        );
    }
    if decks.is_empty() && to_fetch.is_empty() {
        bail!(if http.offline() {
            "no stored decks to show (offline)"
        } else {
            "no decks matched; loosen the filters"
        });
    }
    let later = if keep_fetching && !more.is_empty() {
        format!(", then {} more while the browser is open", more.len())
    } else {
        String::new()
    };
    eprintln!(
        "  {} stored deck(s) ready, {} to fetch in the background{later} · {}",
        decks.len(),
        to_fetch.len(),
        summarize_counts(&http.request_counts())
    );
    if keep_fetching {
        to_fetch.extend(more);
        stats.queued = to_fetch.len();
    }
    let fetcher = (!to_fetch.is_empty())
        .then(|| Fetcher::spawn(Arc::clone(http), store.clone(), to_fetch, opts.commanders.clone()));
    let mut pool = Pool::new(cards, combos, salt, stats);
    pool.add_commander(&slug, sources);
    for d in decks {
        pool.add(d);
    }
    Ok(Session { title, name: slug, pool, fetcher, show_commander: false })
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
    args: &ScoutArgs,
    types: &[TypeFilter],
    target: &Target,
    select: SelectOptions,
    keep_fetching: bool,
) -> Result<Session<'a>> {
    let themes = filters::themes(types);
    let mut candidates: Vec<ListedCommander> = Vec::new();
    for theme in &themes {
        match edhrec::theme_commanders(http, theme) {
            Ok(list) => candidates.extend(list),
            Err(e) => eprintln!("  note: couldn't load EDHREC's '{theme}' theme ({e:#})"),
        }
    }
    let source = if candidates.is_empty() {
        candidates = edhrec::top_commanders(http).context("EDHREC's top commanders")?;
        "EDHREC's most-built commanders".to_string()
    } else {
        format!("EDHREC theme{} {}", if themes.len() > 1 { "s" } else { "" }, themes.join(", "))
    };
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| seen.insert(c.slug.clone()));
    candidates.sort_by_key(|c| std::cmp::Reverse(c.num_decks));
    candidates.truncate(args.commanders.max(1));
    let wanted = filters::describe(types);
    eprintln!(
        "searching {} commanders from {source} for decks with {wanted}, {} ...",
        candidates.len(),
        target.describe()
    );
    let stats = PoolStats { commanders_total: candidates.len(), ..Default::default() };
    let pool = Pool::new(cards, combos, salt, stats);
    let fetcher = Fetcher::spawn_search(Arc::clone(http), store.clone(), candidates, select, keep_fetching);
    Ok(Session {
        title: format!("decks with {wanted}"),
        name: format!("search-{}", slugify(&wanted)),
        pool,
        fetcher: Some(fetcher),
        show_commander: true,
    })
}

/// Browse (or print) the session, then report what it cost.
fn finish(http: &Arc<Http>, session: Session, args: &ScoutArgs, target: Target, weights: Weights) -> Result<()> {
    let Session { title, name, mut pool, fetcher, show_commander } = session;
    let export_dir = args.output.export.clone().unwrap_or_else(|| PathBuf::from(format!("edh-scout-{name}")));
    if interactive(args.output.plain) {
        let settings = tui::Settings {
            title,
            target,
            mode: args.rank,
            weights,
            export_dir,
            format: args.output.format,
            report_top: args.output.top,
            show_price: args.output.price,
            show_commander,
        };
        let mut app = tui::App::new(settings, pool, fetcher, Some(http));
        tui::run(&mut app)?;
        let stats = app.stats();
        if let Some(why) = &stats.stopped {
            eprintln!("  stopped fetching early: {why}");
        }
        eprintln!("  {} · {}", stats.summary(), summarize_counts(&http.request_counts()));
        return Ok(());
    }
    if let Some(f) = fetcher {
        pool.drain(f);
    }
    if let Some(why) = &pool.stats.stopped {
        eprintln!("  stopped fetching early: {why}");
    }
    eprintln!("  {} · {}", pool.stats.summary(), summarize_counts(&http.request_counts()));
    if pool.decks.is_empty() {
        bail!("no decks matched");
    }
    let refs: Vec<_> = pool.decks.iter().map(|(_, a)| a).collect();
    let ranking = rank(&refs, &target, args.rank, &weights);
    let columns = plain::Columns { price: args.output.price, commander: show_commander };
    println!("{}", plain::ranked_table(&pool.decks, &ranking, &target, args.output.top, columns));
    println!("{}", plain::legend(&target, args.rank));
    if args.output.export.is_some() {
        let input = ReportInput {
            title: &title,
            target: &target,
            mode: args.rank,
            decks: &pool.decks,
            ranking: &ranking,
            limit: args.output.top,
            format: args.output.format,
            show_price: args.output.price,
        };
        let path = export::write_report(&export_dir, &input)?;
        eprintln!("wrote {} and the top {} decklists", path.display(), args.output.top.min(ranking.len()));
    }
    Ok(())
}

fn run_check(http: &Http, paths: &Paths, args: CheckArgs) -> Result<()> {
    let target = target_from(&args.target)?;
    let text = std::fs::read_to_string(&args.file).with_context(|| format!("reading {}", args.file.display()))?;
    let (cards, combos) = ensure_databases(http, paths)?;
    let mut deck = parse_decklist(&text, &args.commanders);
    deck.id = args.file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "deck".into());
    if deck.commanders.is_empty() {
        eprintln!("warning: no commander found (use --commander, a 'Commander' section or *CMDR*)");
    }
    let canonical: Vec<String> = deck.commanders.iter().map(|c| cards.canonical(c).unwrap_or(c).to_string()).collect();
    let slug = (!canonical.is_empty()).then(|| commander_slug(&canonical));
    let title = format!("{} ({})", deck.id, deck.commanders.join(" + "));
    let mut pool = Pool::new(&cards, &combos, load_salt(http), PoolStats::default());
    if let Some(slug) = &slug {
        pool.add_commander(slug, load_commander_sources(http, slug));
    }
    pool.add(deck);
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
        };
        tui::run(&mut tui::App::new(settings, pool, None, None))?;
    } else {
        let (deck, a) = &pool.decks[0];
        let fits = target.fits(a);
        let mut lines = view::detail_lines(deck, a, &target, None, args.price);
        if let Some(width) = terminal_width() {
            lines = view::wrap_lines(lines, width);
        }
        println!("{}", view::to_ansi(&lines, color()));
        println!("{} target {}", if fits { "✓ fits" } else { "✗ outside" }, target.describe());
        println!("EDHPowerLevel: {}", edhpl::edhpowerlevel_url(&deck.sectioned_lines()));
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = (|| -> Result<()> {
        let paths = Paths::new(cli.cache_dir.clone(), cli.state_dir.clone())?;
        let http = Arc::new(Http::new(paths.http(), cli.offline));
        match cli.command {
            Command::Scout(args) => run_scout(&http, &paths, &Config::load(cli.config.clone())?, args),
            Command::Check(args) => run_check(&http, &paths, args),
            Command::Sync(args) => {
                let both = !args.cards && !args.combos;
                sync(
                    &http,
                    &paths,
                    SyncWhat { cards: both || args.cards, combos: both || args.combos, use_existing_raw: args.rebuild },
                )?;
                eprintln!(
                    "{} · cache in {} · decks in {}",
                    summarize_counts(&http.request_counts()),
                    paths.root.display(),
                    paths.decks().display()
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
