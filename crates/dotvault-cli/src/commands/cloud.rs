//! Helpers shared by the commands that talk to the server.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use dotvault_core::api::{
    CreateItemRequest, ItemDetail, ListItemsResponse, UpdateItemRequest, MAX_ITEM_NAME_BYTES,
    MAX_ITEM_PAYLOAD_BYTES,
};
use dotvault_core::crypto::{self, derive_account_keys, unlock_with_password, Key};
use dotvault_core::dotenv::DotEnv;
use dotvault_core::payload::EnvPayload;
use dotvault_core::share_code;

use super::{read_text, Ctx};
use crate::client::{normalize_server, Api, ApiError};
use crate::fsutil;
use crate::output::{plural, tilde};
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

    /// Server for register / login / recover: the flag, else the config,
    /// else the server of the current session.
    pub fn pick_server(&self, flag: Option<&str>) -> Result<String> {
        let chosen = flag
            .map(str::to_string)
            .or_else(|| self.config.server.clone())
            .or_else(|| self.session_store().load().ok().flatten().map(|s| s.server))
            .ok_or_else(|| anyhow!("no server configured; pass --server https://your-server"))?;
        normalize_server(&chosen)
    }

    /// Saves the server in the config the first time, so it need not be retyped.
    pub fn remember_server(&mut self, server: &str) {
        if self.config.server.is_none() {
            self.config.server = Some(server.to_string());
            let _ = self.config.save(&self.dirs);
        }
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

    pub fn read_item(&self, vault_key: &Key, item: &ItemMeta) -> Result<EnvPayload> {
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
    pub fn write_item(
        &self,
        vault_key: &Key,
        existing: Option<&ItemMeta>,
        name: &str,
        payload: &EnvPayload,
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
            "{} already exists; add --merge to add the variables to it, or --force to replace it",
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
