use std::fmt;

use zeroize::Zeroizing;

/// A credential value: access, refresh or ID token, authorization code, PKCE verifier.
///
/// It deliberately implements neither `Serialize` nor `PartialEq`, so it can't be sent over IPC
/// or compared in variable time by accident. `Debug` and `Display` never reveal the value, and
/// the memory is zeroed on drop.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Use only at the point where the value leaves the process (HTTP request, Keychain).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_are_redacted() {
        let secret = Secret::new("eyJhbGciOi.super-secret-token");
        assert_eq!(format!("{secret:?}"), "[redacted]");
        assert_eq!(format!("{secret}"), "[redacted]");
        assert_eq!(
            format!("{:#?}", Some(secret.clone())),
            "Some(\n    [redacted],\n)"
        );
        assert_eq!(secret.expose(), "eyJhbGciOi.super-secret-token");
    }
}
