//! Terminal output: masking, aligned tables, value formats.
//!
//! Everything is written through `anstream`, which strips colors when stdout
//! is not a terminal or `NO_COLOR` is set, so callers can style freely.

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::ValueEnum;
use dotvault_core::dotenv::DotEnv;
use owo_colors::OwoColorize;
use unicode_width::UnicodeWidthStr;

pub const MASK: &str = "********";

/// True for characters that can do something to a terminal or mislead the eye:
/// control characters (ESC starts every escape sequence, BEL, CR, ...) and the
/// Unicode directional overrides that reorder text.
fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

/// Makes text from somewhere else (a share, a server, a file in a cloned repo)
/// safe to print. Dangerous characters become visible escapes like `\u{1b}`,
/// so a hostile value cannot rewrite the screen, set the window title or write
/// to the clipboard.
pub fn sanitize(text: &str) -> Cow<'_, str> {
    escape_unsafe(text, false)
}

/// Like [`sanitize`] but keeps real line breaks and tabs, for multi-line values
/// shown in a machine format (a PEM key should still look like one).
pub fn sanitize_keeping_layout(text: &str) -> Cow<'_, str> {
    escape_unsafe(text, true)
}

fn escape_unsafe(text: &str, keep_layout: bool) -> Cow<'_, str> {
    if !text.chars().any(is_unsafe_char) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\n' | '\t' if keep_layout => out.push(c),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if is_unsafe_char(c) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// What to print instead of a secret.
pub fn masked(value: &str) -> &'static str {
    if value.is_empty() {
        "(empty)"
    } else {
        MASK
    }
}

pub fn shown(value: &str, reveal: bool) -> String {
    if reveal {
        value.to_string()
    } else {
        masked(value).to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Aligned KEY / VALUE columns
    Table,
    /// KEY=VALUE lines
    Dotenv,
    /// A JSON object
    Json,
    /// `export KEY='value'` for POSIX shells
    Shell,
    /// `$env:KEY = 'value'` for PowerShell
    Powershell,
}

/// Whether `key` can be emitted in `format` without being able to smuggle
/// anything: keys from a share are arbitrary text, and these formats are meant
/// to be piped into `eval`, `source` or a file.
fn name_ok(format: Format, key: &str) -> bool {
    match format {
        Format::Shell | Format::Powershell => {
            let mut chars = key.chars();
            matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        Format::Dotenv => dotvault_core::dotenv::is_valid_key(key),
        Format::Table | Format::Json => true,
    }
}

/// How many of `pairs` [`render_pairs`] leaves out because the name is unusable.
pub fn skipped_names(format: Format, pairs: &[(String, String)]) -> usize {
    pairs.iter().filter(|(k, _)| !name_ok(format, k)).count()
}

/// [`render_pairs`], plus a warning on stderr when some variables had to be left out.
///
/// Piped, the output is exact data. On a terminal, control characters in names
/// and values are made visible instead, so showing a hostile value cannot
/// drive the terminal.
pub fn render_pairs_warn(format: Format, pairs: &[(String, String)], reveal: bool) -> String {
    use std::io::IsTerminal;
    render_pairs_for(io::stdout().is_terminal(), format, pairs, reveal)
}

/// [`render_pairs_warn`] with the terminal check made by the caller, so that
/// something that is not going to the screen (a copy) can ask for exact data.
pub fn render_pairs_for(
    terminal: bool,
    format: Format,
    pairs: &[(String, String)],
    reveal: bool,
) -> String {
    if terminal {
        let safe: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| {
                (
                    sanitize(k).into_owned(),
                    sanitize_keeping_layout(v).into_owned(),
                )
            })
            .collect();
        return render_pairs_checked(format, &safe, reveal);
    }
    render_pairs_checked(format, pairs, reveal)
}

fn render_pairs_checked(format: Format, pairs: &[(String, String)], reveal: bool) -> String {
    let skipped = skipped_names(format, pairs);
    if skipped > 0 {
        anstream::eprintln!(
            "{} left out {} with a name that cannot be used safely in this format (names look like MY_VAR)",
            "warning:".yellow().bold(),
            plural(skipped, "variable")
        );
    }
    render_pairs(format, pairs, reveal)
}

/// Renders `pairs` in a machine-friendly format. Values are masked unless
/// `reveal` is set, so these formats are safe to print by accident.
pub fn render_pairs(format: Format, pairs: &[(String, String)], reveal: bool) -> String {
    match format {
        Format::Table => unreachable!("tables are rendered by the caller"),
        Format::Dotenv => {
            let mut env = DotEnv::new();
            for (k, v) in pairs.iter().filter(|(k, _)| name_ok(format, k)) {
                let _ = env.set(k, &shown(v, reveal));
            }
            env.to_string()
        }
        Format::Json => {
            let mut map = serde_json::Map::new();
            for (k, v) in pairs {
                map.insert(k.clone(), serde_json::Value::String(shown(v, reveal)));
            }
            let mut s = serde_json::to_string_pretty(&map).expect("map serializes");
            s.push('\n');
            s
        }
        Format::Shell => pairs
            .iter()
            .filter(|(k, _)| name_ok(format, k))
            .map(|(k, v)| format!("export {k}={}\n", sh_quote(&shown(v, reveal))))
            .collect(),
        Format::Powershell => pairs
            .iter()
            .filter(|(k, _)| name_ok(format, k))
            .map(|(k, v)| format!("$env:{k} = {}\n", ps_quote(&shown(v, reveal))))
            .collect(),
    }
}

pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Buffered, color-aware stdout. Without the buffer every table row and JSON
/// token would be its own `write` syscall. Flushes when dropped; a closed pipe
/// (`dotvault ls | head`) is then ignored rather than a panic.
pub struct Out(io::BufWriter<anstream::AutoStream<io::Stdout>>);

pub fn stdout() -> Out {
    Out(io::BufWriter::new(anstream::stdout()))
}

impl Out {
    /// Kept so call sites read `stdout().lock()`; the buffer is already exclusive.
    pub fn lock(self) -> Out {
        self
    }
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

pub fn print_json<T: serde::Serialize>(value: &T) -> io::Result<()> {
    let mut out = stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)
}

// --- paths and times --------------------------------------------------------

/// `/home/me/code/x` -> `~/code/x`
pub fn tilde(path: &Path) -> String {
    sanitize(&tilde_raw(path)).into_owned()
}

fn tilde_raw(path: &Path) -> String {
    static HOME: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    let home = HOME.get_or_init(crate::discover::home_dir);
    if let Some(home) = home {
        if let Ok(rest) = path.strip_prefix(home) {
            return if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
            };
        }
    }
    path.display().to_string()
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub fn age(now: i64, then: i64) -> String {
    let secs = (now - then).max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        86_400..=2_591_999 => format!("{}d ago", secs / 86_400),
        2_592_000..=31_535_999 => format!("{}mo ago", secs / 2_592_000),
        _ => format!("{}y ago", secs / 31_536_000),
    }
}

/// `1 file`, `2 files`.
pub fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// `2026-10-21 04:11 UTC`
pub fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `90s`, `30m`, `24h`, `7d`, `2w`.
pub fn parse_duration(text: &str) -> anyhow::Result<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .filter(|&i| i > 0)
        .ok_or_else(|| anyhow::anyhow!("{text:?} is not a duration; use e.g. 30m, 24h or 7d"))?;
    let (digits, unit) = text.split_at(split);
    let n: u64 = digits.parse()?;
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 604_800,
        _ => anyhow::bail!("unknown time unit {unit:?} in {text:?}; use s, m, h, d or w"),
    };
    n.checked_mul(factor)
        .ok_or_else(|| anyhow::anyhow!("{text:?} is too large"))
}

/// `45m`, `24h`, `7d`: the largest unit that divides evenly, else the largest that fits.
pub fn humanize_secs(secs: u64) -> String {
    for (unit, size) in [("d", 86_400), ("h", 3600), ("m", 60)] {
        if secs >= size && secs % size == 0 {
            return format!("{}{unit}", secs / size);
        }
    }
    for (unit, size) in [("d", 86_400), ("h", 3600), ("m", 60)] {
        if secs >= size {
            return format!("{}{unit}", secs / size);
        }
    }
    format!("{secs}s")
}

/// `in 3h` / `expired`
pub fn until(now: i64, then: i64) -> String {
    if then <= now {
        "expired".to_string()
    } else {
        format!("in {}", humanize_secs((then - now) as u64))
    }
}

// --- tables -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Dim,
    Bold,
    Yellow,
    Cyan,
}

#[derive(Debug, Clone)]
pub struct Cell {
    text: String,
    style: Style,
}

impl Cell {
    /// Everything shown in a table passes through [`sanitize`].
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        let text = text.into();
        Cell {
            text: sanitize(&text).into_owned(),
            style,
        }
    }
}

impl From<String> for Cell {
    fn from(text: String) -> Self {
        Cell::new(text, Style::Plain)
    }
}

impl From<&str> for Cell {
    fn from(text: &str) -> Self {
        Cell::new(text, Style::Plain)
    }
}

/// Borderless, column-aligned table: easy to read and to grep.
pub struct Table {
    headers: Vec<&'static str>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new(headers: &[&'static str]) -> Self {
        Table {
            headers: headers.to_vec(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: Vec<Cell>) {
        debug_assert_eq!(cells.len(), self.headers.len());
        self.rows.push(cells);
    }

    pub fn render(&self, out: &mut impl Write) -> io::Result<()> {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.width()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.text.width());
            }
        }
        let last = widths.len().saturating_sub(1);

        for (i, h) in self.headers.iter().enumerate() {
            write!(out, "{}", h.bold().dimmed())?;
            if i != last {
                write!(out, "{}", " ".repeat(widths[i] - h.width() + 2))?;
            }
        }
        writeln!(out)?;

        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                write_styled(out, &cell.text, cell.style)?;
                if i != last {
                    write!(out, "{}", " ".repeat(widths[i] - cell.text.width() + 2))?;
                }
            }
            writeln!(out)?;
        }
        Ok(())
    }
}

fn write_styled(out: &mut impl Write, text: &str, style: Style) -> io::Result<()> {
    match style {
        Style::Plain => write!(out, "{text}"),
        Style::Dim => write!(out, "{}", text.dimmed()),
        Style::Bold => write!(out, "{}", text.bold()),
        Style::Yellow => write!(out, "{}", text.yellow()),
        Style::Cyan => write!(out, "{}", text.cyan()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs() -> Vec<(String, String)> {
        vec![
            ("A".into(), "plain".into()),
            ("B".into(), "it's a \"secret\"".into()),
            ("C".into(), String::new()),
        ]
    }

    #[test]
    fn masks_by_default_and_reveals_on_request() {
        assert_eq!(masked("x"), MASK);
        assert_eq!(masked(""), "(empty)");
        let hidden = render_pairs(Format::Dotenv, &pairs(), false);
        assert!(!hidden.contains("plain") && !hidden.contains("secret"));
        let shown = render_pairs(Format::Dotenv, &pairs(), true);
        assert!(shown.contains("A=plain\n"));
        assert_eq!(DotEnv::parse(&shown).get("B"), Some("it's a \"secret\""));
    }

    #[test]
    fn shell_quoting_survives_single_quotes() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(ps_quote("it's"), "'it''s'");
        let out = render_pairs(Format::Shell, &pairs(), true);
        assert!(out.contains("export A='plain'\n"));
        assert!(out.contains(r#"export B='it'\''s a "secret"'"#));
        let ps = render_pairs(Format::Powershell, &pairs(), true);
        assert!(ps.contains("$env:A = 'plain'\n"));
    }

    #[test]
    fn json_keeps_order_and_masks() {
        let json = render_pairs(Format::Json, &pairs(), false);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["A", "B", "C"]);
        assert_eq!(v["A"], MASK);
        assert_eq!(v["C"], "(empty)");
        let revealed: serde_json::Value =
            serde_json::from_str(&render_pairs(Format::Json, &pairs(), true)).unwrap();
        assert_eq!(revealed["A"], "plain");
    }

    #[test]
    fn sanitize_defuses_terminal_control_sequences() {
        assert_eq!(sanitize("plain text é ü 日本"), "plain text é ü 日本");
        assert!(matches!(sanitize("plain"), Cow::Borrowed(_)));
        assert_eq!(sanitize("a\x1b[31mred\x1b[0m"), "a\\u{1b}[31mred\\u{1b}[0m");
        assert_eq!(
            sanitize("title\x1b]0;pwned\x07"),
            "title\\u{1b}]0;pwned\\u{7}"
        );
        assert_eq!(sanitize("a\r\nb\tc"), "a\\r\\nb\\tc");
        assert_eq!(sanitize("del\x7f c1\u{9b}"), "del\\u{7f} c1\\u{9b}");
        // right-to-left override would reorder what is displayed
        assert_eq!(sanitize("evil\u{202e}gnp.exe"), "evil\\u{202e}gnp.exe");
        for s in ["\x1b[2J", "\x07", "\u{202e}x", "\u{2066}x"] {
            assert!(!sanitize(s).chars().any(is_unsafe_char), "{s:?}");
        }
    }

    #[test]
    fn layout_keeping_variant_only_touches_dangerous_characters() {
        assert_eq!(
            sanitize_keeping_layout("-----BEGIN-----\nabc\n\tx"),
            "-----BEGIN-----\nabc\n\tx"
        );
        assert_eq!(
            sanitize_keeping_layout("a\x1b[2J\nb\r"),
            "a\\u{1b}[2J\nb\\r"
        );
    }

    #[test]
    fn table_cells_are_sanitized() {
        let mut t = Table::new(&["K"]);
        t.row(vec!["bad\x1b]52;c;ZXZpbA==\x07".into()]);
        let mut buf = Vec::new();
        t.render(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        // The header carries the table's own styling (rendered raw here, since there is
        // no anstream in front of this buffer); the data row must be free of control bytes.
        let row = out.lines().nth(1).unwrap();
        assert!(!row.contains('\x1b') && !row.contains('\x07'), "{row:?}");
        assert!(row.contains("\\u{1b}]52"), "{row:?}");
    }

    #[test]
    fn shell_formats_never_emit_unsafe_names() {
        let pairs = vec![
            ("OK".to_string(), "fine".to_string()),
            ("X; echo pwned".to_string(), "v".to_string()),
            ("a-b".to_string(), "v".to_string()),
            ("A\nrm -rf ~".to_string(), "v".to_string()),
            ("$(id)".to_string(), "v".to_string()),
            ("1BAD".to_string(), "v".to_string()),
            ("ok_2".to_string(), "it's".to_string()),
        ];
        for format in [Format::Shell, Format::Powershell] {
            let out = render_pairs(format, &pairs, true);
            assert!(
                !out.contains("pwned") && !out.contains("rm -rf") && !out.contains("$(id)"),
                "{out}"
            );
            assert!(!out.contains("a-b") && !out.contains("1BAD"), "{out}");
            assert_eq!(out.lines().count(), 2, "{out}");
            assert_eq!(skipped_names(format, &pairs), 5);
        }
        assert_eq!(
            render_pairs(Format::Shell, &pairs, true),
            "export OK='fine'\nexport ok_2='it'\\''s'\n"
        );
        // dotenv keeps dots and dashes (valid there) but drops anything else
        assert_eq!(skipped_names(Format::Dotenv, &pairs), 4);
        assert_eq!(skipped_names(Format::Json, &pairs), 0);
        let json: serde_json::Value =
            serde_json::from_str(&render_pairs(Format::Json, &pairs, true)).unwrap();
        assert_eq!(json["X; echo pwned"], "v"); // JSON escapes it; nothing to inject into
    }

    #[test]
    fn ages() {
        assert_eq!(age(1000, 990), "just now");
        assert_eq!(age(1000, 1000 - 120), "2m ago");
        assert_eq!(age(100_000, 100_000 - 7200), "2h ago");
        assert_eq!(age(1_000_000, 1_000_000 - 3 * 86_400), "3d ago");
        assert_eq!(age(100_000_000, 100_000_000 - 5_184_000), "2mo ago");
        assert_eq!(age(100_000_000, 100_000_000 - 70_000_000), "2y ago");
        assert_eq!(age(10, 50), "just now");
    }

    #[test]
    fn plurals() {
        assert_eq!(plural(0, "file"), "0 files");
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(2, "example file"), "2 example files");
    }

    #[test]
    fn utc_formatting() {
        assert_eq!(format_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(format_utc(4_102_444_799), "2099-12-31 23:59 UTC");
        assert_eq!(format_utc(-1), "1969-12-31 23:59 UTC");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90s").unwrap(), 90);
        assert_eq!(parse_duration("30m").unwrap(), 1800);
        assert_eq!(parse_duration("24h").unwrap(), 86_400);
        assert_eq!(parse_duration("7d").unwrap(), 604_800);
        assert_eq!(parse_duration("2w").unwrap(), 1_209_600);
        for bad in [
            "",
            "h",
            "10",
            "10x",
            "-5m",
            "1.5h",
            "5 m",
            "99999999999999999999d",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
        assert_eq!(humanize_secs(86_400), "1d");
        assert_eq!(humanize_secs(7200), "2h");
        // exact beats rounded: 90 minutes, not "1h"
        assert_eq!(humanize_secs(5400), "90m");
        // not evenly divisible by any unit: round down to the largest that fits
        assert_eq!(humanize_secs(5430), "1h");
        assert_eq!(humanize_secs(45), "45s");
        assert_eq!(until(100, 100 + 3600), "in 1h");
        assert_eq!(until(100, 50), "expired");
    }

    #[test]
    fn table_aligns_columns() {
        let mut t = Table::new(&["KEY", "VALUE"]);
        t.row(vec!["A".into(), "1".into()]);
        t.row(vec!["LONGER".into(), "2".into()]);
        let mut buf = Vec::new();
        t.render(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        let col = |l: &str| l.find(['1', '2']).unwrap();
        // both value cells start in the same column
        assert_eq!(col(lines[1]), col(lines[2]));
    }
}
