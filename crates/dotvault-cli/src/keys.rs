//! Choosing variables by name or by glob, for `get` and `show --keys`.

use anyhow::{Context, Result};
use globset::Glob;

/// Whether `text` is a glob such as `RAZORPAY_*` and not a plain name.
pub fn is_glob(text: &str) -> bool {
    text.contains(['*', '?', '[', '{'])
}

pub struct Selection<'a> {
    /// The chosen variables, once each: in the order the patterns were given,
    /// and in file order within one glob.
    pub chosen: Vec<(String, String)>,
    /// Patterns that matched nothing.
    pub missing: Vec<&'a str>,
}

/// Picks from `pairs` what `patterns` name. A plain name matches exactly and a
/// glob matches case-sensitively, because variable names are case-sensitive.
pub fn select<'a>(pairs: &[(String, String)], patterns: &'a [String]) -> Result<Selection<'a>> {
    let mut chosen: Vec<(String, String)> = Vec::new();
    let mut missing = Vec::new();
    for pattern in patterns {
        let hits: Vec<&(String, String)> = if is_glob(pattern) {
            let matcher = Glob::new(pattern)
                .with_context(|| format!("invalid pattern {pattern:?}"))?
                .compile_matcher();
            pairs.iter().filter(|(k, _)| matcher.is_match(k)).collect()
        } else {
            pairs.iter().filter(|(k, _)| k == pattern).collect()
        };
        if hits.is_empty() {
            missing.push(pattern.as_str());
        }
        for pair in hits {
            if !chosen.iter().any(|(k, _)| *k == pair.0) {
                chosen.push(pair.clone());
            }
        }
    }
    Ok(Selection { chosen, missing })
}

/// What to tell the user about a pattern that matched nothing.
pub fn describe_missing(pattern: &str) -> String {
    if is_glob(pattern) {
        format!("no variable matches {pattern}")
    } else {
        format!("{pattern} is not set")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs() -> Vec<(String, String)> {
        [
            ("DATABASE_URL", "db"),
            ("RAZORPAY_KEY_ID", "id"),
            ("RAZORPAY_KEY_SECRET", "secret"),
            ("razorpay_lower", "low"),
            ("LIVEKIT_URL", "lk"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    fn names(sel: &Selection) -> Vec<String> {
        sel.chosen.iter().map(|(k, _)| k.clone()).collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plain_names_match_exactly_and_keep_the_order_given() {
        let want = strings(&["LIVEKIT_URL", "DATABASE_URL"]);
        let sel = select(&pairs(), &want).unwrap();
        assert_eq!(names(&sel), ["LIVEKIT_URL", "DATABASE_URL"]);
        assert!(sel.missing.is_empty());
    }

    #[test]
    fn a_plain_name_is_case_sensitive() {
        let want = strings(&["database_url"]);
        let sel = select(&pairs(), &want).unwrap();
        assert!(sel.chosen.is_empty());
        assert_eq!(sel.missing, ["database_url"]);
    }

    #[test]
    fn a_glob_picks_every_match_in_file_order_and_is_case_sensitive() {
        let want = strings(&["RAZORPAY_*"]);
        let sel = select(&pairs(), &want).unwrap();
        assert_eq!(names(&sel), ["RAZORPAY_KEY_ID", "RAZORPAY_KEY_SECRET"]);
        let want = strings(&["*_URL", "RAZORPAY_KEY_?D"]);
        let sel = select(&pairs(), &want).unwrap();
        assert_eq!(
            names(&sel),
            ["DATABASE_URL", "LIVEKIT_URL", "RAZORPAY_KEY_ID"]
        );
    }

    #[test]
    fn a_variable_named_twice_is_chosen_once() {
        let want = strings(&["RAZORPAY_KEY_ID", "RAZORPAY_*", "RAZORPAY_KEY_ID"]);
        let sel = select(&pairs(), &want).unwrap();
        assert_eq!(names(&sel), ["RAZORPAY_KEY_ID", "RAZORPAY_KEY_SECRET"]);
        assert!(sel.missing.is_empty());
    }

    #[test]
    fn what_matched_nothing_is_reported_and_the_rest_is_kept() {
        let want = strings(&["DATABASE_URL", "NOPE", "ZZZ_*"]);
        let sel = select(&pairs(), &want).unwrap();
        assert_eq!(names(&sel), ["DATABASE_URL"]);
        assert_eq!(sel.missing, ["NOPE", "ZZZ_*"]);
        assert_eq!(describe_missing("NOPE"), "NOPE is not set");
        assert_eq!(describe_missing("ZZZ_*"), "no variable matches ZZZ_*");
    }

    #[test]
    fn a_broken_glob_is_an_error_not_a_miss() {
        let want = strings(&["RAZORPAY_[*"]);
        let err = select(&pairs(), &want)
            .err()
            .expect("must fail")
            .to_string();
        assert!(err.contains("invalid pattern"), "{err}");
    }

    #[test]
    fn only_wildcard_characters_make_a_glob() {
        for glob in ["A*", "A?", "A[12]", "A{B,C}"] {
            assert!(is_glob(glob), "{glob}");
        }
        for plain in ["A", "A_B", "a.b", "A-B"] {
            assert!(!is_glob(plain), "{plain}");
        }
    }
}
