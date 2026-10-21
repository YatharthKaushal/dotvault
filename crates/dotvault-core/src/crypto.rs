//! End-to-end crypto.
//!
//! ```text
//! password ──Argon2id(salt)──▶ master ──HKDF──▶ auth_key  (sent to server, server hashes it again)
//!                                        └────▶ enc_key   (never leaves the client)
//! vault_key: random 32 bytes, stored only as XChaCha20-Poly1305(enc_key, vault_key)
//! recovery_key: random 32 bytes shown once; wraps vault_key a second time
//! ```
//!
//! Everything the server stores is either a hash of a derived secret or
//! ciphertext. Sealed blobs are `version(1) || nonce(24) || ciphertext || tag(16)`.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};
use data_encoding::BASE32_NOPAD;
use hkdf::Hkdf;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const SALT_LEN: usize = 16;
const TAG_LEN: usize = 16;
const SEAL_VERSION: u8 = 1;
const RECOVERY_PREFIX: &str = "DVR1";

pub type Key = Zeroizing<[u8; KEY_LEN]>;

const CTX_WRAP_PASSWORD: &[u8] = b"dotvault/v1/wrap/vault-key/password";
const CTX_WRAP_RECOVERY: &[u8] = b"dotvault/v1/wrap/vault-key/recovery";

pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|_| Error::Random)?;
    Ok(buf)
}

pub fn random_key() -> Result<Key> {
    Ok(Zeroizing::new(random_bytes()?))
}

// ---------------------------------------------------------------------------
// Password-based key derivation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            m_kib: 64 * 1024,
            t: 3,
            p: 4,
        }
    }
}

impl KdfParams {
    /// Parameters come from the server, so they are bounded on both sides: a
    /// hostile server must not be able to weaken the KDF or exhaust our memory.
    pub fn validate(&self) -> Result<()> {
        let ok = (19 * 1024..=1024 * 1024).contains(&self.m_kib)
            && (2..=16).contains(&self.t)
            && (1..=16).contains(&self.p);
        if ok {
            Ok(())
        } else {
            Err(Error::Kdf("parameters outside the accepted range".into()))
        }
    }
}

/// The two keys derived from a password.
pub struct AccountKeys {
    /// Proves knowledge of the password to the server.
    pub auth_key: Key,
    /// Wraps the vault key. Never sent anywhere.
    pub enc_key: Key,
}

pub fn derive_account_keys(password: &str, salt: &[u8], params: &KdfParams) -> Result<AccountKeys> {
    params.validate()?;
    let argon_params = Params::new(params.m_kib, params.t, params.p, Some(KEY_LEN))
        .map_err(|e| Error::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut master = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut master[..])
        .map_err(|e| Error::Kdf(e.to_string()))?;
    Ok(AccountKeys {
        auth_key: hkdf_key(&master[..], b"dotvault/v1/auth")?,
        enc_key: hkdf_key(&master[..], b"dotvault/v1/enc")?,
    })
}

fn hkdf_key(ikm: &[u8], info: &[u8]) -> Result<Key> {
    let hk = Hkdf::<Sha256>::new(None, ikm);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(info, &mut out[..])
        .map_err(|e| Error::Kdf(e.to_string()))?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// AEAD
// ---------------------------------------------------------------------------

fn full_aad(aad: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(aad.len() + 1);
    v.push(SEAL_VERSION);
    v.extend_from_slice(aad);
    v
}

/// Encrypts `plaintext` under `key`, binding it to `aad`.
pub fn seal(key: &[u8; KEY_LEN], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| Error::Encrypt)?;
    let nonce_bytes = random_bytes::<NONCE_LEN>()?;
    let nonce = XNonce::try_from(&nonce_bytes[..]).map_err(|_| Error::Encrypt)?;
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &full_aad(aad),
            },
        )
        .map_err(|_| Error::Encrypt)?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
    out.push(SEAL_VERSION);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Reverses [`seal`]. Fails if the key, the `aad` or any byte differs.
pub fn open(key: &[u8; KEY_LEN], sealed: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if sealed.len() < 1 + NONCE_LEN + TAG_LEN {
        return Err(Error::Decrypt);
    }
    if sealed[0] != SEAL_VERSION {
        return Err(Error::Version(sealed[0]));
    }
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| Error::Decrypt)?;
    let nonce = XNonce::try_from(&sealed[1..1 + NONCE_LEN]).map_err(|_| Error::Decrypt)?;
    let pt = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &sealed[1 + NONCE_LEN..],
                aad: &full_aad(aad),
            },
        )
        .map_err(|_| Error::Decrypt)?;
    Ok(Zeroizing::new(pt))
}

pub fn seal_json<T: Serialize>(key: &[u8; KEY_LEN], value: &T, aad: &[u8]) -> Result<Vec<u8>> {
    let json = Zeroizing::new(serde_json::to_vec(value).map_err(|_| Error::Encrypt)?);
    seal(key, &json, aad)
}

pub fn open_json<T: DeserializeOwned>(key: &[u8; KEY_LEN], sealed: &[u8], aad: &[u8]) -> Result<T> {
    let pt = open(key, sealed, aad)?;
    serde_json::from_slice(&pt).map_err(|e| Error::Encoding(e.to_string()))
}

fn wrap_key(wrapping: &[u8; KEY_LEN], key: &[u8; KEY_LEN], context: &[u8]) -> Result<Vec<u8>> {
    seal(wrapping, key, context)
}

fn unwrap_key(wrapping: &[u8; KEY_LEN], wrapped: &[u8], context: &[u8]) -> Result<Key> {
    let pt = open(wrapping, wrapped, context)?;
    let key: [u8; KEY_LEN] = pt.as_slice().try_into().map_err(|_| Error::Decrypt)?;
    Ok(Zeroizing::new(key))
}

// ---------------------------------------------------------------------------
// Associated data for stored objects
// ---------------------------------------------------------------------------

/// Binds a vault item payload to its id and version, so the server cannot
/// swap or roll back ciphertexts between items.
pub fn aad_item_payload(item_id: &str, version: u32) -> Vec<u8> {
    format!("dotvault/v1/item/{item_id}/v{version}").into_bytes()
}

pub fn aad_item_name(item_id: &str) -> Vec<u8> {
    format!("dotvault/v1/item-name/{item_id}").into_bytes()
}

/// Shares use a fresh key each, so the context is constant.
pub const AAD_SHARE: &[u8] = b"dotvault/v1/share";

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------

/// Everything the server stores about how to unlock the vault with a password.
pub struct PasswordMaterial {
    pub kdf_salt: [u8; SALT_LEN],
    pub kdf_params: KdfParams,
    pub auth_key: Key,
    pub wrapped_vault_key: Vec<u8>,
}

pub struct NewAccount {
    pub password: PasswordMaterial,
    pub recovery_auth_key: Key,
    pub wrapped_vault_key_recovery: Vec<u8>,
    pub vault_key: Key,
    /// Show to the user exactly once, see [`format_recovery_key`].
    pub recovery_key: Key,
}

pub fn create_account(password: &str, params: KdfParams) -> Result<NewAccount> {
    let vault_key = random_key()?;
    let recovery_key = random_key()?;
    let password_material = rewrap_for_password(password, params, &vault_key)?;
    let recovery = derive_recovery_keys(&recovery_key)?;
    Ok(NewAccount {
        password: password_material,
        recovery_auth_key: recovery.auth_key,
        wrapped_vault_key_recovery: wrap_key(&recovery.enc_key, &vault_key, CTX_WRAP_RECOVERY)?,
        vault_key,
        recovery_key,
    })
}

/// Wraps an existing vault key under a (new) password with a fresh salt.
/// Used for registration, password change and recovery.
pub fn rewrap_for_password(
    password: &str,
    params: KdfParams,
    vault_key: &[u8; KEY_LEN],
) -> Result<PasswordMaterial> {
    let kdf_salt = random_bytes::<SALT_LEN>()?;
    let keys = derive_account_keys(password, &kdf_salt, &params)?;
    Ok(PasswordMaterial {
        kdf_salt,
        kdf_params: params,
        wrapped_vault_key: wrap_key(&keys.enc_key, vault_key, CTX_WRAP_PASSWORD)?,
        auth_key: keys.auth_key,
    })
}

/// Login step 2: unwrap the vault key the server returned.
pub fn unlock_with_password(keys: &AccountKeys, wrapped_vault_key: &[u8]) -> Result<Key> {
    unwrap_key(&keys.enc_key, wrapped_vault_key, CTX_WRAP_PASSWORD)
}

pub struct RecoveryKeys {
    pub auth_key: Key,
    pub enc_key: Key,
}

pub fn derive_recovery_keys(recovery_key: &[u8; KEY_LEN]) -> Result<RecoveryKeys> {
    Ok(RecoveryKeys {
        auth_key: hkdf_key(recovery_key, b"dotvault/v1/recovery-auth")?,
        enc_key: hkdf_key(recovery_key, b"dotvault/v1/recovery-enc")?,
    })
}

pub fn unlock_with_recovery(keys: &RecoveryKeys, wrapped_vault_key_recovery: &[u8]) -> Result<Key> {
    unwrap_key(&keys.enc_key, wrapped_vault_key_recovery, CTX_WRAP_RECOVERY)
}

/// `DVR1-ABCD-EFGH-...`: base32 in groups of four so it can be written down.
pub fn format_recovery_key(key: &[u8; KEY_LEN]) -> String {
    let b32 = BASE32_NOPAD.encode(key);
    let groups: Vec<&str> = b32
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("base32 is ascii"))
        .collect();
    format!("{RECOVERY_PREFIX}-{}", groups.join("-"))
}

pub fn parse_recovery_key(input: &str) -> Result<Key> {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    let body = cleaned
        .strip_prefix(RECOVERY_PREFIX)
        .ok_or(Error::RecoveryKey)?;
    let bytes = BASE32_NOPAD
        .decode(body.as_bytes())
        .map_err(|_| Error::RecoveryKey)?;
    let key: [u8; KEY_LEN] = bytes.try_into().map_err(|_| Error::RecoveryKey)?;
    Ok(Zeroizing::new(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smallest parameters `validate` accepts, to keep tests quick.
    const FAST: KdfParams = KdfParams {
        m_kib: 19 * 1024,
        t: 2,
        p: 1,
    };

    fn key(b: u8) -> [u8; KEY_LEN] {
        [b; KEY_LEN]
    }

    #[test]
    fn seal_open_roundtrip() {
        let sealed = seal(&key(7), b"DB_URL=postgres://x", b"ctx").unwrap();
        assert_eq!(sealed[0], SEAL_VERSION);
        assert_eq!(sealed.len(), 1 + NONCE_LEN + 19 + TAG_LEN);
        let opened = open(&key(7), &sealed, b"ctx").unwrap();
        assert_eq!(&opened[..], b"DB_URL=postgres://x");
    }

    #[test]
    fn seal_is_randomized() {
        let a = seal(&key(1), b"same", b"").unwrap();
        let b = seal(&key(1), b"same", b"").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn open_rejects_wrong_key_aad_and_tampering() {
        let sealed = seal(&key(1), b"secret", b"aad").unwrap();
        assert_eq!(open(&key(2), &sealed, b"aad").unwrap_err(), Error::Decrypt);
        assert_eq!(
            open(&key(1), &sealed, b"other").unwrap_err(),
            Error::Decrypt
        );
        for i in 0..sealed.len() {
            let mut bad = sealed.clone();
            bad[i] ^= 1;
            assert!(open(&key(1), &bad, b"aad").is_err(), "flip at byte {i}");
        }
        assert_eq!(
            open(&key(1), &sealed[..10], b"aad").unwrap_err(),
            Error::Decrypt
        );
    }

    #[test]
    fn open_rejects_unknown_version() {
        let mut sealed = seal(&key(1), b"x", b"").unwrap();
        sealed[0] = 9;
        assert_eq!(open(&key(1), &sealed, b"").unwrap_err(), Error::Version(9));
    }

    #[test]
    fn item_aad_binds_id_and_version() {
        let sealed = seal(&key(1), b"p", &aad_item_payload("abc", 1)).unwrap();
        assert!(open(&key(1), &sealed, &aad_item_payload("abc", 1)).is_ok());
        assert!(open(&key(1), &sealed, &aad_item_payload("abc", 2)).is_err());
        assert!(open(&key(1), &sealed, &aad_item_payload("abd", 1)).is_err());
    }

    #[test]
    fn json_roundtrip() {
        let sealed = seal_json(&key(3), &vec!["a", "b"], b"j").unwrap();
        let back: Vec<String> = open_json(&key(3), &sealed, b"j").unwrap();
        assert_eq!(back, vec!["a", "b"]);
    }

    #[test]
    fn kdf_is_deterministic_and_separates_keys() {
        let salt = [5u8; SALT_LEN];
        let a = derive_account_keys("hunter2", &salt, &FAST).unwrap();
        let b = derive_account_keys("hunter2", &salt, &FAST).unwrap();
        let c = derive_account_keys("hunter3", &salt, &FAST).unwrap();
        assert_eq!(*a.auth_key, *b.auth_key);
        assert_eq!(*a.enc_key, *b.enc_key);
        assert_ne!(*a.auth_key, *c.auth_key);
        assert_ne!(*a.auth_key, *a.enc_key);
        let other_salt = derive_account_keys("hunter2", &[6u8; SALT_LEN], &FAST).unwrap();
        assert_ne!(*a.auth_key, *other_salt.auth_key);
    }

    /// Pins the derivation: if this changes, every existing account becomes
    /// impossible to unlock. Bump the format version instead of editing it.
    /// Expected values were cross-checked against argon2-cffi (reference
    /// libargon2) and an RFC 5869 HKDF written with Python's `hmac`.
    #[test]
    fn kdf_output_is_pinned() {
        use data_encoding::HEXLOWER;
        let keys =
            derive_account_keys("correct horse battery staple", &[7u8; SALT_LEN], &FAST).unwrap();
        assert_eq!(
            HEXLOWER.encode(&keys.auth_key[..]),
            "a1f9b05ae6d5d882240c973961d5bdc7c721294b65611c33215744d3fad1c52f"
        );
        assert_eq!(
            HEXLOWER.encode(&keys.enc_key[..]),
            "1399d4656382d32be1991ef7b615ddff3d4e134c3cd2de8976e24be5a8cf2008"
        );
    }

    #[test]
    fn recovery_derivation_is_pinned() {
        use data_encoding::HEXLOWER;
        let rk = derive_recovery_keys(&[9u8; KEY_LEN]).unwrap();
        assert_eq!(
            HEXLOWER.encode(&rk.auth_key[..]),
            "6cede341fb8f17dc96e6a424148eb762ed5b83a242ae7c09a77df98d84991679"
        );
        assert_eq!(
            HEXLOWER.encode(&rk.enc_key[..]),
            "5df9d25d680a17fcc151c38d87b4e96d6c7fb66e92296b194780783e5cb44923"
        );
    }

    #[test]
    fn kdf_params_are_bounded() {
        let salt = [0u8; SALT_LEN];
        for bad in [
            KdfParams {
                m_kib: 1024,
                ..FAST
            },
            KdfParams { t: 1, ..FAST },
            KdfParams { p: 0, ..FAST },
            KdfParams {
                m_kib: 4 * 1024 * 1024,
                ..FAST
            },
            KdfParams { t: 1000, ..FAST },
        ] {
            assert!(derive_account_keys("pw", &salt, &bad).is_err(), "{bad:?}");
        }
        assert!(KdfParams::default().validate().is_ok());
    }

    #[test]
    fn account_lifecycle() {
        let acct = create_account("correct horse", FAST).unwrap();

        // login: derive from password + server-held salt, unwrap
        let keys = derive_account_keys(
            "correct horse",
            &acct.password.kdf_salt,
            &acct.password.kdf_params,
        )
        .unwrap();
        assert_eq!(*keys.auth_key, *acct.password.auth_key);
        let vk = unlock_with_password(&keys, &acct.password.wrapped_vault_key).unwrap();
        assert_eq!(*vk, *acct.vault_key);

        // wrong password cannot unwrap
        let wrong = derive_account_keys("wrong", &acct.password.kdf_salt, &FAST).unwrap();
        assert!(unlock_with_password(&wrong, &acct.password.wrapped_vault_key).is_err());

        // recovery path yields the same vault key
        let shown = format_recovery_key(&acct.recovery_key);
        let parsed = parse_recovery_key(&shown).unwrap();
        let rk = derive_recovery_keys(&parsed).unwrap();
        assert_eq!(*rk.auth_key, *acct.recovery_auth_key);
        let vk2 = unlock_with_recovery(&rk, &acct.wrapped_vault_key_recovery).unwrap();
        assert_eq!(*vk2, *acct.vault_key);

        // password change keeps the vault key and invalidates the old password
        let changed = rewrap_for_password("new password", FAST, &vk).unwrap();
        let new_keys =
            derive_account_keys("new password", &changed.kdf_salt, &changed.kdf_params).unwrap();
        let vk3 = unlock_with_password(&new_keys, &changed.wrapped_vault_key).unwrap();
        assert_eq!(*vk3, *acct.vault_key);
        assert_ne!(changed.kdf_salt, acct.password.kdf_salt);
        assert!(unlock_with_password(&keys, &changed.wrapped_vault_key).is_err());
    }

    #[test]
    fn password_and_recovery_wraps_are_not_interchangeable() {
        let acct = create_account("pw", FAST).unwrap();
        let rk = derive_recovery_keys(&acct.recovery_key).unwrap();
        // same ciphertext, wrong context
        assert!(unwrap_key(
            &rk.enc_key,
            &acct.wrapped_vault_key_recovery,
            CTX_WRAP_PASSWORD
        )
        .is_err());
    }

    #[test]
    fn recovery_key_format() {
        let k = [0xABu8; KEY_LEN];
        let s = format_recovery_key(&k);
        assert!(s.starts_with("DVR1-"));
        assert_eq!(s.matches('-').count(), 13);
        assert_eq!(*parse_recovery_key(&s).unwrap(), k);
        // tolerant of case, spacing and missing dashes
        let sloppy = s.to_lowercase().replace('-', " ");
        assert_eq!(*parse_recovery_key(&sloppy).unwrap(), k);
        assert!(parse_recovery_key("DVR1-AAAA").is_err());
        assert!(parse_recovery_key("XXXX-AAAA").is_err());
        assert!(parse_recovery_key(&format!("DVR1-1{}", &s[6..])).is_err());
    }
}
