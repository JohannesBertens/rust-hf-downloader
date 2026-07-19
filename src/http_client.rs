//! Shared HTTP client and authenticated GET helper.
//!
//! A single pooled `reqwest::Client` is reused across all requests so that
//! HTTP keep-alive and TLS session state survive between calls (the reqwest
//! docs recommend creating one client and reusing it). Authentication is
//! attached per-request via `bearer_auth`, so the same pooled client serves
//! both authenticated and anonymous requests.

use once_cell::sync::Lazy;
use reqwest::{header, Client};
use std::time::Duration;

/// Process-wide pooled HTTP client (connection keep-alive + TLS reuse).
/// Carries no default headers or timeout; auth and timeouts are set per-request.
static SHARED_CLIENT: Lazy<Client> =
    Lazy::new(|| Client::builder().build().expect("failed to build reqwest::Client"));

/// Borrow the shared pooled HTTP client.
pub fn shared_client() -> &'static Client {
    &SHARED_CLIENT
}

/// Build an HTTP client with an optional token baked into default headers.
///
/// Kept for the download stream, which needs a per-download timeout. The API
/// path uses [`get_with_optional_token`] on the shared client instead.
pub fn build_client_with_token(
    token: Option<&String>,
    timeout: Option<Duration>,
) -> Result<Client, reqwest::Error> {
    let mut builder = Client::builder();

    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }

    // ONLY add authorization header if token is provided and non-empty
    if let Some(token) = token {
        if !token.is_empty() {
            let mut headers = header::HeaderMap::new();
            let auth_value = format!("Bearer {}", token);
            if let Ok(header_val) = header::HeaderValue::from_str(&auth_value) {
                headers.insert(header::AUTHORIZATION, header_val);
            }
            builder = builder.default_headers(headers);
        }
    }

    builder.build()
}

/// Make a GET request with an optional bearer token, reusing the shared
/// pooled client. If `token` is `None` or empty, the request is anonymous.
pub async fn get_with_optional_token(
    url: &str,
    token: Option<&String>,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut request = SHARED_CLIENT.get(url);
    if let Some(token) = token {
        if !token.is_empty() {
            request = request.bearer_auth(token.as_str());
        }
    }
    request.send().await
}
