//! Authenticated HTTP request helpers: construction of the ONE
//! `reqwest::Client` a run shares across all its HuggingFace API
//! requests, plus the GET helper every such request goes through.
//!
//! One client per run (plan M4/B5): the client is built once at the
//! frontend's bootstrap — the CLI's `run::load_run_config` tail (Runner),
//! the TUI's `App::new` — with the resolved token installed as the
//! client's default `Authorization` header. Every API call threads
//! `&Client` through [`get_with_optional_token`]; nothing builds a
//! per-request client anymore (one TLS session pool per run instead of
//! one TLS handshake per tree directory).
//!
//! The download transport keeps its own per-download client
//! (`download_timeout_secs` applies to the byte stream, not to metadata
//! lookups) built by the same [`build_client_with_token`] — and is
//! therefore covered by the same invalid-token contract: a token that
//! cannot be represented in a header value is an explicit
//! [`ClientBuildError::InvalidToken`], never a silent downgrade to an
//! unauthenticated request that later surfaces as a confusing 401.

use reqwest::{header, Client};
use std::time::Duration;

/// Why a run's shared [`Client`] could not be built.
///
/// `InvalidToken` is the M4/B5 fix: a malformed token used to be silently
/// dropped from the default headers (`if let Ok(..) = HeaderValue::from_str`),
/// sending every request of the run unauthenticated — gated repos then
/// answered a confusing 401/`auth_required` that pointed users at
/// `--token` instead of at their malformed token. It is now an explicit
/// error the callers surface.
#[derive(Debug)]
pub enum ClientBuildError {
    /// `Client::builder().build()` itself failed (TLS backend init).
    Build(reqwest::Error),
    /// The configured token contains bytes that cannot appear in a
    /// header value (control characters other than horizontal tab, or
    /// DEL). The token is never echoed — it is a secret.
    InvalidToken,
}

impl std::fmt::Display for ClientBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientBuildError::Build(e) => write!(f, "failed to build HTTP client: {e}"),
            ClientBuildError::InvalidToken => write!(
                f,
                "the configured HF token cannot be used in an Authorization header \
                 (it contains characters that cannot appear in a header value, e.g. a \
                 control character)"
            ),
        }
    }
}

impl std::error::Error for ClientBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientBuildError::Build(e) => Some(e),
            ClientBuildError::InvalidToken => None,
        }
    }
}

/// Build the HTTP client a run shares, with an optional HuggingFace
/// token installed as the default `Authorization: Bearer <token>`
/// header.
///
/// Token semantics (unchanged since v0.9.5): `None` or `""` builds an
/// unauthenticated client — requests carry no `Authorization` header at
/// all. A non-empty token MUST be representable as a header value;
/// otherwise [`ClientBuildError::InvalidToken`] is returned (see the
/// module docs: previously the token was silently dropped).
pub fn build_client_with_token(
    token: Option<&str>,
    timeout: Option<Duration>,
) -> Result<Client, ClientBuildError> {
    let mut builder = Client::builder();

    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }

    // ONLY add authorization header if token is provided and non-empty
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        let auth_value = format!("Bearer {token}");
        let header_val = header::HeaderValue::from_str(&auth_value)
            .map_err(|_| ClientBuildError::InvalidToken)?;
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, header_val);
        builder = builder.default_headers(headers);
    }

    builder.build().map_err(ClientBuildError::Build)
}

/// GET `url` through the run's shared client — a thin wrapper keeping
/// the historical entry point name; the optional-token semantics live in
/// the client's default headers (see [`build_client_with_token`]).
pub async fn get_with_optional_token(
    client: &Client,
    url: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    client.get(url).send().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_empty_and_valid_tokens_build_a_client() {
        assert!(build_client_with_token(None, None).is_ok());
        assert!(build_client_with_token(Some(""), None).is_ok());
        assert!(build_client_with_token(Some("hf_xyz"), None).is_ok());
        // The timeout knob is orthogonal to the token axis.
        assert!(build_client_with_token(Some("hf_xyz"), Some(Duration::from_secs(1))).is_ok());
    }

    #[test]
    fn invalid_token_is_an_explicit_error_not_a_silent_downgrade() {
        // `HeaderValue::from_str` rejects control characters (other than
        // horizontal tab) and DEL. Pre-B5 these tokens built an
        // UNAUTHENTICATED client (the header was silently dropped) — the
        // confusing-401 root cause this pins shut.
        for bad in [
            "bad\ntoken",
            "bad\rtoken",
            "bad\u{0}token",
            "bad\u{7}token",
            "bad\u{7f}token",
        ] {
            assert!(
                matches!(
                    build_client_with_token(Some(bad), None),
                    Err(ClientBuildError::InvalidToken)
                ),
                "token {bad:?} must fail explicitly"
            );
        }
        // Whitespace-only padding is representable (HeaderValue allows
        // visible ASCII + tab, and treats high bytes as obs-text) — pinned
        // so the reject set stays exact.
        assert!(build_client_with_token(Some(" padded "), None).is_ok());
        assert!(build_client_with_token(Some("tökén"), None).is_ok());
    }
}
