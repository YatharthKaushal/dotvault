//! Helpers shared by the commands that talk to the server.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use dotvault_core::api::{
    CreateItemRequest, ItemDetail, ListItemsResponse, UpdateItemRequest, MAX_ITEM_NAME_BYTES,
    MAX_ITEM_PAYLOAD_BYTES,
};
use dotvault_core::crypto::{self, derive_account_keys, unlock_with_password, Key};
use dotvault_core::dotenv::DotEnv;
use dotvault_core::fingerprint::fingerprint_bytes;
use dotvault_core::payload::{EnvPayload, FilePayload, Payload};
use dotvault_core::share_code;
use owo_colors::OwoColorize;

use super::{read_text, Ctx};
use crate::client::{normalize_server, Api, ApiError};
use crate::fsutil;
use crate::output::{plural, sanitize, tilde};
use crate::prompt;
use crate::session::{Session, SessionStore};

impl Ctx {
    pub fn session_store(&self) -> SessionStore {
        SessionStore::new(&self.dirs)
    }

    /// The logged-in session and a client for its server.
    pub fn cloud(&self) -> Result<Cloud> {
        let session = self.session_store().load()?.ok_or_else(|| {
            anyhow!("you are not logged in; run `dotvault login` (or `dotvault register`)")
        })?;
        let api = Api::new(&session.server, Some(session.token.clone()))?;
        Ok(Cloud { session, api })
    }

    /// Server for register / login / recover: the flag or `DOTVAULT_SERVER`, else
    /// the config, else the server of the current session, else [`DEFAULT_SERVER`].
    /// Says so when the default is used, so nobody registers on it by accident.
    pub fn pick_server(&self, flag: Option<&str>) -> Result<String> {
        let session = self.session_store().load().ok().flatten().map(|s| s.server);
        let (chosen, defaulted) =
            choose_server(flag, self.config.server.as_deref(), session.as_deref());
        let server = normalize_server(&chosen)?;
        if defaulted {
            anstream::eprintln!(
                "{}",
                format!(
                    "Using the default server {server}. Use --server or DOTVAULT_SERVER for another one."
                )
                .dimmed()
            );
        }
        Ok(server)
    }

    /// Saves the server in the config the first time, so it need not be retyped.
    pub fn remember_server(&mut self, server: &str) {
        if self.config.server.is_none() {
            self.config.server = Some(server.to_string());
            let _ = self.config.save(&self.dirs);
        }
    }
}

/// The hosted server, used when nothing else says which one to talk to. The first
/// successful `register` or `login` saves the chosen server in the config, so a
/// later release that changes this value cannot silently move anyone.
pub const DEFAULT_SERVER: &str = "https://dotvault.synquic.tech";

/// Which server to use, and whether it is the built-in default. Order: the flag
/// (or `DOTVAULT_SERVER`), the config file, the saved session, the default.
fn choose_server(
    flag: Option<&str>,
    configured: Option<&str>,
    session: Option<&str>,
) -> (String, bool) {
    match flag.or(configured).or(session) {
        Some(server) => (server.to_string(), false),
        None => (DEFAULT_SERVER.to_string(), true),
    }
}

pub struct Cloud {
    pub session: Session,
    pub api: Api,
}

#[derive(Debug)]
pub struct ItemMeta {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub updated_at: i64,
}

/// Turns API failures into messages for a person.
pub fn friendly(e: ApiError) -> anyhow::Error {
    match &e {
        ApiError::Http {
            status: 401, code, ..
        } if code != "invalid_credentials" => anyhow!(
            "you are not logged in, or your session expired or was revoked; run `dotvault login`"
        ),
        ApiError::Http { code, .. } if code == "invalid_credentials" => {
            anyhow!("wrong username or password")
        }
        ApiError::Http { status: 429, .. } => {
            anyhow!("too many requests; wait a minute and try again")
        }
        _ => anyhow::Error::new(e),
    }
}

impl Cloud {
    /// The vault key: cached in the session, or unwrapped with the password.
    pub fn vault_key(&self) -> Result<Key> {
        if let Some(key) = self.session.cached_vault_key() {
            return Ok(key);
        }
        if !self.session.has_vault_material() {
            bail!("this session has no vault key (it comes from DOTVAULT_TOKEN); run `dotvault login` to use the vault");
        }
        let password = prompt::password("Password: ", "DOTVAULT_PASSWORD")?;
        let keys =
            derive_account_keys(&password, &self.session.kdf_salt, &self.session.kdf_params)?;
        unlock_with_password(&keys, &self.session.wrapped_vault_key)
            .map_err(|_| anyhow!("wrong password"))
    }

    /// Item names are encrypted, so listing means decrypting. Returns the
    /// items and how many could not be decrypted.
    pub fn items(&self, vault_key: &Key) -> Result<(Vec<ItemMeta>, usize)> {
        let list: ListItemsResponse = self.api.get("/vault/items").map_err(friendly)?;
        let mut items = Vec::with_capacity(list.items.len());
        let mut undecryptable = 0;
        for it in list.items {
            match crypto::open(vault_key, &it.name_ct, &crypto::aad_item_name(&it.id)) {
                Ok(name) => items.push(ItemMeta {
                    name: String::from_utf8_lossy(&name).into_owned(),
                    id: it.id,
                    version: it.version,
                    updated_at: it.updated_at,
                }),
                Err(_) => undecryptable += 1,
            }
        }
        items.sort_by(|a, b| a.name.cmp(&b.name));
        Ok((items, undecryptable))
    }

    pub fn read_item(&self, vault_key: &Key, item: &ItemMeta) -> Result<Payload> {
        let detail: ItemDetail = self
            .api
            .get(&format!("/vault/items/{}", item.id))
            .map_err(friendly)?;
        crypto::open_json(
            vault_key,
            &detail.payload_ct,
            &crypto::aad_item_payload(&detail.id, detail.version),
        )
        .map_err(|_| anyhow!("could not decrypt {:?}; it may be damaged", item.name))
    }

    /// Creates the item, or adds a new version when `existing` is given.
    /// Returns the item id and its version.
    pub fn write_item<T: serde::Serialize>(
        &self,
        vault_key: &Key,
        existing: Option<&ItemMeta>,
        name: &str,
        payload: &T,
    ) -> Result<(String, u32)> {
        let id = match existing {
            Some(e) => e.id.clone(),
            None => share_code::new_id()?,
        };
        let version = existing.map_or(1, |e| e.version + 1);

        let name_ct = crypto::seal(vault_key, name.as_bytes(), &crypto::aad_item_name(&id))?;
        if name_ct.len() > MAX_ITEM_NAME_BYTES {
            bail!("the name is too long");
        }
        let payload_ct =
            crypto::seal_json(vault_key, payload, &crypto::aad_item_payload(&id, version))?;
        if payload_ct.len() > MAX_ITEM_PAYLOAD_BYTES {
            bail!(
                "this is too large for the vault ({} KiB, the limit is {} KiB)",
                payload_ct.len() / 1024,
                MAX_ITEM_PAYLOAD_BYTES / 1024
            );
        }

        let detail: ItemDetail = match existing {
            Some(_) => self
                .api
                .put(
                    &format!("/vault/items/{id}"),
                    &UpdateItemRequest { name_ct, payload_ct, version },
                )
                .map_err(|e| match e.code() {
                    Some("conflict") => anyhow!(
                        "{name:?} was changed from another device in the meantime; run the command again"
                    ),
                    _ => friendly(e),
                })?,
            None => self
                .api
                .post(
                    "/vault/items",
                    &CreateItemRequest { id: id.clone(), name_ct, payload_ct },
                )
                .map_err(friendly)?,
        };
        Ok((detail.id, detail.version))
    }
}

/// A file this large cannot fit in a vault item once sealed, so it is not even read.
const MAX_FILE_READ: u64 = MAX_ITEM_PAYLOAD_BYTES as u64;

/// Reads a whole file into a payload that can be sealed into the vault or a share.
pub fn file_payload(path: &Path, secret_kind: &str, source: &str) -> Result<FilePayload> {
    let md = std::fs::metadata(path).with_context(|| format!("cannot read {}", tilde(path)))?;
    if !md.is_file() {
        bail!("{} is not a file", tilde(path));
    }
    if md.len() > MAX_FILE_READ {
        bail!(
            "{} is {} KiB; the vault holds files up to about {} KiB",
            tilde(path),
            md.len() / 1024,
            MAX_ITEM_PAYLOAD_BYTES * 3 / 4 / 1024
        );
    }
    let data = std::fs::read(path).with_context(|| format!("cannot read {}", tilde(path)))?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(md.permissions().mode() & 0o777)
    };
    #[cfg(not(unix))]
    let mode = None;
    let name = path
        .file_name()
        .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().into_owned());
    Ok(FilePayload::new(name, secret_kind, mode, data, source))
}

/// `ssh-key, 411 bytes`, safe to print.
pub fn describe_file(file: &FilePayload) -> String {
    let kind = if file.secret_kind.is_empty() {
        "file"
    } else {
        &file.secret_kind
    };
    format!("{}, {} bytes", sanitize(kind), file.size)
}

/// A short tag of the file's bytes, so two copies can be compared without opening them.
pub fn file_tag(file: &FilePayload) -> String {
    fingerprint_bytes(&file.data)
}

/// Writes a file received from the vault or a share, exactly as it was and readable
/// only by you. The sender's file name is never used: the destination is yours.
pub fn write_secret_file(path: &Path, file: &FilePayload, force: bool) -> Result<String> {
    if !file.is_consistent() {
        bail!("the file arrived damaged (its size does not match); nothing was written");
    }
    if path.is_dir() {
        bail!("{} is a directory", path.display());
    }
    if let Some(folder) = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty() && !p.is_dir())
    {
        bail!(
            "the folder {} does not exist; create it first",
            tilde(folder)
        );
    }
    if path.exists() && !force {
        bail!("{} already exists; add --force to replace it", tilde(path));
    }
    fsutil::write_private(path, &file.data)
        .with_context(|| format!("cannot write {}", tilde(path)))?;
    Ok(format!(
        "Wrote {} ({}) to {}, readable only by you",
        sanitize(&file.name),
        describe_file(file),
        tilde(path)
    ))
}

/// Exact name, then case-insensitive, then a unique substring.
pub fn find_item<'a>(items: &'a [ItemMeta], wanted: &str) -> Result<&'a ItemMeta> {
    if let Some(i) = items.iter().find(|i| i.name == wanted) {
        return Ok(i);
    }
    let lower = wanted.to_lowercase();
    if let Some(i) = items.iter().find(|i| i.name.to_lowercase() == lower) {
        return Ok(i);
    }
    let partial: Vec<&ItemMeta> = items
        .iter()
        .filter(|i| i.name.to_lowercase().contains(&lower))
        .collect();
    match partial.as_slice() {
        [one] => Ok(one),
        [] => bail!("no vault item named {wanted:?}; see `dotvault vault ls`"),
        many => bail!(
            "{wanted:?} matches several items: {}",
            many.iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Fails early, before anything irreversible happens, if `path` can't be written.
pub fn check_write_target(path: &Path, force: bool, merge: bool) -> Result<()> {
    if path.is_dir() {
        bail!("{} is a directory", path.display());
    }
    if path.exists() && !force && !merge {
        bail!(
            "{} already exists; add --force to replace it (or --merge to add variables to an env file)",
            tilde(path)
        );
    }
    Ok(())
}

/// Writes the payload as an env file. With `merge`, existing files keep
/// their comments, layout and other variables.
pub fn write_env_file(
    path: &Path,
    payload: &EnvPayload,
    force: bool,
    merge: bool,
) -> Result<String> {
    check_write_target(path, force, merge)?;
    let (mut env, existed) = if path.exists() && merge {
        (DotEnv::parse(&read_text(path, true)?), true)
    } else {
        (DotEnv::new(), false)
    };

    let (mut added, mut updated) = (0, 0);
    for e in &payload.entries {
        match env.get(&e.key) {
            None => added += 1,
            Some(v) if v != e.value => updated += 1,
            Some(_) => {}
        }
        env.set(&e.key, &e.value)?;
    }
    fsutil::write_atomic(path, env.to_string().as_bytes())?;

    let total = payload.entries.len();
    Ok(if existed {
        format!(
            "Merged {} into {} ({added} added, {updated} changed, {} unchanged)",
            plural(total, "variable"),
            tilde(path),
            total - added - updated
        )
    } else {
        format!("Wrote {} to {}", plural(total, "variable"), tilde(path))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotvault_core::payload::EnvEntry;

    fn file(data: &[u8]) -> FilePayload {
        FilePayload::new(
            "id_ed25519",
            "ssh-key",
            Some(0o644),
            data.to_vec(),
            "keys/id",
        )
    }

    #[test]
    fn a_received_file_is_written_exactly_privately_and_never_over_another_by_accident() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("key");
        let bytes: Vec<u8> = (0..=255u8).collect();
        let message = write_secret_file(&target, &file(&bytes), false).unwrap();
        assert!(
            message.contains("readable only by you") && message.contains("ssh-key"),
            "{message}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&target), 0o600, "the sender's 0644 is not copied");

            // replacing a more permissive file leaves an owner-only one
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
            write_secret_file(&target, &file(b"new"), true).unwrap();
            assert_eq!(mode(&target), 0o600);
        }

        let err = write_secret_file(&target, &file(b"newer"), false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("already exists") && err.contains("--force"),
            "{err}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"new",
            "the existing file is untouched"
        );
        write_secret_file(&target, &file(b"newer"), true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"newer");
    }

    #[test]
    fn a_received_file_is_refused_when_it_cannot_be_written_safely() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_secret_file(&dir.path().join("nope/key"), &file(b"x"), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not exist; create it first"), "{err}");
        let err = write_secret_file(dir.path(), &file(b"x"), true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("is a directory"), "{err}");
        let mut lying = file(b"12345");
        lying.size = 99;
        let target = dir.path().join("k");
        let err = write_secret_file(&target, &lying, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("damaged"), "{err}");
        assert!(!target.exists(), "nothing is written for a damaged file");
    }

    #[test]
    fn a_file_is_read_whole_with_its_name_and_never_when_it_is_too_big() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("release.jks");
        std::fs::write(&path, [0xFE, 0xED, 0, 255]).unwrap();
        let payload = file_payload(&path, "keystore", "proj/release.jks").unwrap();
        assert_eq!(
            (payload.name.as_str(), payload.size, payload.data.clone()),
            ("release.jks", 4, vec![0xFE, 0xED, 0, 255])
        );
        assert_eq!(describe_file(&payload), "keystore, 4 bytes");
        assert_eq!(file_tag(&payload), file_tag(&payload.clone()));

        let big = dir.path().join("big");
        std::fs::write(&big, vec![0u8; MAX_FILE_READ as usize + 1]).unwrap();
        let err = file_payload(&big, "file", "s").unwrap_err().to_string();
        assert!(
            err.contains("KiB") && err.contains("vault holds files up to"),
            "{err}"
        );
        assert!(
            file_payload(dir.path(), "file", "s").is_err(),
            "a folder is not a file"
        );
        assert!(file_payload(&dir.path().join("missing"), "file", "s").is_err());
    }

    fn items(names: &[&str]) -> Vec<ItemMeta> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| ItemMeta {
                id: format!("id{i}"),
                name: n.to_string(),
                version: 1,
                updated_at: 0,
            })
            .collect()
    }

    fn payload(pairs: &[(&str, &str)]) -> EnvPayload {
        EnvPayload {
            v: 1,
            source: String::new(),
            note: None,
            entries: pairs
                .iter()
                .map(|(k, v)| EnvEntry {
                    key: k.to_string(),
                    value: v.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn finds_items_by_exact_then_loose_name() {
        let list = items(&["acme/api/prod", "acme/api/dev", "web/prod"]);
        assert_eq!(find_item(&list, "web/prod").unwrap().id, "id2");
        assert_eq!(find_item(&list, "WEB/PROD").unwrap().id, "id2");
        assert_eq!(find_item(&list, "api/dev").unwrap().id, "id1");
        let err = find_item(&list, "prod").unwrap_err().to_string();
        assert!(
            err.contains("acme/api/prod") && err.contains("web/prod"),
            "{err}"
        );
        assert!(find_item(&list, "nothing").is_err());
        // an exact name beats a longer name that merely contains it
        let nested = items(&["app", "app/prod"]);
        assert_eq!(find_item(&nested, "app").unwrap().id, "id0");
    }

    #[test]
    fn write_refuses_to_clobber_without_a_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "KEEP=1\n").unwrap();
        let err = write_env_file(&path, &payload(&[("A", "1")]), false, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--merge") && err.contains("--force"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "KEEP=1\n");
    }

    #[test]
    fn write_force_replaces_and_merge_preserves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "# mine\nKEEP=1\nA=old\n").unwrap();

        let msg =
            write_env_file(&path, &payload(&[("A", "new"), ("B", "2")]), false, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# mine\nKEEP=1\nA=new\nB=2\n"
        );
        assert!(
            msg.contains("1 added") && msg.contains("1 changed"),
            "{msg}"
        );

        write_env_file(&path, &payload(&[("ONLY", "x")]), true, false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ONLY=x\n");
    }

    #[test]
    fn write_creates_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.env");
        let msg = write_env_file(&path, &payload(&[("A", "1")]), false, false).unwrap();
        assert!(msg.starts_with("Wrote 1 variable to"), "{msg}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A=1\n");
    }

    #[test]
    fn server_choice_follows_the_documented_order() {
        let (s, defaulted) = choose_server(
            Some("https://flag"),
            Some("https://cfg"),
            Some("https://sess"),
        );
        assert_eq!((s.as_str(), defaulted), ("https://flag", false));
        let (s, defaulted) = choose_server(None, Some("https://cfg"), Some("https://sess"));
        assert_eq!((s.as_str(), defaulted), ("https://cfg", false));
        let (s, defaulted) = choose_server(None, None, Some("https://sess"));
        assert_eq!((s.as_str(), defaulted), ("https://sess", false));
        let (s, defaulted) = choose_server(None, None, None);
        assert_eq!((s.as_str(), defaulted), (DEFAULT_SERVER, true));
    }

    #[test]
    fn the_default_server_is_a_valid_https_origin() {
        assert!(DEFAULT_SERVER.starts_with("https://"));
        // normalizing must not change it, or the saved config would differ from the constant
        assert_eq!(normalize_server(DEFAULT_SERVER).unwrap(), DEFAULT_SERVER);
    }

    #[test]
    fn the_chosen_server_is_saved_once_and_then_kept() {
        use crate::config::{Config, Dirs};
        let dir = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: dir.path().join("cfg"),
            data: dir.path().join("data"),
        };
        let mut ctx = Ctx {
            dirs: dirs.clone(),
            config: Config::default(),
        };

        ctx.remember_server(DEFAULT_SERVER);
        assert_eq!(
            Config::load(&dirs).unwrap().server.as_deref(),
            Some(DEFAULT_SERVER)
        );

        // a later choice (for example another --server) must not rewrite the saved one
        ctx.remember_server("https://other.example");
        assert_eq!(
            Config::load(&dirs).unwrap().server.as_deref(),
            Some(DEFAULT_SERVER)
        );
    }

    #[test]
    fn friendly_errors() {
        let unauthorized = ApiError::Http {
            status: 401,
            code: "unauthorized".into(),
            message: "x".into(),
        };
        assert!(friendly(unauthorized)
            .to_string()
            .contains("dotvault login"));
        // the CLI words this itself, whatever the server's text is
        let bad_login = ApiError::Http {
            status: 401,
            code: "invalid_credentials".into(),
            message: "invalid username or password".into(),
        };
        assert_eq!(
            friendly(bad_login).to_string(),
            "wrong username or password"
        );
    }
}
