//! Miner session handshake: `Hello`/`Welcome`/`Configure` and exit codes.

use quip_proto::v1::{Algorithm, Backend, Capabilities, Configure, Hello, Welcome};

/// Protocol version this SDK speaks. `Welcome.protocol_version` must equal this.
pub const PROTOCOL_VERSION: u32 = 2;

/// Errors from building a `Hello` or validating a `Welcome`.
///
/// `Copy`, because the session loop both reports one of these and derives the
/// process exit code from it ([`ExitCode::from`]), and moving it into the
/// conversion would leave nothing to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// `QUIP_SESSION_TOKEN` missing or empty.
    MissingToken,
    /// `Welcome.protocol_version` did not match [`PROTOCOL_VERSION`].
    BadWelcome(u32),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingToken => {
                write!(f, "QUIP_SESSION_TOKEN environment variable not set")
            }
            Self::BadWelcome(v) => write!(f, "unexpected protocol version in Welcome: {v}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// Sysexits-style process exit codes (also carried on `Fatal.exit_code`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Clean exit.
    Clean = 0,
    /// Missing/invalid CLI or config (e.g. no `--quip-coordinator`, bad Welcome).
    ConfigInvalid = 64,
    /// Host/env cannot run this miner (`--check` failed).
    EnvIncompatible = 69,
    /// Unexpected internal failure.
    InternalFatal = 70,
    /// `QUIP_SESSION_TOKEN` missing/empty or rejected.
    TokenRejected = 77,
}

impl ExitCode {
    /// Integer value of this exit code (for process exit / `Fatal.exit_code`).
    #[must_use]
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

impl From<SessionError> for ExitCode {
    fn from(e: SessionError) -> Self {
        match e {
            SessionError::MissingToken => Self::TokenRejected,
            SessionError::BadWelcome(_) => Self::ConfigInvalid,
        }
    }
}

/// Runtime session parameters after applying `Configure` defaults.
pub struct SessionConfig {
    /// Miner identity string from the local config.
    pub miner_id: String,
    /// Max in-flight jobs; default `3` when configure sends `0`.
    pub queue_depth: u32,
    /// Idle timeout seconds; default `300` when configure sends `0`.
    pub idle_timeout_s: u32,
    /// Heartbeat interval seconds; default `15` when configure sends `0`.
    pub heartbeat_s: u32,
    /// Reconnect window seconds; default `60` when configure sends `0`.
    pub reconnect_window_s: u32,
}

impl SessionConfig {
    /// Build config from a miner id and coordinator `Configure`, applying
    /// defaults for any zero field.
    #[must_use]
    pub fn from_configure(miner_id: String, c: &Configure) -> Self {
        let d = |v: u32, default: u32| if v == 0 { default } else { v };
        Self {
            miner_id,
            queue_depth: d(c.queue_depth, 3),
            idle_timeout_s: d(c.idle_timeout_s, 300),
            heartbeat_s: d(c.heartbeat_s, 15),
            reconnect_window_s: d(c.reconnect_window_s, 60),
        }
    }
}

/// Build the miner handshake, reading the session token from the environment.
///
/// # Errors
/// Returns [`SessionError::MissingToken`] if the token is missing or empty.
pub fn build_hello(miner_id: &str, capabilities: Capabilities) -> Result<Hello, SessionError> {
    let token = std::env::var("QUIP_SESSION_TOKEN").map_err(|_| SessionError::MissingToken)?;
    if token.is_empty() {
        return Err(SessionError::MissingToken);
    }
    Ok(Hello {
        miner_id: miner_id.into(),
        session_token: token,
        capabilities: Some(capabilities),
    })
}

/// Reject a `Welcome` whose `protocol_version` is not what this SDK speaks.
///
/// # Errors
/// Returns [`SessionError::BadWelcome`] when the version is not
/// [`PROTOCOL_VERSION`].
pub fn check_welcome(w: &Welcome) -> Result<(), SessionError> {
    if w.protocol_version != PROTOCOL_VERSION {
        return Err(SessionError::BadWelcome(w.protocol_version));
    }
    Ok(())
}

const BACKEND_NAMES: &[(Backend, &str)] = &[
    (Backend::Unspecified, "unspecified"),
    (Backend::Cpu, "cpu"),
    (Backend::Cuda, "cuda"),
    (Backend::Metal, "metal"),
    (Backend::Ane, "ane"),
    (Backend::DwaveQpu, "dwave-qpu"),
    (Backend::Exec, "exec"),
    (Backend::Mock, "mock"),
];
/// Stable lowercase name for a wire identity.
#[must_use]
pub fn backend_name(value: Backend) -> &'static str {
    BACKEND_NAMES
        .iter()
        .find_map(|&(v, name)| (v == value).then_some(name))
        .unwrap_or("unspecified")
}
/// Parse a stable lowercase wire identity name.
#[must_use]
pub fn backend_from_name(name: &str) -> Option<Backend> {
    BACKEND_NAMES
        .iter()
        .find_map(|&(v, n)| (n == name).then_some(v))
}

const ALGORITHM_NAMES: &[(Algorithm, &str)] = &[
    (Algorithm::Unspecified, "unspecified"),
    (Algorithm::Sa, "sa"),
    (Algorithm::Gibbs, "gibbs"),
    (Algorithm::QuantumAnneal, "quantum-anneal"),
    (Algorithm::Fsa, "fsa"),
    (Algorithm::Msa, "msa"),
    (Algorithm::Flatiron, "flatiron"),
    (Algorithm::Mps, "mps"),
    (Algorithm::Mfa, "mfa"),
    (Algorithm::Sb, "sb"),
    (Algorithm::Bsb, "bsb"),
    (Algorithm::Gbsb, "gbsb"),
    (Algorithm::Gdsb, "gdsb"),
    (Algorithm::Ggdsb, "ggdsb"),
    (Algorithm::Hbsb, "hbsb"),
    (Algorithm::Hdsb, "hdsb"),
    (Algorithm::Sbqa, "sbqa"),
    (Algorithm::Tedsb, "tedsb"),
    (Algorithm::External, "external"),
];
/// Stable lowercase name for a wire identity.
#[must_use]
pub fn algorithm_name(value: Algorithm) -> &'static str {
    ALGORITHM_NAMES
        .iter()
        .find_map(|&(v, name)| (v == value).then_some(name))
        .unwrap_or("unspecified")
}
/// Parse a stable lowercase wire identity name.
#[must_use]
pub fn algorithm_from_name(name: &str) -> Option<Algorithm> {
    ALGORITHM_NAMES
        .iter()
        .find_map(|&(v, n)| (n == name).then_some(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quip_proto::v1::{Algorithm, Backend, Capabilities, Configure, Welcome};

    #[test]
    fn hello_requires_token() {
        let caps = Capabilities {
            protocol_version: 2,
            features: vec!["streaming".into()],
            ..Default::default()
        };
        std::env::remove_var("QUIP_SESSION_TOKEN");
        assert_eq!(
            build_hello("cpu-0", caps.clone()),
            Err(SessionError::MissingToken)
        );
        std::env::set_var("QUIP_SESSION_TOKEN", "tok-123");
        let hello = build_hello("cpu-0", caps.clone()).unwrap();
        assert_eq!(hello.session_token, "tok-123");
        assert_eq!(hello.capabilities, Some(caps.clone()));
        assert_eq!(PROTOCOL_VERSION, 2);
        std::env::set_var("QUIP_SESSION_TOKEN", "");
        assert_eq!(build_hello("cpu-0", caps), Err(SessionError::MissingToken));
        std::env::remove_var("QUIP_SESSION_TOKEN");
    }

    #[test]
    fn identity_names_are_unique_and_roundtrip() {
        let mut names = std::collections::HashSet::new();
        for raw in 0..=7 {
            let value = Backend::try_from(raw).unwrap();
            assert!(names.insert(backend_name(value)));
            assert_eq!(backend_from_name(backend_name(value)), Some(value));
        }
        names.clear();
        for raw in 0..=18 {
            let value = Algorithm::try_from(raw).unwrap();
            assert!(names.insert(algorithm_name(value)));
            assert_eq!(algorithm_from_name(algorithm_name(value)), Some(value));
        }
        assert_eq!(backend_from_name("test"), None);
        assert_eq!(algorithm_from_name("quantum"), None);
    }

    #[test]
    fn configure_applies_defaults_for_zero_fields() {
        let c = Configure {
            queue_depth: 0,
            idle_timeout_s: 0,
            heartbeat_s: 0,
            reconnect_window_s: 0,
            backend_toml: String::new(),
        };
        let cfg = SessionConfig::from_configure("cpu-0".into(), &c);
        assert_eq!(cfg.queue_depth, 3);
        assert_eq!(cfg.idle_timeout_s, 300);
        assert_eq!(cfg.heartbeat_s, 15);
        assert_eq!(cfg.reconnect_window_s, 60);
    }

    #[test]
    fn welcome_rejects_non_v2_protocol_version() {
        assert!(check_welcome(&Welcome {
            protocol_version: 2
        })
        .is_ok());
        assert_eq!(
            check_welcome(&Welcome {
                protocol_version: 1
            }),
            Err(SessionError::BadWelcome(1))
        );
        assert_eq!(
            check_welcome(&Welcome {
                protocol_version: 0
            }),
            Err(SessionError::BadWelcome(0))
        );
    }

    #[test]
    fn session_error_maps_to_documented_exit_codes() {
        assert_eq!(
            ExitCode::from(SessionError::MissingToken),
            ExitCode::TokenRejected
        );
        assert_eq!(
            ExitCode::from(SessionError::BadWelcome(2)),
            ExitCode::ConfigInvalid
        );
        assert_eq!(ExitCode::ConfigInvalid.as_i32(), 64);
        assert_eq!(ExitCode::EnvIncompatible.as_i32(), 69);
        assert_eq!(ExitCode::InternalFatal.as_i32(), 70);
        assert_eq!(ExitCode::TokenRejected.as_i32(), 77);
    }
}
