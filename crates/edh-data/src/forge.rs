//! Forge's card scripts (`res/cardsfolder` in [Forge](https://github.com/Card-Forge/forge)): how
//! each card works, written for Forge's rules engine. The mana goldfish reads a few of their
//! fields (what a land or rock makes and what it costs, when a land enters tapped, what a ramp
//! spell fetches) instead of guessing them from oracle text.
//!
//! A script is one file per card: `Key:value` lines, with a line `ALTERNATE` between the faces of
//! a card with two. Abilities are `|`-separated `Param$ Value` pairs (see [`params`]):
//!
//! ```text
//! Name:Drowned Catacomb
//! ManaCost:no cost
//! Types:Land
//! R:Event$ Moved | ValidCard$ Card.Self | Destination$ Battlefield | ReplaceWith$ LandTapped | …
//! SVar:LandTapped:DB$ Tap | Defined$ Self | ETB$ True | ConditionPresent$ Island.YouCtrl,Swamp.YouCtrl | ConditionCompare$ EQ0
//! A:AB$ Mana | Cost$ T | Produced$ Combo U B | SpellDescription$ Add {U} or {B}.
//! ```
//!
//! Only the lines that say how a card works are kept: no oracle text, AI hints or descriptions.
//!
//! The scripts come from, in order: a directory named by `EDH_SCOUT_FORGE_CARDS` or the config
//! file; the one a Nix build of edh-scout points at (nixpkgs' `forge-mtg`); or Forge's latest
//! release, which `sync` downloads (Forge is GPL-3.0; the scripts stay in the cache, outside this
//! repository).

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::storage::{read_json_gz, write_json_gz};
use crate::text::fold;

/// Bump when [`ForgeDb`] changes shape or what's kept of a script; it's rebuilt from the kept raw
/// download without downloading again ([`ForgeSource::load`], `sync`).
pub const FORGE_SCHEMA: u32 = 2;

/// Names the directory of Forge's card scripts to read (`…/res/cardsfolder`).
pub const FORGE_CARDS_ENV: &str = "EDH_SCOUT_FORGE_CARDS";

/// Forge's latest release (one request: its tag).
pub const FORGE_LATEST_RELEASE: &str = "https://api.github.com/repos/Card-Forge/forge/releases/latest";

/// The source of a Forge release, as a gzipped tar (about 250 MB, of which the scripts are 24).
pub fn forge_tarball(tag: &str) -> String {
    format!("https://codeload.github.com/Card-Forge/forge/tar.gz/refs/tags/{tag}")
}

/// Subdirectories of `cardsfolder` that aren't cards of the paper game: Alchemy's rebalanced
/// versions ("A-Akki Ronin").
const NOT_PAPER: &[&str] = &["rebalanced"];

/// Lines of a script that say how the card works. The rest are oracle text, AI hints, deck-building
/// hints and art.
const KEPT: &[&str] = &["Name", "ManaCost", "Types", "PT", "A", "K", "T", "S", "R", "SVar"];

/// One face of a card.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Face {
    pub name: String,
    /// `ManaCost`, as Forge writes it: `2 G`, `W U`, `X R R`, `no cost`.
    pub cost: String,
    /// `Types`: `Legendary Creature Elf Druid`.
    pub types: String,
    /// The `PT:`, `A:`, `K:`, `T:`, `S:`, `R:` and `SVar:` lines, in order, key included,
    /// descriptions left out.
    pub lines: Vec<String>,
}

impl Face {
    /// The values of the lines with this key (`"A"` gives every `A:` line without its `A:`).
    pub fn lines_with<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.lines.iter().filter_map(move |l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')))
    }

    /// The SVar `name`'s value (`SVar:LandTapped:DB$ Tap | …` → `DB$ Tap | …`).
    pub fn svar(&self, name: &str) -> Option<&str> {
        self.lines_with("SVar").find_map(|v| v.strip_prefix(name).and_then(|r| r.strip_prefix(':')))
    }

    /// `K:` keywords, with their parameters (`Multikicker:2`, `Suspend:3:1`).
    pub fn keywords(&self) -> impl Iterator<Item = &str> {
        self.lines_with("K")
    }

    /// A creature's printed power (`PT:2/3`); `None` without one or when it's variable (`*/*`).
    pub fn power(&self) -> Option<u32> {
        self.lines_with("PT").next()?.split('/').next()?.trim().parse().ok()
    }
}

/// One card's script.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Script {
    /// Front face first.
    pub faces: Vec<Face>,
    /// How the faces relate (`AlternateMode`): `Modal`, `DoubleFaced`, `Split`, `Adventure`, …;
    /// empty for a card with one face.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mode: String,
}

impl Script {
    /// Read a script file. `None` if it has no `Name`.
    pub fn parse(text: &str) -> Option<Script> {
        let mut script = Script { faces: vec![Face::default()], mode: String::new() };
        for line in text.lines() {
            let line = line.trim_end();
            match line {
                "ALTERNATE" => {
                    script.faces.push(Face::default());
                    continue;
                }
                // Alchemy's specialized versions follow; they aren't faces of the card.
                "SPECIALIZE" => break,
                _ => {}
            }
            let Some((key, value)) = line.split_once(':') else { continue };
            let face = script.faces.last_mut().expect("a face");
            match key {
                "Name" => face.name = value.to_string(),
                "ManaCost" => face.cost = value.to_string(),
                "Types" => face.types = value.to_string(),
                "AlternateMode" => script.mode = value.to_string(),
                k if KEPT.contains(&k) => face.lines.push(format!("{key}:{}", without_descriptions(value))),
                _ => {}
            }
        }
        script.faces.retain(|f| !f.name.is_empty());
        (!script.faces.is_empty()).then_some(script)
    }

    /// The card's name as Scryfall writes it: its faces' names joined by ` // ` (Forge names each
    /// face), except for melded cards, which Scryfall names by their front face.
    pub fn name(&self) -> String {
        if self.mode == "Meld" || self.faces.len() == 1 {
            return self.faces[0].name.clone();
        }
        self.faces.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(" // ")
    }

    pub fn front(&self) -> &Face {
        &self.faces[0]
    }
}

/// An ability's or trigger's `|`-separated `Param$ Value` pairs, in order, trimmed. The first is
/// its kind (`AB$ Mana`, `SP$ ChangeZone`, `DB$ Tap`, `Mode$ ChangesZone`, `Event$ Moved`).
pub fn params(ability: &str) -> Vec<(&str, &str)> {
    ability
        .split('|')
        .filter_map(|p| {
            let (k, v) = p.split_once('$')?;
            Some((k.trim(), v.trim()))
        })
        .collect()
}

/// The value of `key` among `params`.
pub fn param<'a>(params: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    params.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// An ability without its text for players (`SpellDescription$ …`, `ChangeTypeDesc$ …`,
/// `SelectPrompt$ …`): that's oracle text again, and most of a script's size.
fn without_descriptions(value: &str) -> String {
    if !value.contains('$') {
        return value.to_string();
    }
    // `SVar:Name:DB$ …` keeps its name.
    let (head, body) = match value.split_once(':') {
        Some((name, rest)) if !name.contains('$') && rest.contains('$') => (Some(name), rest),
        _ => (None, value),
    };
    let kept: Vec<&str> = body
        .split('|')
        .filter(|p| {
            let key = p.split_once('$').map(|(k, _)| k.trim()).unwrap_or("");
            !(key.ends_with("Description") || key.ends_with("Desc") || key.ends_with("Prompt"))
        })
        .map(str::trim)
        .collect();
    let body = kept.join(" | ");
    match head {
        Some(name) => format!("{name}:{body}"),
        None => body,
    }
}

/// Every card's script, by name.
#[derive(Serialize, Deserialize)]
pub struct ForgeDb {
    pub schema: u32,
    /// Where the scripts came from: a release tag (`forge-2.0.16`) or a directory.
    pub source: String,
    /// Unix seconds when this was built.
    pub built: u64,
    pub scripts: Vec<Script>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}

/// A downloaded release's scripts, as they were in it: `sync` rebuilds [`ForgeDb`] from this
/// without downloading again.
#[derive(Serialize, Deserialize)]
pub struct ForgeRaw {
    pub source: String,
    /// Path inside `cardsfolder` (`c/cultivate.txt`) and the script.
    pub files: Vec<(String, String)>,
}

impl ForgeDb {
    /// Scripts in `cardsfolder` order (`a/…` to `z/…`, then `upcoming/…`). A card scripted twice
    /// keeps its first script.
    pub fn new(source: String, files: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut files: Vec<(String, String)> = files
            .into_iter()
            .filter(|(path, _)| path.ends_with(".txt") && !NOT_PAPER.iter().any(|d| path.starts_with(&format!("{d}/"))))
            .collect();
        files.sort_by(|(a, _), (b, _)| (a.starts_with("upcoming/"), a).cmp(&(b.starts_with("upcoming/"), b)));
        let scripts = files.iter().filter_map(|(_, text)| Script::parse(text)).collect();
        let built = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut db = ForgeDb { schema: FORGE_SCHEMA, source, built, scripts, index: HashMap::new() };
        db.reindex();
        db
    }

    /// Read the scripts in `dir` (a `cardsfolder`: one subdirectory per letter).
    pub fn from_dir(dir: &Path) -> Result<Self> {
        Ok(Self::new(dir.display().to_string(), read_cardsfolder(dir)?))
    }

    pub fn from_raw(raw: ForgeRaw) -> Self {
        Self::new(raw.source, raw.files)
    }

    fn reindex(&mut self) {
        let mut index = HashMap::with_capacity(self.scripts.len() * 2);
        for (i, s) in self.scripts.iter().enumerate() {
            let name = s.name();
            index.entry(name.to_lowercase()).or_insert(i);
            index.entry(fold(&name)).or_insert(i);
        }
        // Front faces after whole names, so a whole name always wins.
        for (i, s) in self.scripts.iter().enumerate() {
            index.entry(s.front().name.to_lowercase()).or_insert(i);
        }
        self.index = index;
    }

    /// The script of the card called `name` (Scryfall's name, or its front face's).
    pub fn get(&self, name: &str) -> Option<&Script> {
        let find = |n: &str| self.index.get(&n.to_lowercase()).or_else(|| self.index.get(&fold(n)));
        // A back face Forge copies from another card has no name of its own in the script
        // (Joined Researchers // Secret Rendezvous): the front face's name finds it.
        let i = find(name).or_else(|| find(name.split(" // ").next()?))?;
        Some(&self.scripts[*i])
    }

    pub fn len(&self) -> usize {
        self.scripts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_gz(path, self)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut db: ForgeDb = read_json_gz(path)?;
        db.reindex();
        Ok(db)
    }
}

/// Every script under `dir`, by its path inside it.
fn read_cardsfolder(dir: &Path) -> Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    let subdirs = fs::read_dir(dir).with_context(|| format!("reading Forge's card scripts in {}", dir.display()))?;
    for sub in subdirs {
        let sub = sub?;
        if !sub.file_type()?.is_dir() {
            continue;
        }
        let letter = sub.file_name().to_string_lossy().into_owned();
        for file in fs::read_dir(sub.path())? {
            let file = file?;
            let name = file.file_name().to_string_lossy().into_owned();
            if name.ends_with(".txt") {
                let text =
                    fs::read_to_string(file.path()).with_context(|| format!("reading {}", file.path().display()))?;
                files.push((format!("{letter}/{name}"), text));
            }
        }
    }
    anyhow::ensure!(!files.is_empty(), "no card scripts in {} (expected Forge's res/cardsfolder)", dir.display());
    Ok(files)
}

/// The scripts in a Forge source tarball (`<top>/forge-gui/res/cardsfolder/…`), by their path
/// inside `cardsfolder`. Everything else in it is skipped as it streams past.
pub fn read_tarball(gz: impl Read) -> Result<Vec<(String, String)>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(gz));
    let mut files = Vec::new();
    for entry in archive.entries().context("reading Forge's source tarball")? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        let Some((_, inside)) = path.split_once("/forge-gui/res/cardsfolder/") else { continue };
        if !inside.ends_with(".txt") || !entry.header().entry_type().is_file() {
            continue;
        }
        let inside = inside.to_string();
        let mut text = String::new();
        entry.read_to_string(&mut text).with_context(|| format!("reading {path}"))?;
        files.push((inside, text));
    }
    anyhow::ensure!(!files.is_empty(), "Forge's source tarball has no forge-gui/res/cardsfolder");
    Ok(files)
}

/// Where Forge's scripts are read from.
#[derive(Clone, Debug, PartialEq)]
pub enum ForgeSource {
    /// A `cardsfolder` directory: named by [`FORGE_CARDS_ENV`] or the config file, or the one a
    /// Nix build points at.
    Dir(PathBuf),
    /// The release `sync` downloads into the cache.
    Download,
}

/// The directory a Nix build of edh-scout points at (nixpkgs' `forge-mtg`), if it was built so.
const BUILT_IN: Option<&str> = option_env!("EDH_SCOUT_FORGE_CARDS_BUILTIN");

impl ForgeSource {
    /// [`FORGE_CARDS_ENV`], else the config file's path, else a Nix build's, else the download.
    pub fn find(configured: Option<&Path>) -> ForgeSource {
        let env = std::env::var_os(FORGE_CARDS_ENV).filter(|v| !v.is_empty()).map(PathBuf::from);
        match env.or_else(|| configured.map(Path::to_path_buf)).or_else(|| BUILT_IN.map(PathBuf::from)) {
            Some(dir) => ForgeSource::Dir(dir),
            None => ForgeSource::Download,
        }
    }

    /// The scripts, if there are any: read from the directory, or the download's database
    /// (`db_path`). One of an older schema is rebuilt from the download kept beside it (`raw_path`,
    /// [`ForgeRaw`]), and saved for next time; nothing is downloaded.
    pub fn load(&self, db_path: &Path, raw_path: &Path) -> Result<Option<ForgeDb>> {
        match self {
            ForgeSource::Dir(dir) => ForgeDb::from_dir(dir).map(Some),
            ForgeSource::Download => {
                if let Some(db) = ForgeDb::load(db_path).ok().filter(|db| db.schema == FORGE_SCHEMA) {
                    return Ok(Some(db));
                }
                if !raw_path.exists() {
                    return Ok(None);
                }
                let db = ForgeDb::from_raw(read_json_gz(raw_path)?);
                // Read-only cache: still good for this run.
                let _ = db.save(db_path);
                Ok(Some(db))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written for these tests in the shape of Forge's scripts (not copied from Forge).
    const TAPLAND: &str = "Name:Test Grove\nManaCost:no cost\nTypes:Land\n\
        R:Event$ Moved | ValidCard$ Card.Self | Destination$ Battlefield | ReplaceWith$ ETBTapped | Description$ CARDNAME enters tapped.\n\
        SVar:ETBTapped:DB$ Tap | Defined$ Self | ETB$ True\n\
        A:AB$ Mana | Cost$ T | Produced$ Combo G W | SpellDescription$ Add {G} or {W}.\n\
        DeckHints:Type$Elf\nOracle:Test Grove enters tapped.\\n{T}: Add {G} or {W}.\n";

    const MODAL: &str = "Name:Test Bloom\nManaCost:1 G\nTypes:Sorcery\n\
        A:SP$ Draw | NumCards$ 2 | SpellDescription$ Draw two cards.\nAlternateMode:Modal\n\
        Oracle:Draw two cards.\n\nALTERNATE\n\nName:Test Bloomfield\nManaCost:no cost\nTypes:Land\n\
        A:AB$ Mana | Cost$ T | Produced$ G | SpellDescription$ Add {G}.\nOracle:{T}: Add {G}.\n";

    #[test]
    fn a_script_keeps_how_the_card_works_and_not_its_text() {
        let s = Script::parse(TAPLAND).expect("a script");
        let f = s.front();
        assert_eq!((f.name.as_str(), f.cost.as_str(), f.types.as_str()), ("Test Grove", "no cost", "Land"));
        assert_eq!(
            f.lines,
            [
                "R:Event$ Moved | ValidCard$ Card.Self | Destination$ Battlefield | ReplaceWith$ ETBTapped",
                "SVar:ETBTapped:DB$ Tap | Defined$ Self | ETB$ True",
                "A:AB$ Mana | Cost$ T | Produced$ Combo G W",
            ],
            "descriptions, oracle text and hints should be left out, the rest kept as written"
        );
        assert_eq!(f.svar("ETBTapped"), Some("DB$ Tap | Defined$ Self | ETB$ True"));
        let mana = params(f.lines_with("A").next().unwrap());
        assert_eq!(mana[0], ("AB", "Mana"));
        assert_eq!(param(&mana, "Produced"), Some("Combo G W"));
    }

    #[test]
    fn a_two_faced_card_is_found_by_its_whole_name_and_its_front_face() {
        let db = ForgeDb::new(
            "test".into(),
            [("t/test_grove.txt".to_string(), TAPLAND.to_string()), ("t/test_bloom.txt".into(), MODAL.into())],
        );
        let bloom = db.get("Test Bloom // Test Bloomfield").expect("found by Scryfall's name");
        assert_eq!(bloom.mode, "Modal");
        assert_eq!(bloom.faces[1].lines, ["A:AB$ Mana | Cost$ T | Produced$ G"]);
        assert!(std::ptr::eq(db.get("test bloom").unwrap(), bloom), "found by its front face, in any case");
        assert!(db.get("Test Bloomfield").is_none(), "a back face isn't a card of its own");
        assert!(db.get("Test Grove").is_some());
    }

    #[test]
    fn alchemy_rebalanced_cards_and_other_files_are_left_out() {
        let db = ForgeDb::new(
            "test".into(),
            [
                ("rebalanced/a-test_grove.txt".to_string(), TAPLAND.replace("Test Grove", "A-Test Grove")),
                ("mkzip.sh".into(), "Name:Not A Card".into()),
                ("t/test_grove.txt".into(), TAPLAND.into()),
            ],
        );
        assert_eq!(db.len(), 1, "only t/test_grove.txt is a paper card's script");
    }

    #[test]
    fn a_card_scripted_twice_keeps_its_released_script() {
        let upcoming = TAPLAND.replace("Combo G W", "Any");
        let db = ForgeDb::new(
            "test".into(),
            [("upcoming/test_grove.txt".to_string(), upcoming), ("t/test_grove.txt".into(), TAPLAND.into())],
        );
        assert!(db.get("Test Grove").unwrap().front().lines.iter().any(|l| l.ends_with("Combo G W")));
    }

    #[test]
    fn a_configured_directory_is_read_when_the_environment_names_none() {
        // Setting the variable needs `unsafe` in edition 2024, which this crate forbids.
        if std::env::var_os(FORGE_CARDS_ENV).is_some() {
            return;
        }
        let configured = Path::new("/somewhere/cardsfolder");
        assert_eq!(ForgeSource::find(Some(configured)), ForgeSource::Dir(configured.to_path_buf()));
    }

    #[test]
    fn a_creatures_power_is_kept_and_a_variable_one_is_none() {
        let s = Script::parse("Name:Test Elf\nManaCost:1 G\nTypes:Creature Elf\nPT:2/3\n").expect("a script");
        assert_eq!(s.front().power(), Some(2));
        let s = Script::parse("Name:Test Wurm\nManaCost:5 G\nTypes:Creature Wurm\nPT:*/*\n").expect("a script");
        assert_eq!(s.front().power(), None, "a variable power isn't a number");
        assert_eq!(Script::parse(TAPLAND).unwrap().front().power(), None, "a land has none");
    }

    #[test]
    fn a_database_of_an_older_schema_is_rebuilt_from_the_kept_download() {
        let dir = tempfile::tempdir().unwrap();
        let (db_path, raw_path) = (dir.path().join("forge.json.gz"), dir.path().join("raw.json.gz"));
        let raw = ForgeRaw { source: "forge-test".into(), files: vec![("t/test_grove.txt".into(), TAPLAND.into())] };
        write_json_gz(&raw_path, &raw).unwrap();
        let mut old = ForgeDb::from_raw(read_json_gz(&raw_path).unwrap());
        old.schema = FORGE_SCHEMA - 1;
        old.save(&db_path).unwrap();
        let db = ForgeSource::Download.load(&db_path, &raw_path).unwrap().expect("rebuilt from the download");
        assert_eq!((db.schema, db.source.as_str()), (FORGE_SCHEMA, "forge-test"));
        assert!(db.get("Test Grove").is_some());
        assert_eq!(ForgeDb::load(&db_path).unwrap().schema, FORGE_SCHEMA, "saved for next time");
        let none = dir.path().join("none.json.gz");
        assert!(ForgeSource::Download.load(&none, &none).unwrap().is_none(), "nothing to rebuild from: no scripts");
    }

    #[test]
    fn scripts_survive_saving_and_loading() {
        let dir = tempfile::tempdir().unwrap();
        let cards = dir.path().join("cardsfolder");
        fs::create_dir_all(cards.join("t")).unwrap();
        fs::write(cards.join("t/test_grove.txt"), TAPLAND).unwrap();
        fs::write(cards.join("t/test_bloom.txt"), MODAL).unwrap();
        let db = ForgeDb::from_dir(&cards).unwrap();
        let path = dir.path().join("forge.json.gz");
        db.save(&path).unwrap();
        let back = ForgeDb::load(&path).unwrap();
        assert_eq!(back.scripts, db.scripts);
        assert!(back.get("Test Bloom // Test Bloomfield").is_some(), "the index is rebuilt on loading");
    }
}
