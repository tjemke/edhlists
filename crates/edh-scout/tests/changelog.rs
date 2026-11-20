//! CHANGELOG.md keeps up with the version (see "Versioning" in AGENTS.md).

#[test]
fn the_changelog_starts_with_this_version() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md");
    let text = std::fs::read_to_string(path).expect("CHANGELOG.md at the top of the repository");
    let newest = text.lines().find(|l| l.starts_with("## ")).expect("CHANGELOG.md has no version section");
    assert_eq!(
        newest,
        format!("## {}", env!("CARGO_PKG_VERSION")),
        "CHANGELOG.md's newest section should be this version's: bump the version and add its section together"
    );
}
