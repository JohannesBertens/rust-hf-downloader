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
//! cannot be represented in a header value is DROPPED WITH AN EXPLICIT
//! [`TokenDroppedWarning`] (owner revision 2026-10-07 of the M4/B5 fix:
//! warn + proceed unauthenticated — never a silent downgrade, and never
//! a hard bootstrap failure either), so a later 401 is never mysterious.

use reqwest::{header, Client};
use std::time::Duration;

/// Why a run's shared [`Client`] could not be built. (The invalid-token
/// case is NOT an error anymore: the builder drops the token, proceeds
/// unauthenticated, and returns a [`TokenDroppedWarning`] — see the
/// module docs.)
#[derive(Debug)]
pub enum ClientBuildError {
    /// `Client::builder().build()` itself failed (TLS backend init).
    Build(reqwest::Error),
}

impl std::fmt::Display for ClientBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientBuildError::Build(e) => write!(f, "failed to build HTTP client: {e}"),
        }
    }
}

impl std::error::Error for ClientBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientBuildError::Build(e) => Some(e),
        }
    }
}

/// The configured HF token could not be represented in a header value
/// (control characters other than horizontal tab, or DEL) and was
/// dropped: the client runs **unauthenticated**. The token itself is
/// never echoed — it is a secret.
///
/// Owner revision 2026-10-07 (B5 revisited): this is a WARNING the
/// frontends surface, not a run-fatal error — the pre-M4 behavior was a
/// *silent* drop that surfaced as a confusing 401 blaming `--token`;
/// M4/B5 first made it a hard `auth_required` error; the owner chose
/// the middle ground: loud warning, unauthenticated fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenDroppedWarning;

impl TokenDroppedWarning {
    /// The user-facing message (surfaces as a `Warning:` line / warning
    /// event; includes the fix path so a later 401 is diagnosable).
    pub fn message(&self) -> String {
        "the configured HF token cannot be used in an Authorization header \
         (it contains characters that cannot appear in a header value, e.g. a \
         control character) — requests will be sent WITHOUT authentication; \
         fix or remove the token (--token, $HF_TOKEN, or the config file)"
            .to_string()
    }
}

impl std::fmt::Display for TokenDroppedWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

/// Build the HTTP client a run shares, with an optional HuggingFace
/// token installed as the default `Authorization: Bearer <token>`
/// header.
///
/// Token semantics (unchanged since v0.9.5): `None` or `""` builds an
/// unauthenticated client — requests carry no `Authorization` header at
/// all. A non-empty token that CANNOT be represented in a header value is
/// dropped with an explicit [`TokenDroppedWarning`] and the client
/// proceeds unauthenticated (owner revision 2026-10-07; previously —
/// pre-M4 — the drop was silent).
pub fn build_client_with_token(
    token: Option<&str>,
    timeout: Option<Duration>,
) -> Result<(Client, Option<TokenDroppedWarning>), ClientBuildError> {
    let mut builder = Client::builder();

    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }

    // ONLY add authorization header if token is provided, non-empty, AND
    // representable — a malformed token is dropped with the warning above.
    let mut token_warning = None;
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        let auth_value = format!("Bearer {token}");
        match header::HeaderValue::from_str(&auth_value) {
            Ok(header_val) => {
                let mut headers = header::HeaderMap::new();
                headers.insert(header::AUTHORIZATION, header_val);
                builder = builder.default_headers(headers);
            }
            Err(_) => token_warning = Some(TokenDroppedWarning),
        }
    }

    Ok((
        builder.build().map_err(ClientBuildError::Build)?,
        token_warning,
    ))
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
    fn invalid_token_builds_unauthenticated_client_with_warning() {
        // `HeaderValue::from_str` rejects control characters (other than
        // horizontal tab) and DEL. Pre-M4 these tokens built an
        // UNAUTHENTICATED client with the header SILENTLY dropped — the
        // confusing-401 root cause. M4/B5 first made it a hard error;
        // owner revision 2026-10-07: explicit warning + unauthenticated
        // fallback (never silent, never run-fatal).
        for bad in [
            "bad\ntoken",
            "bad\rtoken",
            "bad\u{0}token",
            "bad\u{7}token",
            "bad\u{7f}token",
        ] {
            let (client, warning) = build_client_with_token(Some(bad), None)
                .expect("bad token must not fail the build");
            let warning =
                warning.unwrap_or_else(|| panic!("token {bad:?} must carry the drop warning"));
            assert!(
                warning.message().contains("WITHOUT authentication"),
                "warning must state the downgrade: {}",
                warning.message()
            );
            assert!(
                warning.message().contains("--token"),
                "warning must name the fix path"
            );
            // The client is usable (unauthenticated) — NO default
            // Authorization header. NOTE: checked via the Client's Debug
            // representation, NOT RequestBuilder::build(): reqwest only
            // merges default_headers during execute_request, so a
            // built-but-unsent Request's header map is empty for ANY
            // client and would make this assertion vacuous (Gemini gate
            // P1, 2026-10-07). Client's Debug prints default_headers
            // (verified in reqwest 0.11.27 async_impl/client.rs
            // fmt_fields), so the substring check is load-bearing —
            // the control below proves it can fail.
            assert!(
                !format!("{client:?}").contains("authorization"),
                "dropped-token client must have no default Authorization: {client:?}"
            );
        }
        // Whitespace-only padding is representable (HeaderValue allows
        // visible ASCII + tab, and treats high bytes as obs-text) — pinned
        // so the reject set stays exact: no warning for these.
        assert!(build_client_with_token(Some(" padded "), None)
            .expect("representable")
            .1
            .is_none());
        assert!(build_client_with_token(Some("tökén"), None)
            .expect("representable")
            .1
            .is_none());
        assert!(build_client_with_token(None, None)
            .expect("anon")
            .1
            .is_none());
        assert!(build_client_with_token(Some(""), None)
            .expect("empty")
            .1
            .is_none());
        // Control for the Debug-substring assertion above: a client built
        // with a VALID token DOES carry the default Authorization header
        // in its Debug representation — proving the substring check is
        // load-bearing (it fails when the header survives).
        let (tokened, none_warning) =
            build_client_with_token(Some("hf_ok"), None).expect("valid token builds");
        assert!(none_warning.is_none());
        assert!(
            format!("{tokened:?}").contains("authorization"),
            "tokened client must carry the default Authorization: {tokened:?}"
        );
    }
}
