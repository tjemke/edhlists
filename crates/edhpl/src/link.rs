//! Links back to edhpowerlevel.com.

use std::sync::LazyLock;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;

/// Characters `encodeURIComponent` leaves alone: A-Z a-z 0-9 - _ . ! ~ * ' ( )
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

static BRACKETS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" *\[[^)]*\] *").unwrap());
static ANGLES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" *<[^)]*> *").unwrap());

/// Port of `EDHPowerLevelEncode()` from <https://edhpowerlevel.com/button/>: a link that opens a
/// decklist (one card per line, optional `Commander` / `Deck` headers) on the site, prefilled.
pub fn edhpowerlevel_url(lines: &[String]) -> String {
    let cleaned: Vec<String> = lines
        .iter()
        .map(|l| {
            let c = l.split('(').next().unwrap_or("");
            let c = BRACKETS_RE.replace_all(c, " ");
            let c = ANGLES_RE.replace_all(&c, " ");
            c.trim().to_string()
        })
        .collect();
    let enc = utf8_percent_encode(&cleaned.join("~"), URI_COMPONENT).to_string().replace("%20", "+");
    format!("https://edhpowerlevel.com?d={enc}~Z~")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_matches_the_site_button() {
        let url =
            edhpowerlevel_url(&["Commander".into(), "1 Jodah, the Unifier".into(), "1 Sol Ring (CMM) 464".into()]);
        assert_eq!(url, "https://edhpowerlevel.com?d=Commander~1+Jodah%2C+the+Unifier~1+Sol+Ring~Z~");
        let url = edhpowerlevel_url(&["1 Gaea's Cradle [Lands]".into(), "1 Éomer, Marshal of Rohan".into()]);
        assert_eq!(url, "https://edhpowerlevel.com?d=1+Gaea's+Cradle~1+%C3%89omer%2C+Marshal+of+Rohan~Z~");
    }
}
