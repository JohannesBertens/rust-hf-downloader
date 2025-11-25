use reqwest::{Client, header};
use std::time::Duration;

/// Build an HTTP client with optional token
pub fn build_client_with_token(
    token: Option<&String>,
    timeout: Option<Duration>
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

/// Make a GET request with optional token
/// If token is None or empty string, makes unauthenticated request
pub async fn get_with_optional_token(
    url: &str, 
    token: Option<&String>
) -> Result<reqwest::Response, reqwest::Error> {
    // Check if token is provided AND non-empty
    let has_token = token.is_some_and(|t| !t.is_empty());
    
    if has_token {
        // Build client with token
        let client = build_client_with_token(token, None)?;
        client.get(url).send().await
    } else {
        // Use simple reqwest::get (no client needed)
        reqwest::get(url).await
    }
}
