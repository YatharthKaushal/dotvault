//! The page a share link opens in a browser.
//!
//! It is static and identical for every id (it neither checks nor reveals
//! whether a share exists). Its script reads the key from `location.hash`,
//! which browsers never send to a server, and the CSP forbids the page from
//! making any network request, so the key cannot leave the tab.

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
};

use crate::AppState;

const PAGE: &str = include_str!("../../assets/share.html");
const SCRIPT: &str = include_str!("../../assets/share.js");
const SERVER_NOTE_MARKER: &str = "<!--SERVER_NOTE-->";

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                   img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The landing page, mentioning `public_url` when the operator configured one.
pub fn render_page(public_url: Option<&str>) -> String {
    let note = match public_url {
        Some(url) => format!(
            "<p class=\"muted\">This dotvault server: <code>{}</code></p>",
            escape_html(url)
        ),
        None => String::new(),
    };
    PAGE.replace(SERVER_NOTE_MARKER, &note)
}

fn hardened(mut resp: Response, cache_control: &'static str) -> Response {
    let h = resp.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    resp
}

/// `GET /s/{id}`: the same page for every id.
pub async fn page(State(state): State<AppState>, Path(_id): Path<String>) -> Response {
    let html = render_page(state.config.public_url.as_deref());
    hardened(
        (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            )],
            html,
        )
            .into_response(),
        "no-store",
    )
}

/// `GET /assets/share.js`
pub async fn script() -> Response {
    hardened(
        (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript; charset=utf-8"),
            )],
            SCRIPT,
        )
            .into_response(),
        "public, max-age=300",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_metacharacters() {
        assert_eq!(
            escape_html(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
        assert_eq!(
            escape_html("https://v.example.com"),
            "https://v.example.com"
        );
    }

    #[test]
    fn page_is_self_contained() {
        let html = render_page(None);
        assert!(!html.contains(SERVER_NOTE_MARKER));
        // no external resources: every URL-bearing attribute is local
        for needle in ["http://", "https://"] {
            for (i, _) in html.match_indices(needle) {
                let around = &html[i.saturating_sub(12)..i];
                assert!(
                    !around.contains("src=") && !around.contains("href="),
                    "external resource reference near {:?}",
                    &html[i.saturating_sub(12)..(i + 30).min(html.len())]
                );
            }
        }
        assert!(html.contains("/assets/share.js"));
        assert!(!SCRIPT.contains("fetch(") && !SCRIPT.contains("XMLHttpRequest"));
    }

    #[test]
    fn public_url_is_escaped_into_the_page() {
        let html = render_page(Some("https://x.test/\"><script>alert(1)</script>"));
        assert!(!html.contains("<script>alert(1)"));
        assert!(html.contains("&lt;script&gt;alert(1)"));
    }
}
