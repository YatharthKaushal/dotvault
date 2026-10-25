//! Plaintext that gets encrypted into vault items and shares.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::dotenv::DotEnv;
use crate::error::Result;

pub const PAYLOAD_VERSION: u8 = 1;

/// The `kind` of a payload that holds one file instead of variables.
pub const FILE_KIND: &str = "file";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvPayload {
    pub v: u8,
    /// Human readable origin, e.g. `acme/api · production`.
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub entries: Vec<EnvEntry>,
}

impl EnvPayload {
    /// Collects entries from `env`. With `keys`, only those are taken (in the
    /// order requested); the second return value lists requested keys that
    /// the file does not define.
    pub fn from_dotenv(
        env: &DotEnv,
        source: impl Into<String>,
        keys: Option<&[String]>,
    ) -> (Self, Vec<String>) {
        let pairs = env.pairs();
        let mut missing = Vec::new();
        let entries = match keys {
            None => pairs
                .iter()
                .map(|(k, v)| EnvEntry {
                    key: k.to_string(),
                    value: v.to_string(),
                })
                .collect(),
            Some(wanted) => {
                let mut out: Vec<EnvEntry> = Vec::new();
                for k in wanted {
                    match pairs.iter().find(|(pk, _)| pk == k) {
                        Some((pk, pv)) if !out.iter().any(|e| e.key == *pk) => out.push(EnvEntry {
                            key: pk.to_string(),
                            value: pv.to_string(),
                        }),
                        Some(_) => {}
                        None => missing.push(k.clone()),
                    }
                }
                out
            }
        };
        (
            EnvPayload {
                v: PAYLOAD_VERSION,
                source: source.into(),
                note: None,
                entries,
            },
            missing,
        )
    }

    /// Renders the entries as a fresh dotenv file.
    pub fn to_dotenv(&self) -> Result<DotEnv> {
        let mut env = DotEnv::new();
        for e in &self.entries {
            env.set(&e.key, &e.value)?;
        }
        Ok(env)
    }

    /// A copy holding only `keys`, in the order requested. The second value
    /// lists the requested keys that are not present.
    pub fn select(&self, keys: &[String]) -> (EnvPayload, Vec<String>) {
        let mut entries: Vec<EnvEntry> = Vec::new();
        let mut missing = Vec::new();
        for k in keys {
            match self.entries.iter().find(|e| e.key == *k) {
                Some(e) if !entries.iter().any(|x| x.key == e.key) => entries.push(e.clone()),
                Some(_) => {}
                None => missing.push(k.clone()),
            }
        }
        (
            EnvPayload {
                entries,
                ..self.clone()
            },
            missing,
        )
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.value.as_str())
    }
}

/// One whole file: an SSH key, a keystore, a service-account JSON. Kept as raw
/// bytes, so binary files survive exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePayload {
    pub v: u8,
    /// Always [`FILE_KIND`]; what tells a file from a set of variables.
    pub kind: String,
    /// The file name as the sender had it. Shown, never used as a path.
    pub name: String,
    /// What dotvault took the file for, such as `ssh-key`. For display only.
    #[serde(default)]
    pub secret_kind: String,
    /// Unix permission bits of the original, for information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    pub size: u64,
    /// Human readable origin, e.g. `acme/id_ed25519`.
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(with = "crate::api::b64")]
    pub data: Vec<u8>,
}

impl FilePayload {
    pub fn new(
        name: impl Into<String>,
        secret_kind: impl Into<String>,
        mode: Option<u32>,
        data: Vec<u8>,
        source: impl Into<String>,
    ) -> Self {
        FilePayload {
            v: PAYLOAD_VERSION,
            kind: FILE_KIND.to_string(),
            name: name.into(),
            secret_kind: secret_kind.into(),
            mode,
            size: data.len() as u64,
            source: source.into(),
            note: None,
            data,
        }
    }

    /// Whether the stated size matches the bytes, so a damaged or tampered
    /// payload is noticed before anything is written.
    pub fn is_consistent(&self) -> bool {
        self.data.len() as u64 == self.size
    }

    /// The bytes as text, if they are text a terminal can show.
    pub fn as_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.data)
            .ok()
            .filter(|t| !t.contains('\0'))
    }
}

/// What a vault item or a share holds: variables, or one file.
///
/// Variables serialize exactly as an [`EnvPayload`] always did, so existing items
/// and shares are unchanged; a file adds `"kind": "file"`, which is how the two
/// are told apart on the way back in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Payload {
    Env(EnvPayload),
    File(FilePayload),
}

impl<'de> Deserialize<'de> for Payload {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let value = Value::deserialize(d)?;
        if value.get("kind").and_then(Value::as_str) == Some(FILE_KIND) {
            serde_json::from_value(value)
                .map(Payload::File)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Payload::Env)
                .map_err(D::Error::custom)
        }
    }
}

impl Payload {
    pub fn source(&self) -> &str {
        match self {
            Payload::Env(p) => &p.source,
            Payload::File(p) => &p.source,
        }
    }

    pub fn note(&self) -> Option<&str> {
        match self {
            Payload::Env(p) => p.note.as_deref(),
            Payload::File(p) => p.note.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> DotEnv {
        DotEnv::parse("# c\nA=1\nB=\"two words\"\nA=3\nC=x\n")
    }

    #[test]
    fn takes_everything_deduped() {
        let (p, missing) = EnvPayload::from_dotenv(&env(), "src", None);
        assert!(missing.is_empty());
        let keys: Vec<_> = p.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["A", "B", "C"]);
        assert_eq!(p.get("A"), Some("3"));
        assert_eq!(p.v, PAYLOAD_VERSION);
        assert_eq!(p.source, "src");
    }

    #[test]
    fn selects_requested_keys_in_order_and_reports_missing() {
        let wanted = vec![
            "C".to_string(),
            "NOPE".to_string(),
            "A".to_string(),
            "C".to_string(),
        ];
        let (p, missing) = EnvPayload::from_dotenv(&env(), "", Some(&wanted));
        let keys: Vec<_> = p.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["C", "A"]);
        assert_eq!(missing, ["NOPE"]);
        assert_eq!(p.get("B"), None);
    }

    #[test]
    fn select_filters_a_payload_and_keeps_metadata() {
        let (mut p, _) = EnvPayload::from_dotenv(&env(), "src", None);
        p.note = Some("hi".into());
        let (picked, missing) = p.select(&["C".to_string(), "ZZZ".to_string(), "A".to_string()]);
        let keys: Vec<_> = picked.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["C", "A"]);
        assert_eq!(missing, ["ZZZ"]);
        assert_eq!(picked.source, "src");
        assert_eq!(picked.note.as_deref(), Some("hi"));
    }

    #[test]
    fn renders_back_to_dotenv() {
        let (p, _) = EnvPayload::from_dotenv(&env(), "", None);
        assert_eq!(
            p.to_dotenv().unwrap().to_string(),
            "A=3\nB=\"two words\"\nC=x\n"
        );
    }

    #[test]
    fn json_shape_is_stable() {
        let p = EnvPayload {
            v: 1,
            source: "s".into(),
            note: None,
            entries: vec![EnvEntry {
                key: "K".into(),
                value: "V".into(),
            }],
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"v":1,"source":"s","entries":[{"key":"K","value":"V"}]}"#
        );
        let back: EnvPayload = serde_json::from_str(r#"{"v":1,"entries":[]}"#).unwrap();
        assert_eq!(back.source, "");
    }

    // --- files ---------------------------------------------------------------

    fn file(data: &[u8]) -> FilePayload {
        let mut f = FilePayload::new(
            "id_ed25519",
            "ssh-key",
            Some(0o600),
            data.to_vec(),
            "keys/id",
        );
        f.note = Some("for the deploy box".to_string());
        f
    }

    #[test]
    fn a_file_round_trips_byte_for_byte_including_binary() {
        let bytes: Vec<u8> = (0..=255u8).chain([0, 0, 255, 254]).collect();
        let sent = Payload::File(file(&bytes));
        let json = serde_json::to_vec(&sent).unwrap();
        let back: Payload = serde_json::from_slice(&json).unwrap();
        assert_eq!(back, sent);
        match back {
            Payload::File(f) => {
                assert_eq!(f.data, bytes);
                assert!(f.is_consistent());
                assert!(f.as_text().is_none(), "binary data is not text");
                assert_eq!(f.mode, Some(0o600));
            }
            Payload::Env(_) => panic!("a file must stay a file"),
        }
    }

    #[test]
    fn variables_serialize_exactly_as_they_always_did() {
        let (env, _) = EnvPayload::from_dotenv(&DotEnv::parse("A=1\nB=two\n"), "src", None);
        let plain = serde_json::to_string(&env).unwrap();
        let wrapped = serde_json::to_string(&Payload::Env(env.clone())).unwrap();
        assert_eq!(plain, wrapped, "existing items and shares must not change");
        assert!(!wrapped.contains("kind"));
        // and what an older version wrote still reads
        assert_eq!(
            serde_json::from_str::<Payload>(&plain).unwrap(),
            Payload::Env(env)
        );
    }

    #[test]
    fn only_kind_file_makes_a_file_and_garbage_is_an_error() {
        let env = r#"{"v":1,"source":"s","entries":[{"key":"A","value":"1"}]}"#;
        assert!(matches!(
            serde_json::from_str::<Payload>(env),
            Ok(Payload::Env(_))
        ));
        let other_kind = r#"{"v":1,"kind":"env","source":"s","entries":[]}"#;
        assert!(matches!(
            serde_json::from_str::<Payload>(other_kind),
            Ok(Payload::Env(_))
        ));
        for bad in [
            r#"{"v":1,"kind":"file"}"#,
            r#"{"v":1,"kind":"file","name":"x","size":1,"data":"!!not base64!!"}"#,
            r#"{"v":1}"#,
            "[]",
            "null",
        ] {
            assert!(serde_json::from_str::<Payload>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_file_payload_that_lies_about_its_size_is_noticed() {
        let mut f = file(b"hello");
        assert!(f.is_consistent());
        f.size = 99;
        assert!(!f.is_consistent());
        assert_eq!(file(b"plain text").as_text(), Some("plain text"));
        assert!(file(b"has\0nul").as_text().is_none());
    }

    #[test]
    fn an_older_reader_cannot_mistake_a_file_for_variables() {
        // what an older dotvault does with a file: parse it as variables, and fail
        let json = serde_json::to_vec(&Payload::File(file(b"secret"))).unwrap();
        assert!(serde_json::from_slice::<EnvPayload>(&json).is_err());
    }

    #[test]
    fn helpers_see_through_the_enum() {
        let mut f = file(b"x");
        f.source = "where/from".into();
        let p = Payload::File(f);
        assert_eq!(
            (p.source(), p.note()),
            ("where/from", Some("for the deploy box"))
        );
        let (env, _) = EnvPayload::from_dotenv(&DotEnv::parse("A=1\n"), "e", None);
        assert_eq!(Payload::Env(env).note(), None);
    }
}
