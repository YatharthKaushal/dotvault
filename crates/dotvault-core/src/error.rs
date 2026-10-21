use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid variable name: {0:?}")]
    InvalidKey(String),
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed (wrong key, wrong password or corrupted data)")]
    Decrypt,
    #[error("key derivation failed: {0}")]
    Kdf(String),
    #[error("could not read secure random bytes")]
    Random,
    #[error("invalid share link or code: {0}")]
    ShareCode(String),
    #[error("invalid recovery key")]
    RecoveryKey,
    #[error("unsupported data format version {0}")]
    Version(u8),
    #[error("invalid encoding: {0}")]
    Encoding(String),
}
