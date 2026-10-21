//! Plaintext that gets encrypted into vault items and shares.

use serde::{Deserialize, Serialize};

use crate::dotenv::DotEnv;
use crate::error::Result;

pub const PAYLOAD_VERSION: u8 = 1;

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
}
