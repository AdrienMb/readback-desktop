//! Typed errors with a user-facing message.
//!
//! Messages are written here, never copied from a server response: responses can echo submitted
//! input. Codes and request IDs are only kept when they look like plain identifiers. Nothing in
//! an `Error` can contain a credential, so it is safe to log and to send to the webview.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// What the UI should do about an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// A ChatGPT usage limit was reached. Primary action: Manage usage.
    UsageLimit,
    /// Signed in, but ChatGPT plan usage (`chatgpt.tokens.use.direct`) wasn't granted.
    SharingDisabled,
    /// Plan usage isn't available for this account, workspace or policy.
    NotEligible,
    /// The saved session can't be used; the user must sign in again.
    Reauth,
    /// ChatGPT couldn't be reached.
    Network,
    /// A temporary problem on ChatGPT's side; trying again later may work.
    Temporary,
    /// The user cancelled.
    Cancelled,
    /// Sign-in didn't complete (denied, wrong account, incomplete registration…).
    SignIn,
    /// ChatGPT rejected the request itself (model, input, unsupported option…).
    Request,
    /// Local storage (Keychain or app data) failed.
    Storage,
    /// Anything else. The message is still specific.
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Error {
    pub kind: ErrorKind,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Refresh errors after which the refresh token is unusable (Errors and recovery → Refresh).
const TERMINAL_REFRESH_CODES: &[&str] = &[
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];

impl Error {
    pub fn new(kind: ErrorKind, code: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: code.to_owned(),
            message: message.into(),
            retryable: matches!(kind, ErrorKind::Network | ErrorKind::Temporary),
            status: None,
            request_id: None,
        }
    }

    fn with_diagnostics(mut self, status: Option<u16>, request_id: Option<&str>) -> Self {
        self.status = status;
        self.request_id = request_id.and_then(safe_identifier).map(str::to_owned);
        self
    }

    pub fn network() -> Self {
        Self::new(
            ErrorKind::Network,
            "network_error",
            "Readback couldn't reach ChatGPT. Check your internet connection and try again.",
        )
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "cancelled", "Cancelled.")
    }

    pub fn sign_in_required() -> Self {
        Self::new(
            ErrorKind::Reauth,
            "sign_in_required",
            "Sign in with ChatGPT to continue.",
        )
    }

    pub fn session_expired() -> Self {
        Self::new(
            ErrorKind::Reauth,
            "session_expired",
            "Your ChatGPT session has expired. Sign in again to continue.",
        )
    }

    pub fn sharing_disabled() -> Self {
        Self::new(
            ErrorKind::SharingDisabled,
            "sharing_not_enabled",
            "You're signed in, but ChatGPT plan usage wasn't granted to Readback. \
             Sign in again and allow plan usage to continue.",
        )
    }

    pub fn invalid_id_token() -> Self {
        Self::new(
            ErrorKind::Reauth,
            "invalid_id_token",
            "ChatGPT's identity response couldn't be verified. Sign in again.",
        )
    }

    pub fn identity_unavailable() -> Self {
        Self::new(
            ErrorKind::Temporary,
            "identity_verification_unavailable",
            "ChatGPT identity verification is temporarily unavailable. Your connection is kept. \
             Try again shortly.",
        )
    }

    pub fn storage(code: &str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Storage, code, message)
    }

    pub fn is_terminal_refresh(&self) -> bool {
        TERMINAL_REFRESH_CODES.contains(&self.code.as_str())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for Error {}

/// Builds an error from an OAuth or Responses API error body.
///
/// OAuth errors use a string `error`; Responses errors use `error.code`; direct-admission
/// failures may only have a free-text `detail`, which is never shown.
pub fn api_error(body: Option<&Value>, status: Option<u16>, request_id: Option<&str>) -> Error {
    let mut detail = body;
    for _ in 0..4 {
        let next = detail.and_then(|value| {
            ["error", "detail"]
                .iter()
                .find_map(|key| value.get(*key).filter(|v| v.is_object()))
        });
        match next {
            Some(inner) => detail = Some(inner),
            None => break,
        }
    }
    let code = detail
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .or_else(|| value.get("code").and_then(Value::as_str))
        })
        .and_then(safe_identifier)
        .map(normalize_legacy_code)
        .unwrap_or("api_error");
    let (kind, message) = known_code(code).unwrap_or_else(|| by_status(status));
    Error::new(kind, code, message).with_diagnostics(status, request_id)
}

fn normalize_legacy_code(code: &str) -> &str {
    match code {
        "subscription_sharing_v2_user_not_eligible" => "subscription_sharing_user_not_eligible",
        "subscription_sharing_v2_route_not_supported" => "subscription_sharing_route_not_supported",
        "subscription_sharing_v2_invalid_user" => "subscription_sharing_invalid_user",
        "subscription_sharing_v2_user_unavailable" => "subscription_sharing_user_unavailable",
        other => other,
    }
}

fn known_code(code: &str) -> Option<(ErrorKind, &'static str)> {
    use ErrorKind::*;
    Some(match code {
        "subscription_sharing_usage_limit_exceeded" => (
            UsageLimit,
            "You've reached a ChatGPT usage limit. Review your plan or this app's limit in \
             ChatGPT settings.",
        ),
        "subscription_sharing_usage_unavailable" => (
            Temporary,
            "ChatGPT couldn't check your plan usage right now. Try again in a few minutes.",
        ),
        "subscription_sharing_user_not_eligible" => (
            NotEligible,
            "ChatGPT plan usage isn't available for this account or workspace.",
        ),
        "subscription_sharing_unsupported_capability" => (
            Request,
            "This request uses something ChatGPT plan usage doesn't support.",
        ),
        "subscription_sharing_route_not_supported" => (
            Request,
            "ChatGPT plan usage doesn't support this kind of request.",
        ),
        "subscription_sharing_invalid_user" => (
            Other,
            "ChatGPT couldn't validate this account's plan. Try again; if it keeps happening, \
             sign out and sign in again.",
        ),
        "subscription_sharing_user_unavailable" => (
            Temporary,
            "Your ChatGPT account or workspace is temporarily unavailable. Try again shortly.",
        ),
        "subscription_sharing_v2_client_not_enabled" => {
            (Other, "This app isn't enabled for ChatGPT plan usage.")
        }
        "chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context" => (
            Other,
            "The permissions ChatGPT granted to Readback don't allow this request.",
        ),
        "invalid_client" => (
            Other,
            "ChatGPT rejected Readback's saved app registration. Sign-in can't continue with it.",
        ),
        code if TERMINAL_REFRESH_CODES.contains(&code) => (
            Reauth,
            "Your ChatGPT session has expired. Sign in again to continue.",
        ),
        "invalid_token" | "invalid_api_key" => (
            Reauth,
            "ChatGPT didn't accept this connection. Sign in again to continue.",
        ),
        "access_denied" => (
            SignIn,
            "Sign-in was cancelled in the browser. You can try again when you're ready.",
        ),
        "model_not_found" => (
            Request,
            "This model isn't available for your ChatGPT account. Choose another model.",
        ),
        _ => return None,
    })
}

fn by_status(status: Option<u16>) -> (ErrorKind, &'static str) {
    use ErrorKind::*;
    match status {
        Some(400 | 422) => (
            Request,
            "ChatGPT rejected this request. Check the model and prompt.",
        ),
        Some(401) => (
            Reauth,
            "ChatGPT didn't accept this account's permissions. Sign in again to continue.",
        ),
        Some(403) => (
            Other,
            "A ChatGPT policy blocked this request, for example because of the serving region.",
        ),
        Some(429) => (
            Temporary,
            "ChatGPT is receiving too many requests. Wait a moment.",
        ),
        Some(status) if status >= 500 => (
            Temporary,
            "ChatGPT is temporarily unavailable. Try again shortly.",
        ),
        _ => (Other, "ChatGPT couldn't complete this request."),
    }
}

/// Keeps a value only if it looks like a plain identifier (no spaces, no free text).
pub fn safe_identifier(value: &str) -> Option<&str> {
    let mut chars = value.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let rest_ok = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '[' | ']' | '-'));
    (first_ok && rest_ok && value.len() <= 160).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_limit_from_responses_error() {
        let body = json!({ "error": { "code": "subscription_sharing_usage_limit_exceeded",
                                      "message": "limit for user@example.com" } });
        let error = api_error(Some(&body), Some(429), Some("req_123"));
        assert_eq!(error.kind, ErrorKind::UsageLimit);
        assert_eq!(error.code, "subscription_sharing_usage_limit_exceeded");
        assert!(!error.retryable);
        assert_eq!(error.request_id.as_deref(), Some("req_123"));
        assert!(
            !error.message.contains("example.com"),
            "server text must not be echoed"
        );
    }

    #[test]
    fn oauth_string_error_and_terminal_refresh() {
        let error = api_error(
            Some(&json!({ "error": "refresh_token_reused" })),
            Some(400),
            None,
        );
        assert_eq!(error.kind, ErrorKind::Reauth);
        assert!(error.is_terminal_refresh());
    }

    #[test]
    fn detail_text_is_never_a_code() {
        let body = json!({ "detail": "Direct routing not enabled for sk-secret" });
        let error = api_error(Some(&body), Some(503), None);
        assert_eq!(error.code, "api_error");
        assert_eq!(error.kind, ErrorKind::Temporary);
        assert!(!error.message.contains("sk-secret"));
    }

    #[test]
    fn unsafe_request_id_is_dropped() {
        let error = api_error(None, Some(500), Some("has spaces <script>"));
        assert_eq!(error.request_id, None);
    }

    #[test]
    fn legacy_codes_are_recognized() {
        let body = json!({ "error": { "code": "subscription_sharing_v2_user_not_eligible" } });
        assert_eq!(
            api_error(Some(&body), Some(403), None).kind,
            ErrorKind::NotEligible
        );
    }
}
