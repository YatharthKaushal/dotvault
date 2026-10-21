//! The logged-in session: which server, who, the session token, and what is
//! needed to unlock the vault key.
//!
//! Kept in `session.json` in the config directory with mode 0600. The token
//! can be revoked server-side. The vault key is cached there by default;
//! `login --no-cache-key` leaves it out, and vault commands then ask for the
//! password and unwrap it from `wrapped_vault_key`.

use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use dotvault_core::api::b64;
use dotvault_core::crypto::{KdfParams, Key, KEY_LEN};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::config::Dirs;
use crate::fsutil;

#[derive(Serialize, Deserialize, Clone)]
pub struct Session {
    pub server: String,
    pub username: String,
    pub token: String,
    pub expires_at: i64,
    /// Empty for sessions that come from `DOTVAULT_TOKEN`.
    #[serde(with = "b64")]
    pub wrapped_vault_key: Vec<u8>,
    #[serde(with = "b64")]
    pub kdf_salt: Vec<u8>,
    pub kdf_params: KdfParams,
    /// The unwrapped vault key, base64url, unless the user opted out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vault_key: Option<String>,
}

// Never let a token or key reach a log line by accident.
impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("server", &self.server)
            .field("username", &self.username)
            .field("token", &"<redacted>")
            .field("vault_key", &self.vault_key.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn new(
        server: String,
        username: String,
        token: String,
        expires_at: i64,
        wrapped_vault_key: Vec<u8>,
        kdf_salt: Vec<u8>,
        kdf_params: KdfParams,
    ) -> Session {
        Session {
            server,
            username,
            token,
            expires_at,
            wrapped_vault_key,
            kdf_salt,
            kdf_params,
            vault_key: None,
        }
    }

    pub fn cache_vault_key(&mut self, key: Option<&Key>) {
        self.vault_key = key.map(|k| URL_SAFE_NO_PAD.encode(&k[..]));
    }

    pub fn cached_vault_key(&self) -> Option<Key> {
        let bytes = URL_SAFE_NO_PAD.decode(self.vault_key.as_deref()?).ok()?;
        let key: [u8; KEY_LEN] = bytes.try_into().ok()?;
        Some(Zeroizing::new(key))
    }

    /// Whether the vault key can be unwrapped locally at all.
    pub fn has_vault_material(&self) -> bool {
        !self.wrapped_vault_key.is_empty() && !self.kdf_salt.is_empty()
    }

    /// `DOTVAULT_TOKEN` (+ `DOTVAULT_SERVER`) for scripts and CI. Such a
    /// session can share and open but has no vault key.
    fn from_env() -> Result<Option<Session>> {
        let Some(token) = std::env::var("DOTVAULT_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
        else {
            return Ok(None);
        };
        let server = std::env::var("DOTVAULT_SERVER")
            .ok()
            .filter(|s| !s.is_empty())
            .context("DOTVAULT_TOKEN is set but DOTVAULT_SERVER is not")?;
        Ok(Some(Session {
            server,
            username: "(from DOTVAULT_TOKEN)".into(),
            token,
            expires_at: 0,
            wrapped_vault_key: Vec::new(),
            kdf_salt: Vec::new(),
            kdf_params: KdfParams::default(),
            vault_key: None,
        }))
    }
}

pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(dirs: &Dirs) -> SessionStore {
        SessionStore {
            path: dirs.config.join("session.json"),
        }
    }

    /// The environment wins over the file.
    pub fn load(&self) -> Result<Option<Session>> {
        if let Some(session) = Session::from_env()? {
            return Ok(Some(session));
        }
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| format!("cannot read {}", self.path.display()))
            }
        };
        self.warn_if_exposed();
        let session = serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is damaged; run `dotvault login` again",
                self.path.display()
            )
        })?;
        Ok(Some(session))
    }

    pub fn save(&self, session: &Session) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let json = serde_json::to_vec_pretty(session)?;
        fsutil::write_private(&self.path, &json)
            .with_context(|| format!("cannot write {}", self.path.display()))
    }

    /// Returns whether there was a session file to remove.
    pub fn clear(&self) -> Result<bool> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("cannot remove {}", self.path.display())),
        }
    }

    #[cfg(unix)]
    fn warn_if_exposed(&self) {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(md) = std::fs::metadata(&self.path) {
            if md.permissions().mode() & 0o077 != 0 {
                anstream::eprintln!(
                    "warning: {} can be read by other users; run: chmod 600 {}",
                    self.path.display(),
                    self.path.display()
                );
            }
        }
    }

    #[cfg(not(unix))]
    fn warn_if_exposed(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        Session::new(
            "https://v.example".into(),
            "ada".into(),
            "tok-secret".into(),
            123,
            vec![1, 2, 3],
            vec![4; 16],
            KdfParams::default(),
        )
    }

    fn store() -> (tempfile::TempDir, SessionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(&Dirs {
            config: dir.path().to_path_buf(),
            data: dir.path().to_path_buf(),
        });
        (dir, store)
    }

    #[test]
    fn roundtrips_with_and_without_cached_key() {
        let (_dir, store) = store();
        assert!(store.load().unwrap().is_none());

        let mut s = sample();
        let key: Key = Zeroizing::new([9u8; KEY_LEN]);
        s.cache_vault_key(Some(&key));
        store.save(&s).unwrap();
        let back = store.load().unwrap().unwrap();
        assert_eq!(back.username, "ada");
        assert_eq!(back.wrapped_vault_key, vec![1, 2, 3]);
        assert_eq!(*back.cached_vault_key().unwrap(), [9u8; KEY_LEN]);

        s.cache_vault_key(None);
        store.save(&s).unwrap();
        let text = std::fs::read_to_string(&store.path).unwrap();
        // quoted on both sides: "wrapped_vault_key" must not count
        assert!(
            !text.contains("\"vault_key\""),
            "no cached key field: {text}"
        );
        assert!(store.load().unwrap().unwrap().cached_vault_key().is_none());

        assert!(store.clear().unwrap());
        assert!(!store.clear().unwrap());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn debug_output_hides_secrets() {
        let mut s = sample();
        s.cache_vault_key(Some(&Zeroizing::new([7u8; KEY_LEN])));
        let shown = format!("{s:?}");
        assert!(!shown.contains("tok-secret"));
        assert!(!shown.contains(&URL_SAFE_NO_PAD.encode([7u8; KEY_LEN])));
        assert!(shown.contains("ada"));
    }

    #[cfg(unix)]
    #[test]
    fn session_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store();
        store.save(&sample()).unwrap();
        let mode = std::fs::metadata(&store.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn damaged_file_gives_a_clear_error() {
        let (_dir, store) = store();
        std::fs::write(&store.path, "{ not json").unwrap();
        let err = store.load().unwrap_err().to_string();
        assert!(err.contains("dotvault login"), "{err}");
    }
}
