//! Riot locale validation shared by settings and the replay launcher.

/// Normalizes a Riot locale such as `es_mx` to `es_MX`.
///
/// Accepts exactly two ASCII letters, `_`, two ASCII letters (surrounding
/// whitespace is ignored). Anything else returns `None`, so the value is always
/// safe to pass as a single `-Locale=` argument.
pub fn normalize_locale(locale: &str) -> Option<String> {
    let (language, territory) = locale.trim().split_once('_')?;
    let valid = |part: &str| part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic());
    (valid(language) && valid(territory)).then(|| {
        format!(
            "{}_{}",
            language.to_ascii_lowercase(),
            territory.to_ascii_uppercase()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_valid_locales() {
        assert_eq!(normalize_locale("en_US"), Some("en_US".to_string()));
        assert_eq!(normalize_locale("es_mx"), Some("es_MX".to_string()));
        assert_eq!(normalize_locale(" KO_kr "), Some("ko_KR".to_string()));
    }

    #[test]
    fn rejects_malformed_locales() {
        for bad in [
            "",
            "english",
            "en-US",
            "en_USA",
            "e_US",
            "en_U1",
            "en_US --bad",
            "en_US_x",
            "é_US",
            "_",
        ] {
            assert_eq!(normalize_locale(bad), None, "{bad:?} should be rejected");
        }
    }
}
