//! Shared building blocks for the dotvault CLI and server: a lossless `.env`
//! parser, env-file classification, end-to-end crypto, share codes and the
//! wire types of the HTTP API.

pub mod api;
pub mod classify;
pub mod crypto;
pub mod dotenv;
pub mod error;
pub mod fingerprint;
pub mod payload;
pub mod share_code;

pub use error::{Error, Result};
