use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use edh_data::cards::CardDb;
use edh_data::config::Paths;
use edh_data::deck::parse_decklist;
use edh_data::edhrec::DeckStore;
use edh_data::http::{Http, OfflineMiss, summarize_counts};
use edh_data::sync::{SyncWhat, ensure_databases, sync};
use edh_data::text::{commander_slug, date_days_ago};
use edh_scout::analysis::analyze;
use edh_scout::export::{self, ExportFormat, ReportInput};
use edh_scout::rank::{RankMode, Target, Weights, parse_range, rank};
use edh_scout::scout::{ScoutOptions, load_sources, scout};
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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
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
    commander: String,
    /// Second commander (partner, background, ...).
    #[arg(long)]
    partner: Option<String>,
    /// Override the EDHREC page slug, e.g. kraum-ludevics-opus-tymna-the-weaver.
    #[arg(long)]
    slug: Option<String>,
    #[command(flatten)]
    target: TargetArgs,
    /// How to rank decks.
    #[arg(short, long, value_enum, default_value_t = RankMode::Power)]
    rank: RankMode,
    /// Weights for --rank combined.
    #[arg(long, default_value = "power=0.5,salt=0.2,structure=0.15,synergy=0.15")]
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

fn run_scout(http: &Http, paths: &Paths, args: ScoutArgs) -> Result<()> {
    let target = target_from(&args.target)?;
    let weights: Weights = args.weights.parse().context("--weights")?;
    let since = match &args.since {
        Some(s) if s.chars().all(|c| c.is_ascii_digit()) => Some(date_days_ago(s.parse()?)),
        Some(s) if s.len() == 10 && s.as_bytes()[4] == b'-' => Some(s.clone()),
        Some(s) => bail!("--since takes YYYY-MM-DD or a number of days, got '{s}'"),
        None => None,
    };
    let (cards, combos) = ensure_databases(http, paths)?;

    let mut commanders = vec![resolve_commander(&cards, &args.commander, "commander")?];
    if let Some(p) = &args.partner {
        commanders.push(resolve_commander(&cards, p, "partner")?);
    }
    let slug = args.slug.clone().unwrap_or_else(|| commander_slug(&commanders));
    let title = commanders.iter().map(|c| edh_data::text::front_face(c)).collect::<Vec<_>>().join(" + ");
    http.set_min_gap("edhrec.com", std::time::Duration::from_secs_f64(args.deck_delay.max(2.0)));
    eprintln!("scouting {title} (edhrec.com/commanders/{slug}) for {} ...", target.describe());

    let opts = ScoutOptions {
        commanders,
        slug: slug.clone(),
        select: SelectOptions {
            brackets: target.brackets,
            declared_only: args.declared_only,
            max_price: args.max_price,
            max_salt: args.max_salt,
            tags: args.tags.clone(),
            since,
            budget: args.fetch,
        },
    };
    let store = DeckStore::new(paths.decks());
    let pool = scout(http, &store, &cards, &combos, &opts)?;
    let s = &pool.stats;
    let mut summary = format!("{} on EDHREC, {} eligible", s.listed, s.eligible);
    if s.cached > 0 {
        summary += &format!(", {} reused", s.cached);
    }
    if s.fetched > 0 {
        summary += &format!(", {} fetched", s.fetched);
    }
    if s.failed > 0 {
        summary += &format!(", {} failed", s.failed);
    }
    if let Some(c) = s.ceiling {
        eprintln!(
            "  decks capped at salt {:.0} / ${:.0} (90th percentile of {} decks declared {})",
            c.salt,
            c.price,
            c.from_decks,
            target.describe().split(" · ").next().unwrap_or("")
        );
    }
    if let Some(why) = &s.stopped {
        eprintln!("  stopped fetching early: {why}");
    }
    eprintln!("  {summary} · {}", summarize_counts(&http.request_counts()));
    if pool.decks.is_empty() {
        bail!(if http.offline() {
            "no cached decks to show (offline)"
        } else {
            "no decks matched; loosen the filters"
        });
    }

    let export_dir = args.output.export.clone().unwrap_or_else(|| PathBuf::from(format!("edh-scout-{slug}")));
    if interactive(args.output.plain) {
        let app = tui::App::new(
            title,
            summary,
            target,
            args.rank,
            weights,
            &pool.decks,
            &cards,
            export_dir,
            args.output.format,
            args.output.top,
        );
        tui::run(app)?;
    } else {
        let refs: Vec<_> = pool.decks.iter().map(|(_, a)| a).collect();
        let ranking = rank(&refs, &target, args.rank, &weights);
        println!("{}", plain::ranked_table(&pool.decks, &ranking, &target, args.output.top));
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
            };
            let path = export::write_report(&export_dir, &input)?;
            eprintln!("wrote {} and the top {} decklists", path.display(), args.output.top.min(ranking.len()));
        }
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
    let sources = load_sources(http, slug.as_deref());
    let analysis = analyze(&deck, &sources.context(&cards, &combos));
    let title = format!("{} ({})", deck.id, deck.commanders.join(" + "));
    let decks = vec![(deck, analysis)];
    if interactive(args.plain) {
        let app = tui::App::new(
            title,
            "local decklist".into(),
            target,
            RankMode::Power,
            Weights::default(),
            &decks,
            &cards,
            PathBuf::from("."),
            ExportFormat::Plain,
            1,
        );
        tui::run(app)?;
    } else {
        let (deck, a) = &decks[0];
        let fits = target.fits(a);
        println!("{}", view::to_ansi(&view::detail_lines(deck, a, &target, None), color()));
        println!("{} target {}", if fits { "✓ fits" } else { "✗ outside" }, target.describe());
        println!("EDHPowerLevel: {}", edhpl::edhpowerlevel_url(&deck.sectioned_lines()));
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = (|| -> Result<()> {
        let paths = Paths::new(cli.cache_dir.clone(), cli.state_dir.clone())?;
        let http = Http::new(paths.http(), cli.offline);
        match cli.command {
            Command::Scout(args) => run_scout(&http, &paths, args),
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
