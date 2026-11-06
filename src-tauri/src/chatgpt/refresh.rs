//! Token refresh. The refresh token rotates: each refresh returns a replacement, which must be
//! saved together with the new access token, expiry and scopes. Callers serialize refreshes.

use reqwest::Client;

use super::errors::Result;
use super::oauth::{Discovery, RESOURCE, TokenGrant, parse_token_response, token_request};
use super::secret::Secret;

/// Refresh this long before the access token expires.
pub const REFRESH_MARGIN_MS: i64 = 60_000;

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// The access token is usable as is.
    Fresh,
    /// Refresh now.
    Refresh,
    /// Expired, but `earliest_refresh_at` hasn't been reached yet.
    NotReady,
}

pub fn decide(expires_at: Option<i64>, earliest_refresh_at: Option<i64>, now: i64) -> Decision {
    let expires_at = expires_at.unwrap_or(0);
    if expires_at > now + REFRESH_MARGIN_MS {
        Decision::Fresh
    } else if earliest_refresh_at.is_none_or(|earliest| earliest <= now) {
        Decision::Refresh
    } else if expires_at > now {
        Decision::Fresh
    } else {
        Decision::NotReady
    }
}

/// `grant_type=refresh_token` with the issued client ID. `scope` is omitted; if the response
/// omits it too, the previous scopes are kept.
pub async fn refresh_tokens(
    http: &Client,
    discovery: &Discovery,
    client_id: &str,
    refresh: &Secret,
    previous_scopes: &[String],
    now_ms: i64,
) -> Result<TokenGrant> {
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh.expose()),
        ("resource", RESOURCE),
    ];
    let data = token_request(http, &discovery.token_endpoint, &form).await?;
    parse_token_response(&data, Some(previous_scopes), true, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000_000;

    #[test]
    fn decisions() {
        assert_eq!(decide(Some(NOW + 10 * 60_000), None, NOW), Decision::Fresh);
        assert_eq!(decide(Some(NOW + 30_000), None, NOW), Decision::Refresh);
        assert_eq!(decide(Some(NOW - 1), Some(NOW - 1), NOW), Decision::Refresh);
        assert_eq!(
            decide(Some(NOW + 30_000), Some(NOW + 20_000), NOW),
            Decision::Fresh
        );
        assert_eq!(
            decide(Some(NOW - 1), Some(NOW + 20_000), NOW),
            Decision::NotReady
        );
        assert_eq!(decide(None, None, NOW), Decision::Refresh);
    }
}
