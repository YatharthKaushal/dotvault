//! Finds the secret files that are not env files: SSH and TLS private keys,
//! keystores, service-account and other secret JSON, and credential dotfiles.
//!
//! Two pure stages keep a scan over millions of files cheap. [`candidate`] looks
//! only at a file's name and size (no allocation) and says how much of it to read;
//! [`confirm`] decides from those bytes. Nothing here touches the disk, and what a
//! caller may keep from a file is its kind, a short detail line and, for JSON, the
//! top-level key names. Never the contents.

use std::collections::BTreeSet;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest JSON or text file that is read whole. The same size as the biggest
/// share, so anything bigger could not be shared anyway.
pub const MAX_TEXT_BYTES: u64 = 64 * 1024;

/// How much of a key or keystore file is read: enough for a certificate chain
/// followed by its key, and for any magic number.
pub const HEAD_BYTES: usize = 16 * 1024;

/// What a file is. `Env` is here so one value can describe every indexed file.
/// Serialized as the same kebab-case name `as_str` gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Env,
    SshKey,
    PrivateKey,
    Keystore,
    ServiceAccount,
    AuthJson,
    SecretJson,
    AwsCredentials,
    NpmToken,
    Netrc,
    PypiToken,
    GitCredentials,
    Kubeconfig,
    /// A file the user marked by hand.
    Marked,
}

impl Kind {
    /// Every kind except `Env`, in the order they are listed in help.
    pub const SECRET: [Kind; 13] = [
        Kind::SshKey,
        Kind::PrivateKey,
        Kind::Keystore,
        Kind::ServiceAccount,
        Kind::AuthJson,
        Kind::SecretJson,
        Kind::AwsCredentials,
        Kind::NpmToken,
        Kind::Netrc,
        Kind::PypiToken,
        Kind::GitCredentials,
        Kind::Kubeconfig,
        Kind::Marked,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Env => "env",
            Kind::SshKey => "ssh-key",
            Kind::PrivateKey => "private-key",
            Kind::Keystore => "keystore",
            Kind::ServiceAccount => "service-account",
            Kind::AuthJson => "auth-json",
            Kind::SecretJson => "secret-json",
            Kind::AwsCredentials => "aws-credentials",
            Kind::NpmToken => "npm-token",
            Kind::Netrc => "netrc",
            Kind::PypiToken => "pypi-token",
            Kind::GitCredentials => "git-credentials",
            Kind::Kubeconfig => "kubeconfig",
            Kind::Marked => "marked",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        std::iter::once(Kind::Env)
            .chain(Kind::SECRET)
            .find(|k| k.as_str().eq_ignore_ascii_case(text))
    }

    pub fn is_secret(self) -> bool {
        self != Kind::Env
    }

    /// Files whose whole value is a credential someone could act as you with. Sharing
    /// one asks for confirmation first.
    pub fn is_key_material(self) -> bool {
        matches!(
            self,
            Kind::SshKey | Kind::PrivateKey | Kind::Keystore | Kind::ServiceAccount
        )
    }

    /// Kinds whose file is JSON, so its top-level key names are known.
    pub fn is_json(self) -> bool {
        matches!(
            self,
            Kind::ServiceAccount | Kind::AuthJson | Kind::SecretJson
        )
    }
}

/// What a file turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub kind: Kind,
    /// Top-level key names of a JSON file; empty for everything else.
    pub keys: Vec<String>,
    /// A short note such as `ed25519, passphrase` or `jks`. Never a secret.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hint {
    Ssh,
    Pem,
    Keystore,
    KeystoreProps,
    Json,
    GoogleServices,
    Aws,
    Npmrc,
    Netrc,
    Pypirc,
    GitCredentials,
    Kube,
}

/// A file worth reading. Made by [`candidate`], consumed by [`confirm`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    hint: Hint,
    /// How many bytes to read (fewer if the file is shorter).
    pub read: usize,
}

fn extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot + 1 < name.len() => &name[dot + 1..],
        _ => "",
    }
}

fn is_any(text: &str, options: &[&str]) -> bool {
    options.iter().any(|o| text.eq_ignore_ascii_case(o))
}

fn ends_with_ci(text: &str, suffix: &str) -> bool {
    let (t, s) = (text.as_bytes(), suffix.as_bytes());
    t.len() >= s.len() && t[t.len() - s.len()..].eq_ignore_ascii_case(s)
}

fn starts_with_ci(text: &str, prefix: &str) -> bool {
    let (t, p) = (text.as_bytes(), prefix.as_bytes());
    t.len() >= p.len() && t[..p.len()].eq_ignore_ascii_case(p)
}

/// `bytes` lowercased on the stack, or `None` if it does not fit in `N` bytes. A
/// scan asks this about nearly every file, so it must not allocate.
fn lower<const N: usize>(bytes: &[u8]) -> Option<([u8; N], usize)> {
    if bytes.len() > N {
        return None;
    }
    let mut buf = [0u8; N];
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[..bytes.len()].make_ascii_lowercase();
    Some((buf, bytes.len()))
}

/// JSON files that are configuration, never credentials. Skipped before reading.
fn is_plain_config_json(name: &str) -> bool {
    starts_with_ci(name, "tsconfig")
        || starts_with_ci(name, "jsconfig")
        || ends_with_ci(name, ".schema.json")
        || is_any(
            name,
            &[
                "package.json",
                "package-lock.json",
                "composer.json",
                "manifest.json",
                "tslint.json",
                ".eslintrc.json",
                ".prettierrc.json",
                "renovate.json",
                "vercel.json",
                "firebase.json",
                "angular.json",
                "nest-cli.json",
                "turbo.json",
                "lerna.json",
                "nx.json",
                "project.json",
                "launch.json",
                "settings.json",
                "tasks.json",
                "extensions.json",
            ],
        )
}

fn hint_for<'a>(name: &str, parent: impl Fn() -> &'a str, size: u64) -> Option<Hint> {
    let ext = extension(name);
    // The extension decides most files at once: one lowercase copy, one match.
    if let Some((buf, n)) = lower::<8>(ext.as_bytes()) {
        match &buf[..n] {
            b"json" => {
                if size > MAX_TEXT_BYTES {
                    return None;
                }
                if name.eq_ignore_ascii_case("google-services.json") {
                    return Some(Hint::GoogleServices);
                }
                return (!is_plain_config_json(name)).then_some(Hint::Json);
            }
            b"pem" | b"key" | b"p8" | b"pk8" => {
                return (size <= MAX_TEXT_BYTES).then_some(Hint::Pem);
            }
            b"jks" | b"keystore" | b"p12" | b"pfx" | b"bks" | b"jceks" | b"pkcs12" => {
                return Some(Hint::Keystore);
            }
            b"ppk" => return (size <= MAX_TEXT_BYTES).then_some(Hint::Ssh),
            _ => {}
        }
    }
    if size > MAX_TEXT_BYTES {
        return None;
    }
    // Exact names, all short and all starting with one of four letters.
    if matches!(
        name.as_bytes().first().map(u8::to_ascii_lowercase),
        Some(b'.' | b'_' | b'c' | b'k')
    ) {
        if let Some((buf, n)) = lower::<20>(name.as_bytes()) {
            match &buf[..n] {
                b"key.properties" | b"keystore.properties" => return Some(Hint::KeystoreProps),
                b".npmrc" => return Some(Hint::Npmrc),
                b".netrc" | b"_netrc" => return Some(Hint::Netrc),
                b".pypirc" => return Some(Hint::Pypirc),
                b".git-credentials" => return Some(Hint::GitCredentials),
                b"kubeconfig" => return Some(Hint::Kube),
                b"credentials" if parent().eq_ignore_ascii_case(".aws") => {
                    return Some(Hint::Aws);
                }
                _ => {}
            }
        }
        if name == "config" && parent().eq_ignore_ascii_case(".kube") {
            return Some(Hint::Kube);
        }
    }
    if ends_with_ci(name, ".kubeconfig") {
        return Some(Hint::Kube);
    }
    if (starts_with_ci(name, "id_") && !ext.eq_ignore_ascii_case("pub"))
        || ["_rsa", "_dsa", "_ecdsa", "_ed25519"]
            .iter()
            .any(|s| ends_with_ci(name, s))
    {
        return Some(Hint::Ssh);
    }
    // A file with no extension in `.ssh` is a key (or a name from the list below).
    // Files that have one are left out so that most files never ask for the folder.
    if ext.is_empty() && parent().eq_ignore_ascii_case(".ssh") {
        let skip = starts_with_ci(name, "known_hosts")
            || starts_with_ci(name, "authorized_keys")
            || name.eq_ignore_ascii_case("config");
        return (!skip).then_some(Hint::Ssh);
    }
    None
}

/// Whether a file of this name and size is worth reading to find out. Cheap and
/// allocation-free: it runs for every file a scan meets. `parent` gives the name of
/// the folder holding the file and is only called when a rule needs it.
pub fn candidate_with<'a>(
    name: &str,
    parent: impl Fn() -> &'a str,
    size: u64,
) -> Option<Candidate> {
    if size == 0 {
        return None;
    }
    let hint = hint_for(name, parent, size)?;
    let read = match hint {
        Hint::Keystore => 16,
        Hint::Json | Hint::GoogleServices => MAX_TEXT_BYTES as usize,
        Hint::Ssh | Hint::Pem => HEAD_BYTES,
        _ => MAX_TEXT_BYTES as usize,
    };
    Some(Candidate { hint, read })
}

/// [`candidate_with`] when the folder name is already at hand.
pub fn candidate(name: &str, parent: &str, size: u64) -> Option<Candidate> {
    candidate_with(name, || parent, size)
}

/// Decides from the bytes read. `name` is only used to judge weak JSON keys.
pub fn confirm(candidate: &Candidate, name: &str, bytes: &[u8]) -> Option<Detected> {
    match candidate.hint {
        Hint::Ssh | Hint::Pem => private_key(candidate.hint, bytes),
        Hint::Keystore => keystore(bytes),
        Hint::KeystoreProps => keystore_properties(bytes),
        Hint::Json => json(name, bytes, false),
        Hint::GoogleServices => json(name, bytes, true),
        Hint::Aws => aws_credentials(bytes),
        Hint::Npmrc => npmrc(bytes),
        Hint::Netrc => netrc(bytes),
        Hint::Pypirc => pypirc(bytes),
        Hint::GitCredentials => git_credentials(bytes),
        Hint::Kube => kubeconfig(bytes),
    }
}

fn found(kind: Kind, detail: impl Into<String>) -> Option<Detected> {
    Some(Detected {
        kind,
        keys: Vec::new(),
        detail: detail.into(),
    })
}

fn text_of(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes)
}

/// A value that is plausibly real, not a placeholder such as `<token>` or `${TOKEN}`.
fn looks_real(value: &str) -> bool {
    let v = value.trim().trim_matches(|c| c == '"' || c == '\'');
    v.len() >= 8
        && !v.contains("${")
        && !v.contains("{{")
        && !v.starts_with('<')
        && !v.starts_with("YOUR")
        && !v.starts_with("your")
        && !v.eq_ignore_ascii_case("changeme")
        && !v.chars().all(|c| matches!(c, '*' | 'x' | 'X' | '.'))
}

// --- private keys ------------------------------------------------------------

struct PemBlock {
    label: String,
    body: String,
    encrypted_header: bool,
}

fn first_private_key_block(text: &str) -> Option<PemBlock> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("-----BEGIN ") else {
            continue;
        };
        let Some(label) = rest.strip_suffix("-----") else {
            continue;
        };
        if !label.ends_with("PRIVATE KEY") {
            continue;
        }
        let (mut body, mut encrypted_header) = (String::new(), false);
        for line in lines.by_ref() {
            let line = line.trim();
            if line.starts_with("-----END ") {
                break;
            }
            if line.starts_with("Proc-Type:") && line.contains("ENCRYPTED") {
                encrypted_header = true;
            }
            if !line.contains(':') {
                body.push_str(line);
            }
        }
        return Some(PemBlock {
            label: label.to_string(),
            body,
            encrypted_header,
        });
    }
    None
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.bytes(4)?.try_into().ok()?))
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.bytes(n)
    }
}

/// The algorithm and whether a passphrase protects it, for the OpenSSH format.
fn openssh_info(body: &str) -> Option<(String, bool)> {
    let raw = STANDARD.decode(body.as_bytes()).ok()?;
    let mut r = Reader(raw.strip_prefix(b"openssh-key-v1\0")?);
    let cipher = r.string()?;
    let _kdf = r.string()?;
    let _kdf_options = r.string()?;
    let _count = r.u32()?;
    let mut public = Reader(r.string()?);
    let key_type = String::from_utf8_lossy(public.string()?).into_owned();
    let algo = match key_type.as_str() {
        "ssh-ed25519" => "ed25519".to_string(),
        "ssh-rsa" => "rsa".to_string(),
        "ssh-dss" => "dsa".to_string(),
        t if t.starts_with("ecdsa-") => "ecdsa".to_string(),
        t if t.starts_with("sk-") => "security-key".to_string(),
        t => t.to_string(),
    };
    Some((algo, cipher != b"none"))
}

fn private_key(hint: Hint, bytes: &[u8]) -> Option<Detected> {
    let text = text_of(bytes);
    if text.starts_with("PuTTY-User-Key-File-") {
        let mut algo = String::new();
        let mut encrypted = false;
        for line in text.lines().take(4) {
            if let Some((name, value)) = line.split_once(':') {
                if name.starts_with("PuTTY-User-Key-File-") {
                    algo = value.trim().trim_start_matches("ssh-").to_string();
                } else if name == "Encryption" {
                    encrypted = value.trim() != "none";
                }
            }
        }
        return found(Kind::SshKey, describe(&algo, encrypted));
    }
    let block = first_private_key_block(&text)?;
    let (algo, encrypted) = match block.label.as_str() {
        "OPENSSH PRIVATE KEY" => openssh_info(&block.body).unwrap_or_default(),
        "RSA PRIVATE KEY" => ("rsa".to_string(), block.encrypted_header),
        "EC PRIVATE KEY" => ("ecdsa".to_string(), block.encrypted_header),
        "DSA PRIVATE KEY" => ("dsa".to_string(), block.encrypted_header),
        "ENCRYPTED PRIVATE KEY" => ("pkcs8".to_string(), true),
        _ => ("pkcs8".to_string(), block.encrypted_header),
    };
    let kind = if hint == Hint::Ssh || block.label == "OPENSSH PRIVATE KEY" {
        Kind::SshKey
    } else {
        Kind::PrivateKey
    };
    found(kind, describe(&algo, encrypted))
}

fn describe(algo: &str, encrypted: bool) -> String {
    match (algo.is_empty(), encrypted) {
        (true, false) => String::new(),
        (true, true) => "passphrase".to_string(),
        (false, false) => algo.to_string(),
        (false, true) => format!("{algo}, passphrase"),
    }
}

// --- keystores -----------------------------------------------------------------

/// A PKCS#12 file is a DER sequence whose first element is the integer 3.
fn is_pkcs12(b: &[u8]) -> bool {
    if b.len() < 8 || b[0] != 0x30 {
        return false;
    }
    let skip = match b[1] {
        l if l < 0x80 => 0,
        0x80 => 0,
        l @ 0x81..=0x84 => (l - 0x80) as usize,
        _ => return false,
    };
    let at = 2 + skip;
    b.len() >= at + 3 && b[at..at + 3] == [0x02, 0x01, 0x03]
}

fn keystore(bytes: &[u8]) -> Option<Detected> {
    let kind = if bytes.starts_with(&[0xFE, 0xED, 0xFE, 0xED]) {
        "jks"
    } else if bytes.starts_with(&[0xCE, 0xCE, 0xCE, 0xCE]) {
        "jceks"
    } else if is_pkcs12(bytes) {
        "pkcs12"
    } else if bytes.len() >= 4 && bytes[..3] == [0, 0, 0] && matches!(bytes[3], 1 | 2) {
        "bks"
    } else {
        return None;
    };
    found(Kind::Keystore, kind)
}

fn keystore_properties(bytes: &[u8]) -> Option<Detected> {
    let text = text_of(bytes);
    let mut names = BTreeSet::new();
    let mut has_password = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            let name = name.trim();
            names.insert(name.to_string());
            if matches!(name, "storePassword" | "keyPassword") && looks_real(value) {
                has_password = true;
            }
        }
    }
    has_password.then(|| Detected {
        kind: Kind::Keystore,
        keys: names.into_iter().collect(),
        detail: "signing config".to_string(),
    })
}

// --- JSON -----------------------------------------------------------------------

const STRONG_KEYS: [&str; 8] = [
    "private_key",
    "client_secret",
    "refresh_token",
    "secret_access_key",
    "secret_key",
    "access_token",
    "auth_token",
    "api_secret",
];

const WEAK_KEYS: [&str; 6] = ["password", "passwd", "token", "apikey", "api_key", "secret"];

fn non_empty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.trim().is_empty())
}

fn name_says_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "credential",
        "secret",
        "token",
        "auth",
        "password",
        "private",
        "apikey",
        "serviceaccount",
        "service-account",
        "service_account",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// A strong (or, when the file name says so, weak) secret-looking key within three
/// levels. Returns that key's name.
fn find_secret_key(
    value: &Value,
    weak_too: bool,
    depth: usize,
    seen: &mut usize,
) -> Option<String> {
    *seen += 1;
    if depth > 3 || *seen > 2000 {
        return None;
    }
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                let lower = key.to_ascii_lowercase();
                let strong = STRONG_KEYS.contains(&lower.as_str());
                let weak = weak_too && WEAK_KEYS.contains(&lower.as_str());
                if (strong || weak) && v.as_str().is_some_and(looks_real) {
                    return Some(key.clone());
                }
            }
            map.values()
                .find_map(|v| find_secret_key(v, weak_too, depth + 1, seen))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|v| find_secret_key(v, weak_too, depth + 1, seen)),
        _ => None,
    }
}

fn json(name: &str, bytes: &[u8], google_services: bool) -> Option<Detected> {
    let value: Value =
        serde_json::from_slice(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes))
            .ok()?;
    let object = value.as_object()?;
    let mut keys: Vec<String> = object.keys().take(100).cloned().collect();
    keys.sort();
    let with = |kind: Kind, detail: &str| {
        Some(Detected {
            kind,
            keys: keys.clone(),
            detail: detail.to_string(),
        })
    };

    if google_services {
        if object.contains_key("project_info") {
            return with(Kind::ServiceAccount, "google-services");
        }
        return None;
    }

    let kind_type = non_empty(&value, "type");
    if kind_type == Some("service_account") && non_empty(&value, "private_key").is_some() {
        return with(Kind::ServiceAccount, "");
    }
    match kind_type {
        Some("authorized_user")
            if non_empty(&value, "refresh_token").is_some()
                || non_empty(&value, "client_secret").is_some() =>
        {
            return with(Kind::AuthJson, "authorized_user");
        }
        Some("external_account") => return with(Kind::AuthJson, "external_account"),
        _ => {}
    }
    for oauth in ["installed", "web"] {
        if value
            .get(oauth)
            .is_some_and(|c| non_empty(c, "client_secret").is_some())
        {
            return with(Kind::AuthJson, "oauth-client");
        }
    }
    if non_empty(&value, "refresh_token").is_some() {
        return with(Kind::AuthJson, "oauth-token");
    }
    if let Some(auths) = value.get("auths").and_then(Value::as_object) {
        let has_auth = auths.values().any(|a| {
            ["auth", "identitytoken", "password"]
                .iter()
                .any(|k| non_empty(a, k).is_some())
        });
        if has_auth {
            return with(Kind::AuthJson, "docker");
        }
    }
    for composer in [
        "github-oauth",
        "gitlab-oauth",
        "gitlab-token",
        "bitbucket-oauth",
        "http-basic",
        "bearer",
    ] {
        if value
            .get(composer)
            .and_then(Value::as_object)
            .is_some_and(|m| !m.is_empty())
        {
            return with(Kind::AuthJson, "composer");
        }
    }
    let mut seen = 0;
    let hit = find_secret_key(&value, name_says_secret(name), 0, &mut seen)?;
    with(Kind::SecretJson, &format!("has {hit}"))
}

// --- credential dotfiles -----------------------------------------------------------

/// `name = value` or `name=value`, ignoring comment lines.
fn assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.starts_with('#') || line.starts_with(';') {
        return None;
    }
    let (name, value) = line.split_once('=')?;
    Some((name.trim(), value.trim()))
}

fn aws_credentials(bytes: &[u8]) -> Option<Detected> {
    let text = text_of(bytes);
    let (mut profile, mut with_secret) = (String::new(), Vec::new());
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            profile = name.trim().to_string();
        } else if let Some((name, value)) = assignment(line) {
            if name.eq_ignore_ascii_case("aws_secret_access_key") && looks_real(value) {
                with_secret.push(profile.clone());
            }
        }
    }
    if with_secret.is_empty() {
        return None;
    }
    let count = with_secret.len();
    Some(Detected {
        kind: Kind::AwsCredentials,
        keys: with_secret,
        detail: format!("{count} profile{}", if count == 1 { "" } else { "s" }),
    })
}

fn npmrc(bytes: &[u8]) -> Option<Detected> {
    let text = text_of(bytes);
    let hit = text.lines().find_map(|line| {
        let (name, value) = assignment(line)?;
        let ends = ["_authToken", "_auth", "_password"]
            .iter()
            .any(|suffix| name.ends_with(suffix));
        (ends && looks_real(value)).then_some(())
    });
    hit.and_then(|_| found(Kind::NpmToken, ""))
}

fn netrc(bytes: &[u8]) -> Option<Detected> {
    let text = text_of(bytes);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let has_password = tokens
        .windows(2)
        .any(|w| w[0] == "password" && !w[1].is_empty());
    let machines = tokens.iter().filter(|t| **t == "machine").count();
    has_password
        .then(|| format!("{machines} machine{}", if machines == 1 { "" } else { "s" }))
        .and_then(|detail| found(Kind::Netrc, detail))
}

fn pypirc(bytes: &[u8]) -> Option<Detected> {
    let text = text_of(bytes);
    let value = text.lines().find_map(|line| {
        let (name, value) = line
            .split_once('=')
            .or_else(|| line.split_once(':'))
            .map(|(n, v)| (n.trim(), v.trim()))?;
        (name == "password" && looks_real(value)).then_some(value)
    })?;
    found(
        Kind::PypiToken,
        if value.starts_with("pypi-") {
            "token"
        } else {
            "password"
        },
    )
}

fn git_credentials(bytes: &[u8]) -> Option<Detected> {
    let text = text_of(bytes);
    let count = text
        .lines()
        .filter(|line| {
            let Some((_, rest)) = line.trim().split_once("://") else {
                return false;
            };
            let Some((userinfo, _host)) = rest.split_once('@') else {
                return false;
            };
            userinfo
                .split_once(':')
                .is_some_and(|(user, pass)| !user.is_empty() && !pass.is_empty())
        })
        .count();
    if count == 0 {
        return None;
    }
    found(
        Kind::GitCredentials,
        format!("{count} login{}", if count == 1 { "" } else { "s" }),
    )
}

fn kubeconfig(bytes: &[u8]) -> Option<Detected> {
    let text = text_of(bytes);
    let mut found_kinds: BTreeSet<&str> = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches("- ");
        for (field, label) in [
            ("client-key-data:", "client key"),
            ("token:", "token"),
            ("password:", "password"),
        ] {
            if let Some(value) = line.strip_prefix(field) {
                if !value.trim().is_empty() {
                    found_kinds.insert(label);
                }
            }
        }
    }
    if found_kinds.is_empty() {
        return None;
    }
    found(
        Kind::Kubeconfig,
        found_kinds.into_iter().collect::<Vec<_>>().join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Fixtures are built from fragments at run time, so nothing in the repository
    // looks like real key material to a secret scanner.

    fn begin(label: &str) -> String {
        format!("{}BEGIN {label}{}", "-----", "-----")
    }

    fn end(label: &str) -> String {
        format!("{}END {label}{}", "-----", "-----")
    }

    fn pem(label: &str, body: &str) -> String {
        format!("{}\n{body}\n{}\n", begin(label), end(label))
    }

    fn string(out: &mut Vec<u8>, s: &[u8]) {
        out.extend((s.len() as u32).to_be_bytes());
        out.extend(s);
    }

    /// An OpenSSH key container with the given cipher and public key type.
    fn openssh(cipher: &str, key_type: &str) -> String {
        let mut b = b"openssh-key-v1\0".to_vec();
        string(&mut b, cipher.as_bytes());
        string(&mut b, if cipher == "none" { b"none" } else { b"bcrypt" });
        string(&mut b, b"");
        b.extend(1u32.to_be_bytes());
        let mut public = Vec::new();
        string(&mut public, key_type.as_bytes());
        string(&mut public, &[0u8; 32]);
        string(&mut b, &public);
        string(&mut b, &[0u8; 16]);
        let wrapped: Vec<String> = STANDARD
            .encode(&b)
            .as_bytes()
            .chunks(70)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        pem("OPENSSH PRIVATE KEY", &wrapped.join("\n"))
    }

    fn detect(name: &str, parent: &str, content: &[u8]) -> Option<Detected> {
        let c = candidate(name, parent, content.len() as u64)?;
        confirm(&c, name, content)
    }

    fn kind_of(name: &str, parent: &str, content: &[u8]) -> Option<Kind> {
        detect(name, parent, content).map(|d| d.kind)
    }

    fn sa() -> Vec<u8> {
        json!({
            "type": "service_account",
            "project_id": "demo",
            "private_key_id": "abc123",
            "private_key": pem("PRIVATE KEY", "QUJDREVGR0g="),
            "client_email": "bot@demo.example",
        })
        .to_string()
        .into_bytes()
    }

    // --- names -----------------------------------------------------------------

    #[test]
    fn kinds_round_trip_through_their_names() {
        for kind in std::iter::once(Kind::Env).chain(Kind::SECRET) {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind), "{kind:?}");
        }
        assert_eq!(Kind::parse("SSH-KEY"), Some(Kind::SshKey));
        assert_eq!(Kind::parse("nope"), None);
        assert!(!Kind::Env.is_secret() && Kind::Marked.is_secret());
        assert!(Kind::SshKey.is_key_material() && !Kind::NpmToken.is_key_material());
    }

    #[test]
    fn the_serialized_name_is_the_display_name() {
        for kind in std::iter::once(Kind::Env).chain(Kind::SECRET) {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()), "{kind:?}");
            assert_eq!(serde_json::from_str::<Kind>(&json).unwrap(), kind);
        }
    }

    #[test]
    fn only_plausible_names_are_worth_reading() {
        for (name, parent) in [
            ("id_ed25519", ".ssh"),
            ("id_rsa", "anywhere"),
            ("deploy_ed25519", "keys"),
            ("work", ".ssh"),
            ("server.pem", "certs"),
            ("tls.key", "certs"),
            ("AuthKey.p8", "apple"),
            ("upload.keystore", "android"),
            ("release.JKS", "x"),
            ("cert.p12", "x"),
            ("putty.ppk", "x"),
            ("key.properties", "android"),
            ("my-project-1a2b3c.json", "downloads"),
            ("google-services.json", "app"),
            (".npmrc", "home"),
            (".netrc", "home"),
            ("_netrc", "home"),
            (".pypirc", "home"),
            (".git-credentials", "home"),
            ("credentials", ".aws"),
            ("config", ".kube"),
            ("prod.kubeconfig", "x"),
        ] {
            assert!(candidate(name, parent, 100).is_some(), "{name} in {parent}");
        }
        for (name, parent) in [
            ("README.md", "x"),
            ("main.rs", "src"),
            ("id_ed25519.pub", ".ssh"),
            ("known_hosts", ".ssh"),
            ("known_hosts.old", ".ssh"),
            ("authorized_keys", ".ssh"),
            ("config", ".ssh"),
            ("package.json", "app"),
            ("tsconfig.build.json", "app"),
            ("Cargo.lock", "app"),
            ("credentials", "elsewhere"),
            ("config", "elsewhere"),
            ("notes.txt", ".ssh/../x"),
        ] {
            assert!(candidate(name, parent, 100).is_none(), "{name} in {parent}");
        }
    }

    #[test]
    fn only_files_without_an_extension_are_keys_just_for_living_in_ssh() {
        // a key is usually called `work` or `github`, not `work.bak`; mark odd ones by hand
        assert!(candidate("github", ".ssh", 100).is_some());
        assert!(candidate("deploy_key", ".ssh", 100).is_some());
        assert!(candidate("mykey.bak", ".ssh", 100).is_none());
        assert!(candidate("notes.txt", ".ssh", 100).is_none());
        // the same names elsewhere are not keys
        assert!(candidate("github", "projects", 100).is_none());
        // the parent is only looked up when a rule needs it
        let looked_up = std::cell::Cell::new(0);
        let parent = || {
            looked_up.set(looked_up.get() + 1);
            "src"
        };
        for name in [
            "main.rs",
            "a.txt",
            "image.png",
            "Cargo.toml",
            "readme.md",
            "x.json",
        ] {
            let _ = candidate_with(name, parent, 100);
        }
        assert_eq!(
            looked_up.get(),
            0,
            "ordinary files never ask for their folder"
        );
        let _ = candidate_with("LICENSE", parent, 100);
        assert_eq!(looked_up.get(), 1, "a name with no extension does");
    }

    #[test]
    fn empty_and_oversize_files_are_not_candidates() {
        assert!(candidate("id_rsa", ".ssh", 0).is_none());
        assert!(candidate("big.json", "x", MAX_TEXT_BYTES + 1).is_none());
        assert!(candidate("big.json", "x", MAX_TEXT_BYTES).is_some());
        assert!(
            candidate("movie.key", "x", MAX_TEXT_BYTES + 1).is_none(),
            "a Keynote file"
        );
        // a keystore may be large; only its first bytes are read
        assert_eq!(candidate("big.jks", "x", 5_000_000).unwrap().read, 16);
        assert_eq!(
            candidate("a.json", "x", 10).unwrap().read,
            MAX_TEXT_BYTES as usize
        );
    }

    #[test]
    fn names_with_odd_characters_never_panic() {
        for name in [
            "é.json",
            "日本.pem",
            "id_é",
            ".",
            "..",
            "a.",
            ".json",
            "x.JsOn",
        ] {
            let _ = candidate(name, "ß", 10);
        }
    }

    // --- private keys ------------------------------------------------------------

    #[test]
    fn an_openssh_key_reports_its_algorithm_and_whether_it_has_a_passphrase() {
        let open = detect(
            "id_ed25519",
            ".ssh",
            openssh("none", "ssh-ed25519").as_bytes(),
        )
        .unwrap();
        assert_eq!((open.kind, open.detail.as_str()), (Kind::SshKey, "ed25519"));
        let locked = detect(
            "id_rsa",
            ".ssh",
            openssh("aes256-ctr", "ssh-rsa").as_bytes(),
        )
        .unwrap();
        assert_eq!(locked.detail, "rsa, passphrase");
        assert_eq!(
            detect(
                "k",
                ".ssh",
                openssh("none", "ecdsa-sha2-nistp256").as_bytes()
            )
            .unwrap()
            .detail,
            "ecdsa"
        );
    }

    #[test]
    fn older_pem_ssh_keys_and_encrypted_ones_are_recognised() {
        let rsa = pem("RSA PRIVATE KEY", "QUJD");
        assert_eq!(
            detect("id_rsa", ".ssh", rsa.as_bytes()).unwrap().detail,
            "rsa"
        );
        let encrypted = format!(
            "{}\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nQUJD\n{}\n",
            begin("RSA PRIVATE KEY"),
            end("RSA PRIVATE KEY")
        );
        assert_eq!(
            detect("id_rsa", ".ssh", encrypted.as_bytes())
                .unwrap()
                .detail,
            "rsa, passphrase"
        );
        let pkcs8 = pem("ENCRYPTED PRIVATE KEY", "QUJD");
        assert_eq!(
            detect("id_x", ".ssh", pkcs8.as_bytes()).unwrap().detail,
            "pkcs8, passphrase"
        );
    }

    #[test]
    fn a_putty_key_is_an_ssh_key() {
        let ppk = "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\nComment: x\n";
        let d = detect("a.ppk", "x", ppk.as_bytes()).unwrap();
        assert_eq!((d.kind, d.detail.as_str()), (Kind::SshKey, "ed25519"));
        let locked = "PuTTY-User-Key-File-3: ssh-rsa\nEncryption: aes256-cbc\n";
        assert_eq!(
            detect("a.ppk", "x", locked.as_bytes()).unwrap().detail,
            "rsa, passphrase"
        );
    }

    #[test]
    fn a_tls_private_key_is_a_private_key_but_a_certificate_is_nothing() {
        let key = pem("PRIVATE KEY", "QUJD");
        assert_eq!(
            kind_of("server.key", "tls", key.as_bytes()),
            Some(Kind::PrivateKey)
        );
        let cert = pem("CERTIFICATE", "QUJD");
        assert_eq!(kind_of("server.pem", "tls", cert.as_bytes()), None);
        // a chain with the key last is found
        let chain = format!("{cert}{cert}{key}");
        assert_eq!(
            kind_of("fullchain.pem", "tls", chain.as_bytes()),
            Some(Kind::PrivateKey)
        );
        // an OpenSSH key in a .pem is still an SSH key
        assert_eq!(
            kind_of("deploy.pem", "x", openssh("none", "ssh-ed25519").as_bytes()),
            Some(Kind::SshKey)
        );
    }

    #[test]
    fn a_file_named_like_a_key_without_a_key_in_it_is_nothing() {
        assert_eq!(kind_of("id_rsa", ".ssh", b"just some notes"), None);
        assert_eq!(kind_of("id_rsa", ".ssh", &[0u8; 64]), None);
        let public = "ssh-ed25519 AAAAC3Nza user@host\n";
        assert_eq!(kind_of("id_ed25519", ".ssh", public.as_bytes()), None);
        // a header with no key type inside still counts, without an algorithm
        let odd = pem("OPENSSH PRIVATE KEY", "not base64 !!");
        assert_eq!(detect("k", ".ssh", odd.as_bytes()).unwrap().detail, "");
    }

    // --- keystores ---------------------------------------------------------------

    #[test]
    fn keystores_are_recognised_by_their_magic_bytes_not_their_extension() {
        let jks = [0xFE, 0xED, 0xFE, 0xED, 0, 0, 0, 2];
        assert_eq!(detect("a.jks", "x", &jks).unwrap().detail, "jks");
        let jceks = [0xCE, 0xCE, 0xCE, 0xCE, 0, 0, 0, 2];
        assert_eq!(detect("a.keystore", "x", &jceks).unwrap().detail, "jceks");
        let p12 = [0x30, 0x82, 0x0B, 0x7C, 0x02, 0x01, 0x03, 0x30, 0x82];
        assert_eq!(detect("a.p12", "x", &p12).unwrap().detail, "pkcs12");
        let short = [0x30, 0x20, 0x02, 0x01, 0x03, 0x30, 0x10, 0x06];
        assert_eq!(detect("a.pfx", "x", &short).unwrap().detail, "pkcs12");
        let bks = [0, 0, 0, 2, 0, 0, 0, 1];
        assert_eq!(detect("a.bks", "x", &bks).unwrap().detail, "bks");
        // wrong content under a keystore name
        assert_eq!(kind_of("a.jks", "x", b"<html>not a keystore</html>"), None);
        assert_eq!(
            kind_of(
                "a.p12",
                "x",
                &[0x30, 0x82, 0x01, 0x00, 0x05, 0x00, 0x00, 0x00]
            ),
            None
        );
    }

    #[test]
    fn android_signing_properties_count_only_with_a_password() {
        let props = "storePassword=hunter22hunter\nkeyPassword=hunter22hunter\nkeyAlias=upload\nstoreFile=../up.jks\n";
        let d = detect("key.properties", "android", props.as_bytes()).unwrap();
        assert_eq!(
            (d.kind, d.detail.as_str()),
            (Kind::Keystore, "signing config")
        );
        assert_eq!(
            d.keys,
            ["keyAlias", "keyPassword", "storeFile", "storePassword"]
        );
        assert_eq!(
            kind_of("key.properties", "android", b"keyAlias=upload\n"),
            None
        );
        assert_eq!(
            kind_of("key.properties", "android", b"storePassword=\n"),
            None
        );
        assert_eq!(
            kind_of("key.properties", "x", b"storePassword=${STORE_PASSWORD}\n"),
            None
        );
    }

    // --- JSON ----------------------------------------------------------------------

    #[test]
    fn a_service_account_is_found_whatever_the_file_is_called() {
        let d = detect("demo-1a2b3c4d5e6f.json", "downloads", &sa()).unwrap();
        assert_eq!(d.kind, Kind::ServiceAccount);
        assert!(d.keys.contains(&"private_key".to_string()));
        assert!(d.keys.contains(&"client_email".to_string()));
        assert!(d.detail.is_empty(), "no project id or email in the detail");
        // the type alone is not enough
        let bare = json!({"type": "service_account", "project_id": "x"}).to_string();
        assert_eq!(kind_of("a.json", "x", bare.as_bytes()), None);
    }

    #[test]
    fn google_services_json_is_a_service_config_by_name() {
        let body = json!({"project_info": {"project_id": "demo"}, "client": []}).to_string();
        let d = detect("google-services.json", "app", body.as_bytes()).unwrap();
        assert_eq!(
            (d.kind, d.detail.as_str()),
            (Kind::ServiceAccount, "google-services")
        );
        assert_eq!(kind_of("google-services.json", "app", b"{}"), None);
    }

    #[test]
    fn oauth_clients_tokens_and_cloud_logins_are_auth_json() {
        let cases: Vec<(&str, Value, &str)> = vec![
            (
                "credentials.json",
                json!({"installed": {"client_id": "id", "client_secret": "s3cr3tvalue"}}),
                "oauth-client",
            ),
            (
                "client_secret_1.json",
                json!({"web": {"client_secret": "s3cr3tvalue"}}),
                "oauth-client",
            ),
            (
                "token.json",
                json!({"token": "t", "refresh_token": "1//refreshvalue", "client_id": "i"}),
                "oauth-token",
            ),
            (
                "application_default_credentials.json",
                json!({"type": "authorized_user", "refresh_token": "1//refreshvalue", "client_id": "i"}),
                "authorized_user",
            ),
            (
                "config.json",
                json!({"auths": {"registry.example": {"auth": "dXNlcjpwYXNz"}}}),
                "docker",
            ),
            (
                "auth.json",
                json!({"github-oauth": {"github.com": "ghp_value"}}),
                "composer",
            ),
        ];
        for (name, body, detail) in cases {
            let d = detect(name, "x", body.to_string().as_bytes())
                .unwrap_or_else(|| panic!("{name} should be found"));
            assert_eq!(
                (d.kind, d.detail.as_str()),
                (Kind::AuthJson, detail),
                "{name}"
            );
        }
    }

    #[test]
    fn empty_or_placeholder_values_are_not_secrets() {
        for body in [
            json!({"installed": {"client_secret": ""}}),
            json!({"refresh_token": "   "}),
            json!({"auths": {"registry.example": {}}}),
            json!({"auths": {}}),
            json!({"github-oauth": {}}),
            json!({"type": "authorized_user"}),
            json!({"client_secret": "<your client secret>"}),
            json!({"client_secret": "${CLIENT_SECRET}"}),
            json!({"client_secret": "changeme"}),
            json!({"client_secret": "xxxxxxxxxxxx"}),
            json!({"client_secret": "short"}),
        ] {
            assert_eq!(
                kind_of("credentials.json", "x", body.to_string().as_bytes()),
                None,
                "{body}"
            );
        }
    }

    #[test]
    fn other_json_with_a_secret_key_is_secret_json_and_a_weak_key_needs_the_name() {
        let strong = json!({"service": {"nested": {"secret_key": "abcdef123456"}}}).to_string();
        let d = detect("whatever.json", "x", strong.as_bytes()).unwrap();
        assert_eq!(
            (d.kind, d.detail.as_str()),
            (Kind::SecretJson, "has secret_key")
        );
        let weak = json!({"db": {"password": "hunter22hunter"}}).to_string();
        assert_eq!(
            kind_of("settings-prod.json", "x", weak.as_bytes()),
            None,
            "name says nothing"
        );
        assert_eq!(
            kind_of("secrets.json", "x", weak.as_bytes()),
            Some(Kind::SecretJson)
        );
        assert_eq!(
            kind_of("db-credentials.json", "x", weak.as_bytes()),
            Some(Kind::SecretJson)
        );
        // too deep to count
        let deep = json!({"a": {"b": {"c": {"d": {"secret_key": "abcdef123456"}}}}}).to_string();
        assert_eq!(kind_of("x.json", "x", deep.as_bytes()), None);
    }

    #[test]
    fn json_that_is_not_an_object_or_not_json_is_nothing() {
        assert_eq!(kind_of("a.json", "x", b"[1, 2, 3]"), None);
        assert_eq!(kind_of("a.json", "x", b"not json at all"), None);
        assert_eq!(
            kind_of("a.json", "x", b"{\"private_key\": \"abcdefghijkl\""),
            None,
            "truncated"
        );
        assert_eq!(
            kind_of(
                "a.json",
                "x",
                b"// comment\n{\"private_key\": \"abcdefghijkl\"}"
            ),
            None
        );
        let nested = format!("{}{}", "[".repeat(1000), "]".repeat(1000));
        assert_eq!(
            kind_of("a.json", "x", nested.as_bytes()),
            None,
            "hostile nesting is refused"
        );
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_a_secret() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend(sa());
        assert_eq!(kind_of("a.json", "x", &bytes), Some(Kind::ServiceAccount));
    }

    #[test]
    fn json_keys_are_names_only_sorted_and_capped() {
        let mut map = serde_json::Map::new();
        for i in 0..150 {
            map.insert(format!("k{i:03}"), json!("v"));
        }
        map.insert("client_secret".into(), json!("abcdef123456"));
        let d = detect("a.json", "x", Value::Object(map).to_string().as_bytes()).unwrap();
        assert_eq!(d.keys.len(), 100);
        assert!(d.keys.windows(2).all(|w| w[0] <= w[1]));
        assert!(
            !d.keys.iter().any(|k| k.contains("abcdef")),
            "values are never kept"
        );
    }

    // --- credential dotfiles -------------------------------------------------------

    #[test]
    fn aws_credentials_need_a_real_secret_and_list_profiles() {
        let file = "[default]\naws_access_key_id = AKIAEXAMPLE\naws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCY\n\n[work]\naws_secret_access_key = abcdefghijklmnopqrstuvwx\n\n[empty]\naws_secret_access_key =\n";
        let d = detect("credentials", ".aws", file.as_bytes()).unwrap();
        assert_eq!(
            (d.kind, d.detail.as_str()),
            (Kind::AwsCredentials, "2 profiles")
        );
        assert_eq!(d.keys, ["default", "work"]);
        assert_eq!(
            kind_of(
                "credentials",
                ".aws",
                b"[default]\naws_access_key_id = AKIAEXAMPLE\n"
            ),
            None
        );
    }

    #[test]
    fn npmrc_needs_a_token_that_is_not_an_environment_reference() {
        let real =
            "registry=https://registry.example/\n//registry.example/:_authToken=npm_abcdefgh1234\n";
        assert_eq!(
            kind_of(".npmrc", "home", real.as_bytes()),
            Some(Kind::NpmToken)
        );
        let env = "//registry.example/:_authToken=${NPM_TOKEN}\n";
        assert_eq!(kind_of(".npmrc", "home", env.as_bytes()), None);
        assert_eq!(
            kind_of(".npmrc", "home", b"registry=https://registry.example/\n"),
            None
        );
        assert_eq!(
            kind_of(".npmrc", "home", b"; _authToken=npm_abcdefgh1234\n"),
            None,
            "a comment"
        );
    }

    #[test]
    fn netrc_pypirc_and_git_credentials() {
        let netrc = "machine a.example login me password hunter22\nmachine b.example login me password other\n";
        let d = detect(".netrc", "home", netrc.as_bytes()).unwrap();
        assert_eq!((d.kind, d.detail.as_str()), (Kind::Netrc, "2 machines"));
        assert_eq!(
            kind_of(".netrc", "home", b"machine a.example login me\n"),
            None
        );

        let pypirc = "[pypi]\nusername = __token__\npassword = pypi-AgEIcHlwaS5vcmcCabcdef\n";
        assert_eq!(
            detect(".pypirc", "home", pypirc.as_bytes()).unwrap().detail,
            "token"
        );
        assert_eq!(
            kind_of(".pypirc", "home", b"[pypi]\npassword = <password>\n"),
            None
        );

        let git = "https://me:ghp_abcdefghijkl@github.com\nhttps://other@example.com\n";
        let d = detect(".git-credentials", "home", git.as_bytes()).unwrap();
        assert_eq!(
            (d.kind, d.detail.as_str()),
            (Kind::GitCredentials, "1 login")
        );
        assert_eq!(
            kind_of(".git-credentials", "home", b"https://me@github.com\n"),
            None
        );
    }

    #[test]
    fn a_kubeconfig_needs_a_credential_not_just_a_certificate() {
        let with_token = "users:\n- name: me\n  user:\n    token: eyJhbGciOiJSUzI1NiJ9\n";
        let d = detect("config", ".kube", with_token.as_bytes()).unwrap();
        assert_eq!((d.kind, d.detail.as_str()), (Kind::Kubeconfig, "token"));
        let key = "users:\n- name: me\n  user:\n    client-key-data: QUJD\n";
        assert_eq!(
            detect("config", ".kube", key.as_bytes()).unwrap().detail,
            "client key"
        );
        let cert_only = "users:\n- name: me\n  user:\n    client-certificate-data: QUJD\n";
        assert_eq!(kind_of("config", ".kube", cert_only.as_bytes()), None);
        assert_eq!(
            kind_of("config", ".kube", b"users:\n- user:\n    token:\n"),
            None
        );
    }

    #[test]
    fn nothing_returned_ever_contains_a_secret_value() {
        let secret = "needle-needle-needle-needle";
        let samples: Vec<(&str, &str, Vec<u8>)> = vec![
            (
                "a.json",
                "x",
                json!({"client_secret": secret, "other": secret})
                    .to_string()
                    .into_bytes(),
            ),
            (
                "credentials",
                ".aws",
                format!("[p]\naws_secret_access_key = {secret}\n").into_bytes(),
            ),
            (".npmrc", "h", format!("_authToken={secret}\n").into_bytes()),
            (
                ".netrc",
                "h",
                format!("machine m login l password {secret}\n").into_bytes(),
            ),
            (
                "key.properties",
                "a",
                format!("storePassword={secret}\n").into_bytes(),
            ),
            (
                ".git-credentials",
                "h",
                format!("https://u:{secret}@h.example\n").into_bytes(),
            ),
        ];
        for (name, parent, bytes) in samples {
            let d = detect(name, parent, &bytes).unwrap_or_else(|| panic!("{name}"));
            let shown = format!("{d:?}");
            assert!(!shown.contains("needle"), "{name}: {shown}");
        }
    }
}
