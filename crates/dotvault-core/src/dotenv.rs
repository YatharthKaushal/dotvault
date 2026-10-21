//! Lossless `.env` parser and editor.
//!
//! `DotEnv::parse(s).to_string() == s` holds for **every** input. Lines that
//! are not understood (comments, blanks, garbage) are kept verbatim, and an
//! entry that is never modified is written back byte-for-byte. Editing a value
//! only re-renders that one entry, keeping its indent, `export` prefix,
//! spacing around `=` and trailing comment.
//!
//! Values we *write* may come from another person (a share), and plenty of
//! people `source` their `.env` in a shell. So the writer only leaves a value
//! unquoted when every character is inert to a shell, prefers single quotes
//! (inert) for anything with `$`, `` ` ``, `\` or `"`, and always escapes `$`
//! and `` ` `` inside double quotes. Backtick quoting, which shells treat as
//! command substitution, is never produced.

use std::collections::HashMap;
use std::fmt;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    None,
    Single,
    Double,
    Backtick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    key: String,
    value: String,
    quote: Quote,
    export: bool,
    indent: String,
    export_kw: String,
    sep: String,
    tail: String,
    /// Original text, kept until the value is modified.
    raw: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Raw(String),
    Entry(Entry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    content: Content,
    eol: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DotEnv {
    lines: Vec<Line>,
}

impl Entry {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn quote(&self) -> Quote {
        self.quote
    }

    pub fn is_export(&self) -> bool {
        self.export
    }

    fn new(key: &str, value: &str) -> Self {
        Entry {
            key: key.to_string(),
            value: value.to_string(),
            quote: choose_quote(Quote::None, value),
            export: false,
            indent: String::new(),
            export_kw: String::new(),
            sep: "=".to_string(),
            tail: String::new(),
            raw: None,
        }
    }

    fn set_value(&mut self, value: &str) {
        if self.value == value {
            return;
        }
        self.quote = choose_quote(self.quote, value);
        self.value = value.to_string();
        self.raw = None;
    }

    fn render(&self) -> String {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let mut s = String::with_capacity(self.key.len() + self.value.len() + 8);
        s.push_str(&self.indent);
        s.push_str(&self.export_kw);
        s.push_str(&self.key);
        s.push_str(&self.sep);
        match self.quote {
            Quote::None => s.push_str(&self.value),
            Quote::Single => {
                s.push('\'');
                s.push_str(&self.value);
                s.push('\'');
            }
            Quote::Backtick => {
                s.push('`');
                s.push_str(&self.value);
                s.push('`');
            }
            Quote::Double => {
                s.push('"');
                for c in self.value.chars() {
                    match c {
                        '\\' => s.push_str("\\\\"),
                        '"' => s.push_str("\\\""),
                        '$' => s.push_str("\\$"),
                        '`' => s.push_str("\\`"),
                        '\n' => s.push_str("\\n"),
                        '\r' => s.push_str("\\r"),
                        '\t' => s.push_str("\\t"),
                        c => s.push(c),
                    }
                }
                s.push('"');
            }
        }
        s.push_str(&self.tail);
        s
    }
}

impl DotEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse(input: &str) -> Self {
        let mut lines = Vec::new();
        let mut pos = 0;
        while pos < input.len() {
            let (end, eol_len) = line_bounds(input, pos);
            if let Some((entry, content_end, eol_len)) = parse_entry(input, pos, end, eol_len) {
                lines.push(Line {
                    content: Content::Entry(entry),
                    eol: input[content_end..content_end + eol_len].to_string(),
                });
                pos = content_end + eol_len;
            } else {
                lines.push(Line {
                    content: Content::Raw(input[pos..end].to_string()),
                    eol: input[end..end + eol_len].to_string(),
                });
                pos = end + eol_len;
            }
        }
        DotEnv { lines }
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.lines.iter().filter_map(|l| match &l.content {
            Content::Entry(e) => Some(e),
            Content::Raw(_) => None,
        })
    }

    fn entries_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut Entry> {
        self.lines.iter_mut().filter_map(|l| match &mut l.content {
            Content::Entry(e) => Some(e),
            Content::Raw(_) => None,
        })
    }

    /// Value of `key`. If the key appears more than once the last one wins,
    /// like sourcing the file in a shell.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries()
            .rev()
            .find(|e| e.key == key)
            .map(Entry::value)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries().any(|e| e.key == key)
    }

    /// Effective `(key, value)` pairs: one per distinct key, in order of first
    /// appearance, carrying the winning (last) value.
    pub fn pairs(&self) -> Vec<(&str, &str)> {
        let mut index: HashMap<&str, usize> = HashMap::new();
        let mut out: Vec<(&str, &str)> = Vec::new();
        for e in self.entries() {
            match index.get(e.key()) {
                Some(&i) => out[i].1 = e.value(),
                None => {
                    index.insert(e.key(), out.len());
                    out.push((e.key(), e.value()));
                }
            }
        }
        out
    }

    /// Sets `key`, updating the last occurrence in place or appending a new line.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        if !is_valid_key(key) {
            return Err(Error::InvalidKey(key.to_string()));
        }
        if let Some(entry) = self.entries_mut().rev().find(|e| e.key == key) {
            entry.set_value(value);
            return Ok(());
        }
        let eol = self.dominant_eol();
        if let Some(last) = self.lines.last_mut() {
            if last.eol.is_empty() {
                last.eol = eol.clone();
            }
        }
        self.lines.push(Line {
            content: Content::Entry(Entry::new(key, value)),
            eol,
        });
        Ok(())
    }

    /// Removes every occurrence of `key`. Returns how many were removed.
    pub fn unset(&mut self, key: &str) -> usize {
        let before = self.lines.len();
        self.lines
            .retain(|l| !matches!(&l.content, Content::Entry(e) if e.key == key));
        before - self.lines.len()
    }

    fn dominant_eol(&self) -> String {
        self.lines
            .iter()
            .find(|l| !l.eol.is_empty())
            .map(|l| l.eol.clone())
            .unwrap_or_else(|| "\n".to_string())
    }
}

impl fmt::Display for DotEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.lines {
            match &line.content {
                Content::Raw(s) => f.write_str(s)?,
                Content::Entry(e) => f.write_str(&e.render())?,
            }
            f.write_str(&line.eol)?;
        }
        Ok(())
    }
}

pub fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if is_key_char(c, true) => chars.all(|c| is_key_char(c, false)),
        _ => false,
    }
}

fn is_key_char(c: char, first: bool) -> bool {
    if first {
        c.is_ascii_alphabetic() || c == '_'
    } else {
        c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
    }
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Returns `(content_end, eol_len)` for the physical line starting at `from`.
fn line_bounds(input: &str, from: usize) -> (usize, usize) {
    match input[from..].find('\n') {
        None => (input.len(), 0),
        Some(rel) => {
            let nl = from + rel;
            if nl > from && input.as_bytes()[nl - 1] == b'\r' {
                (nl - 1, 2)
            } else {
                (nl, 1)
            }
        }
    }
}

/// Quote style for a value being written, keeping the entry's current style
/// when that is safe.
fn choose_quote(orig: Quote, value: &str) -> Quote {
    let needs_single = value.contains(['$', '`', '\\', '"']);
    match orig {
        Quote::Single if !value.contains('\'') => Quote::Single,
        Quote::Double => Quote::Double,
        Quote::None if value.chars().all(is_shell_inert) => Quote::None,
        _ if value.contains(['\n', '\r']) => Quote::Double,
        _ if needs_single && !value.contains('\'') => Quote::Single,
        _ => Quote::Double,
    }
}

/// Characters that mean nothing to a shell even when unquoted (so `source .env`
/// cannot be turned into code execution by a value).
fn is_shell_inert(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-')
}

/// Tries to parse an entry starting at `start`. Returns the entry, the end of
/// its content (before the line terminator) and the terminator length.
fn parse_entry(
    input: &str,
    start: usize,
    line_end: usize,
    line_eol: usize,
) -> Option<(Entry, usize, usize)> {
    let text = &input[start..line_end];
    let body = text.trim_start_matches([' ', '\t']);
    let indent = &text[..text.len() - body.len()];
    if body.is_empty() || body.starts_with('#') {
        return None;
    }

    // `export KEY=...`; if that does not parse, retry treating `export` as the key.
    if let Some(after) = body.strip_prefix("export") {
        if after.starts_with([' ', '\t']) {
            let rest = after.trim_start_matches([' ', '\t']);
            let export_kw = &body[..body.len() - rest.len()];
            if let Some(r) =
                parse_assignment(input, start, line_end, line_eol, indent, export_kw, rest)
            {
                return Some(r);
            }
        }
    }
    parse_assignment(input, start, line_end, line_eol, indent, "", body)
}

fn parse_assignment(
    input: &str,
    start: usize,
    line_end: usize,
    line_eol: usize,
    indent: &str,
    export_kw: &str,
    rest: &str,
) -> Option<(Entry, usize, usize)> {
    let text = &input[start..line_end];

    let key_len = rest
        .char_indices()
        .find(|&(i, c)| !is_key_char(c, i == 0))
        .map_or(rest.len(), |(i, _)| i);
    if key_len == 0 {
        return None;
    }
    let key = &rest[..key_len];
    let after_key = &rest[key_len..];
    let after_ws = after_key.trim_start_matches([' ', '\t']);
    let after_eq = after_ws.strip_prefix('=')?;
    let value_part = after_eq.trim_start_matches([' ', '\t']);
    let sep = &rest[key_len..rest.len() - value_part.len()];
    let value_off = text.len() - value_part.len();
    let value_start = start + value_off;

    let quote = match value_part.as_bytes().first() {
        Some(b'"') => Quote::Double,
        Some(b'\'') => Quote::Single,
        Some(b'`') => Quote::Backtick,
        _ => Quote::None,
    };

    if quote != Quote::None {
        let q = value_part.as_bytes()[0];
        if let Some(close) = find_closing(input, value_start + 1, q, quote == Quote::Double) {
            let (content_end, eol_len) = line_bounds(input, close);
            let inner = &input[value_start + 1..close];
            let value = if quote == Quote::Double {
                unescape_double(inner)
            } else {
                inner.to_string()
            };
            let entry = Entry {
                key: key.to_string(),
                value,
                quote,
                export: !export_kw.is_empty(),
                indent: indent.to_string(),
                export_kw: export_kw.to_string(),
                sep: sep.to_string(),
                tail: input[close + 1..content_end].to_string(),
                raw: Some(input[start..content_end].to_string()),
            };
            return Some((entry, content_end, eol_len));
        }
        // No closing quote anywhere: fall through and read the line as a plain value.
    }

    let region = &input[value_start..line_end];
    let mut prev_blank = sep.as_bytes().last().is_some_and(|&b| is_blank(b));
    let mut value_end = region.len();
    for (i, &b) in region.as_bytes().iter().enumerate() {
        if b == b'#' && prev_blank {
            value_end = i;
            break;
        }
        prev_blank = is_blank(b);
    }
    let value = region[..value_end].trim_end_matches([' ', '\t']);
    let entry = Entry {
        key: key.to_string(),
        value: value.to_string(),
        quote: Quote::None,
        export: !export_kw.is_empty(),
        indent: indent.to_string(),
        export_kw: export_kw.to_string(),
        sep: sep.to_string(),
        tail: region[value.len()..].to_string(),
        raw: Some(text.to_string()),
    };
    Some((entry, line_end, line_eol))
}

fn find_closing(input: &str, from: usize, quote: u8, escapes: bool) -> Option<usize> {
    let b = input.as_bytes();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'\\' if escapes => i += 2,
            c if c == quote => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn unescape_double(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('$') => out.push('$'),
            Some('`') => out.push('`'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn roundtrip(s: &str) {
        assert_eq!(DotEnv::parse(s).to_string(), s, "input: {s:?}");
    }

    #[test]
    fn roundtrips_assorted_files() {
        for s in [
            "",
            "\n",
            "A=1",
            "A=1\n",
            "# comment\n\nA=1\nB = 2 # trailing\n",
            "export A=1\r\nB=\"x y\"\r\n",
            "  INDENT=1\n",
            "A=\"multi\nline\"\nB=2",
            "A='single # not a comment'\n",
            "garbage line\n=novalue\nexport\nexport A\n",
            "A=\"unterminated\nB=2\n",
            "A=`tick`\n",
            "A=1\r\n\r\nB=2\r\n",
            "ünï=çödé\nK=ünï\n",
        ] {
            roundtrip(s);
        }
    }

    #[test]
    fn parses_values() {
        let env = DotEnv::parse(
            "PLAIN=abc\nSP = padded value  # note\nDQ=\"a\\nb\\\"c\"\nSQ='lit\\n'\nBT=`t`\nEMPTY=\nURL=http://x/#frag\nexport EX=1\n",
        );
        assert_eq!(env.get("PLAIN"), Some("abc"));
        assert_eq!(env.get("SP"), Some("padded value"));
        assert_eq!(env.get("DQ"), Some("a\nb\"c"));
        assert_eq!(env.get("SQ"), Some("lit\\n"));
        assert_eq!(env.get("BT"), Some("t"));
        assert_eq!(env.get("EMPTY"), Some(""));
        assert_eq!(env.get("URL"), Some("http://x/#frag"));
        assert_eq!(env.get("EX"), Some("1"));
        assert!(env.entries().any(|e| e.key() == "EX" && e.is_export()));
    }

    #[test]
    fn parses_multiline_double_quoted() {
        let env = DotEnv::parse("KEY=\"-----BEGIN\nabc\n-----END\"\nNEXT=1\n");
        assert_eq!(env.get("KEY"), Some("-----BEGIN\nabc\n-----END"));
        assert_eq!(env.get("NEXT"), Some("1"));
    }

    #[test]
    fn last_duplicate_wins_and_pairs_dedupe() {
        let env = DotEnv::parse("A=1\nB=2\nA=3\n");
        assert_eq!(env.get("A"), Some("3"));
        assert_eq!(env.pairs(), vec![("A", "3"), ("B", "2")]);
    }

    #[test]
    fn set_preserves_comments_and_layout() {
        let mut env = DotEnv::parse("# db\nexport DB = old # keep me\n\nOTHER=1\n");
        env.set("DB", "new").unwrap();
        assert_eq!(
            env.to_string(),
            "# db\nexport DB = new # keep me\n\nOTHER=1\n"
        );
    }

    #[test]
    fn set_untouched_when_value_equal() {
        let src = "A = \"x\"  # c\n";
        let mut env = DotEnv::parse(src);
        env.set("A", "x").unwrap();
        assert_eq!(env.to_string(), src);
    }

    #[test]
    fn set_quotes_when_needed() {
        let mut env = DotEnv::parse("A=1\n");
        env.set("A", "has space").unwrap();
        env.set("B", "x#y").unwrap();
        env.set("C", "line1\nline2").unwrap();
        assert_eq!(
            env.to_string(),
            "A=\"has space\"\nB=\"x#y\"\nC=\"line1\\nline2\"\n"
        );
        let again = DotEnv::parse(&env.to_string());
        assert_eq!(again.get("C"), Some("line1\nline2"));
    }

    #[test]
    fn set_keeps_single_quote_style() {
        let mut env = DotEnv::parse("A='x'\n");
        env.set("A", "a b $c").unwrap();
        assert_eq!(env.to_string(), "A='a b $c'\n");
        env.set("A", "it's").unwrap();
        assert_eq!(env.to_string(), "A=\"it's\"\n");
    }

    #[test]
    fn set_appends_with_matching_eol() {
        let mut env = DotEnv::parse("A=1");
        env.set("B", "2").unwrap();
        assert_eq!(env.to_string(), "A=1\nB=2\n");

        let mut crlf = DotEnv::parse("A=1\r\n");
        crlf.set("B", "2").unwrap();
        assert_eq!(crlf.to_string(), "A=1\r\nB=2\r\n");

        let mut empty = DotEnv::new();
        empty.set("X", "y").unwrap();
        assert_eq!(empty.to_string(), "X=y\n");
    }

    #[test]
    fn set_rejects_bad_keys() {
        let mut env = DotEnv::new();
        assert!(env.set("1BAD", "x").is_err());
        assert!(env.set("has space", "x").is_err());
        assert!(env.set("", "x").is_err());
    }

    #[test]
    fn unset_removes_all_occurrences() {
        let mut env = DotEnv::parse("A=1\n# c\nB=2\nA=3\n");
        assert_eq!(env.unset("A"), 2);
        assert_eq!(env.unset("A"), 0);
        assert_eq!(env.to_string(), "# c\nB=2\n");
    }

    #[test]
    fn set_updates_last_duplicate() {
        let mut env = DotEnv::parse("A=1\nA=2\n");
        env.set("A", "9").unwrap();
        assert_eq!(env.to_string(), "A=1\nA=9\n");
    }

    #[test]
    fn writer_quotes_by_what_a_shell_would_do() {
        let render = |v: &str| {
            let mut env = DotEnv::new();
            env.set("K", v).unwrap();
            env.to_string()
        };
        // inert characters stay bare, so ordinary values look ordinary
        assert_eq!(
            render("postgres://u:p@db.internal:5432/app"),
            "K=postgres://u:p@db.internal:5432/app\n"
        );
        assert_eq!(render("4000"), "K=4000\n");
        assert_eq!(render(""), "K=\n");
        // shell-active characters get quoted, single quotes preferred
        assert_eq!(render("$(touch x)"), "K='$(touch x)'\n");
        assert_eq!(render("`id`"), "K='`id`'\n");
        assert_eq!(render("a\\b"), "K='a\\b'\n");
        assert_eq!(render("say \"hi\""), "K='say \"hi\"'\n");
        // plain specials without expansion get double quotes
        assert_eq!(render("a;b"), "K=\"a;b\"\n");
        assert_eq!(render("https://x/?a=1&b=2"), "K=\"https://x/?a=1&b=2\"\n");
        assert_eq!(render("~/x"), "K=\"~/x\"\n");
        // a single quote forces double quotes, which then escape $ and `
        assert_eq!(render("it's $HOME"), "K=\"it's \\$HOME\"\n");
    }

    #[test]
    fn escaped_dollar_and_backtick_roundtrip() {
        let mut env = DotEnv::new();
        env.set("K", "it's `x` $y \\ \"q\"").unwrap();
        assert_eq!(
            DotEnv::parse(&env.to_string()).get("K"),
            Some("it's `x` $y \\ \"q\"")
        );
    }

    #[test]
    fn modifying_a_value_never_keeps_backtick_quotes_and_always_escapes_in_double() {
        let mut env = DotEnv::parse("A=`x`\nB=\"x\"\n");
        env.set("A", "$(evil)").unwrap();
        env.set("B", "$(evil)").unwrap();
        assert_eq!(env.to_string(), "A='$(evil)'\nB=\"\\$(evil)\"\n");
    }

    /// The claim that matters: a file we write can be `source`d by a shell
    /// without running anything the value contains, and every value reads back
    /// exactly. Runs a real `sh`.
    #[cfg(unix)]
    #[test]
    fn written_files_are_inert_when_sourced_by_sh() {
        use std::process::Command;

        let hostile = [
            "$(touch MARKER)",
            "`touch MARKER`",
            "\"; touch MARKER; \"",
            "'; touch MARKER; '",
            "it's $(touch MARKER)",
            "a;touch MARKER",
            "a&touch MARKER",
            "a|touch MARKER",
            "a\nb;touch MARKER",
            "$HOME",
            "x${HOME}y",
            "a\\b",
            "a\\\"b'c$d`e",
            "~/x",
            "~root",
            "*",
            "# not a comment",
            "a b  c",
            "!bang",
            "(",
            "))",
            "<(touch MARKER)",
            "",
        ];
        let dir = std::env::temp_dir().join(format!("dotvault-sh-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (i, value) in hostile.iter().enumerate() {
            let file = dir.join(format!("case{i}.env"));
            // fresh file, and also an update of existing entries in each quote style
            for seed in ["", "K=old\n", "K='old'\n", "K=\"old\"\n", "K=`old`\n"] {
                let mut env = DotEnv::parse(seed);
                env.set("K", value).unwrap();
                std::fs::write(&file, env.to_string()).unwrap();

                let _ = std::fs::remove_file(dir.join("MARKER"));
                let out = Command::new("sh")
                    .current_dir(&dir)
                    .arg("-c")
                    .arg(". \"$1\" && printf %s \"$K\"")
                    .arg("sh")
                    .arg(&file)
                    .output()
                    .unwrap();
                assert!(
                    !dir.join("MARKER").exists(),
                    "sourcing executed the value {value:?} (seed {seed:?}):\n{}",
                    std::fs::read_to_string(&file).unwrap()
                );
                assert!(
                    out.status.success(),
                    "sh failed for {value:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                // `sh` keeps backslash-n literal inside quotes, dotenv parsers turn it into a
                // newline, so only compare values without line breaks byte for byte.
                if !value.contains('\n') {
                    assert_eq!(
                        String::from_utf8_lossy(&out.stdout),
                        *value,
                        "value changed by sourcing (seed {seed:?}):\n{}",
                        std::fs::read_to_string(&file).unwrap()
                    );
                }
                // and our own parser reads it back exactly
                assert_eq!(
                    DotEnv::parse(&std::fs::read_to_string(&file).unwrap()).get("K"),
                    Some(*value)
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    proptest! {
        #[test]
        fn parse_then_print_is_identity(s in "\\PC{0,200}") {
            roundtrip(&s);
        }

        #[test]
        fn parse_then_print_is_identity_envish(
            s in "([A-Za-z_]{1,6} ?= ?(\"[a-z \\\\n#]{0,6}\"|'[a-z ]{0,6}'|[a-z#]{0,6})( #[a-z]{0,4})?\r?\n|# [a-z]{0,5}\n|\n){0,8}"
        ) {
            roundtrip(&s);
        }

        #[test]
        fn set_then_get_returns_value(key in "[A-Za-z_][A-Za-z0-9_]{0,8}", value in "\\PC{0,30}") {
            let mut env = DotEnv::parse("# header\nEXISTING=1\n");
            env.set(&key, &value).unwrap();
            let reparsed = DotEnv::parse(&env.to_string());
            prop_assert_eq!(reparsed.get(&key), Some(value.as_str()));
        }
    }
}
