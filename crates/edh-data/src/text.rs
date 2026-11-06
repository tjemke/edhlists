//! Name handling: slugs, folding, and display helpers.

use unicode_normalization::UnicodeNormalization;

/// Fold to lowercase ASCII-ish for matching: "Juzám Djinn" -> "juzam djinn".
pub fn fold(name: &str) -> String {
    name.nfkd().filter(|c| c.is_ascii()).collect::<String>().to_lowercase().trim().to_string()
}

/// EDHREC's URL slug for a card name: front face only, ASCII, hyphenated.
pub fn slugify(name: &str) -> String {
    let front = name.split(" // ").next().unwrap_or(name);
    let s = fold(front).replace('\'', "").replace('+', "plus-");
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// EDHREC orders partner pairs alphabetically by slug.
pub fn commander_slug(commanders: &[String]) -> String {
    let mut slugs: Vec<String> = commanders.iter().map(|c| slugify(c)).collect();
    slugs.sort();
    slugs.join("-")
}

/// The front face of a possibly double-faced name.
pub fn front_face(name: &str) -> &str {
    name.split(" // ").next().unwrap_or(name)
}

/// Civil date (UTC) `days` days before today, as YYYY-MM-DD.
pub fn date_days_ago(days: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    civil_date((now / 86_400).saturating_sub(days) as i64)
}

pub fn today() -> String {
    date_days_ago(0)
}

/// Days since 1970-01-01 -> YYYY-MM-DD (Howard Hinnant's algorithm).
pub fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Remove reminder text: everything in parentheses.
pub fn strip_reminder(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0u32;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        for (name, want) in [
            ("Atraxa, Praetors' Voice", "atraxa-praetors-voice"),
            ("Krenko, Mob Boss", "krenko-mob-boss"),
            ("Jodah, the Unifier", "jodah-the-unifier"),
            ("Tergrid, God of Fright // Tergrid's Lantern", "tergrid-god-of-fright"),
            ("Juzám Djinn", "juzam-djinn"),
            ("Rick, Steadfast Leader", "rick-steadfast-leader"),
            ("Éomer, Marshal of Rohan", "eomer-marshal-of-rohan"),
        ] {
            assert_eq!(slugify(name), want, "{name}");
        }
    }

    #[test]
    fn partner_slug_is_alphabetical() {
        let pair = vec!["Tymna the Weaver".to_string(), "Kraum, Ludevic's Opus".to_string()];
        assert_eq!(commander_slug(&pair), "kraum-ludevics-opus-tymna-the-weaver");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_454), "2026-01-01");
        assert_eq!(civil_date(11_016), "2000-02-29");
    }

    #[test]
    fn reminder_text_is_dropped() {
        assert_eq!(strip_reminder("Cycling {2} ({2}, Discard this card: Draw a card.)"), "Cycling {2} ");
    }
}
