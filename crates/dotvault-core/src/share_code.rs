//! Share links and codes.
//!
//! * link: `https://host/s/<id>#<key>` (the key is in the fragment, which
//!   browsers never send to the server)
//! * code: `dv1.<id>.<key>`
//!
//! `id` is 16 random bytes and `key` is 32 random bytes, both base64url.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zeroize::Zeroizing;

use crate::crypto::{random_bytes, Key, KEY_LEN};
use crate::error::{Error, Result};

const ID_LEN: usize = 16;
const CODE_PREFIX: &str = "dv1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRef {
    /// `scheme://host[:port]` when parsed from a link, `None` for a bare code.
    pub origin: Option<String>,
    pub id: String,
    pub key: Key,
}

pub fn new_id() -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<ID_LEN>()?))
}

pub fn encode_code(id: &str, key: &[u8; KEY_LEN]) -> String {
    format!("{CODE_PREFIX}.{id}.{}", URL_SAFE_NO_PAD.encode(key))
}

pub fn encode_link(origin: &str, id: &str, key: &[u8; KEY_LEN]) -> String {
    format!(
        "{}/s/{id}#{}",
        origin.trim_end_matches('/'),
        URL_SAFE_NO_PAD.encode(key)
    )
}

/// Parses either a link or a code.
pub fn parse(input: &str) -> Result<ShareRef> {
    let input = input.trim();
    if input.starts_with("http://") || input.starts_with("https://") {
        parse_link(input)
    } else {
        parse_code(input)
    }
}

fn bad(msg: &str) -> Error {
    Error::ShareCode(msg.to_string())
}

fn parse_code(input: &str) -> Result<ShareRef> {
    let mut parts = input.split('.');
    let (Some(prefix), Some(id), Some(key), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad("expected dv1.<id>.<key>"));
    };
    if prefix != CODE_PREFIX {
        return Err(bad("unknown code version"));
    }
    Ok(ShareRef {
        origin: None,
        id: check_id(id)?,
        key: decode_key(key)?,
    })
}

fn parse_link(input: &str) -> Result<ShareRef> {
    let (before_fragment, fragment) = input
        .split_once('#')
        .ok_or_else(|| bad("the link is missing its #key part"))?;
    let without_query = before_fragment
        .split_once('?')
        .map_or(before_fragment, |(a, _)| a);

    let scheme_end = without_query.find("://").expect("checked by caller") + 3;
    let rest = &without_query[scheme_end..];
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| bad("no share id in link"))?;
    if authority.is_empty() || authority.contains('@') || authority.contains(char::is_whitespace) {
        return Err(bad("invalid host in link"));
    }
    let id = path
        .trim_end_matches('/')
        .strip_prefix("s/")
        .ok_or_else(|| bad("link path must look like /s/<id>"))?;

    Ok(ShareRef {
        origin: Some(format!("{}{}", &without_query[..scheme_end], authority)),
        id: check_id(id)?,
        key: decode_key(fragment)?,
    })
}

fn check_id(id: &str) -> Result<String> {
    match URL_SAFE_NO_PAD.decode(id) {
        Ok(b) if b.len() == ID_LEN => Ok(id.to_string()),
        _ => Err(bad("malformed share id")),
    }
}

fn decode_key(s: &str) -> Result<Key> {
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| bad("malformed share key"))?;
    let key: [u8; KEY_LEN] = bytes.try_into().map_err(|_| bad("malformed share key"))?;
    Ok(Zeroizing::new(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (String, [u8; KEY_LEN]) {
        (new_id().unwrap(), [42u8; KEY_LEN])
    }

    #[test]
    fn code_roundtrip() {
        let (id, key) = sample();
        let code = encode_code(&id, &key);
        assert!(code.starts_with("dv1."));
        let r = parse(&code).unwrap();
        assert_eq!(r.id, id);
        assert_eq!(*r.key, key);
        assert_eq!(r.origin, None);
    }

    #[test]
    fn link_roundtrip() {
        let (id, key) = sample();
        let link = encode_link("https://vault.example.com/", &id, &key);
        assert!(link.starts_with("https://vault.example.com/s/"));
        let r = parse(&link).unwrap();
        assert_eq!(r.id, id);
        assert_eq!(*r.key, key);
        assert_eq!(r.origin.as_deref(), Some("https://vault.example.com"));
    }

    #[test]
    fn link_keeps_port_and_ignores_query_and_whitespace() {
        let (id, key) = sample();
        let link = encode_link("http://localhost:8080", &id, &key);
        let padded_with_query = format!("  {}  \n", link.replacen('#', "?utm=x#", 1));
        let r = parse(&padded_with_query).unwrap();
        assert_eq!(r.origin.as_deref(), Some("http://localhost:8080"));
        assert_eq!(r.id, id);
    }

    #[test]
    fn rejects_malformed_input() {
        let (id, key) = sample();
        let k = URL_SAFE_NO_PAD.encode(key);
        for bad in [
            "".to_string(),
            "garbage".to_string(),
            format!("dv2.{id}.{k}"),
            format!("dv1.{id}"),
            format!("dv1.{id}.{k}.extra"),
            format!("dv1.short.{k}"),
            format!("dv1.{id}.short"),
            format!("https://host/s/{id}"),       // no fragment
            format!("https://host/x/{id}#{k}"),   // wrong path
            format!("https://host/s/short#{k}"),  // bad id
            format!("https://host/s/{id}#short"), // bad key
            format!("https://trusted.com@evil.com/s/{id}#{k}"), // userinfo
            format!("https:///s/{id}#{k}"),       // empty host
        ] {
            assert!(parse(&bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn ids_are_unique_and_url_safe() {
        let a = new_id().unwrap();
        let b = new_id().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 22);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
