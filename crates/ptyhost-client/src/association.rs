//! Reviewed immutable metadata only; arbitrary host metadata is never forwarded.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::SessionSummary;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Claude,
    Codex,
    Grok,
}

impl Source {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "grok" => Some(Self::Grok),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }
}

/// Exact native identities, not host-name prefixes or command-line guesses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Association {
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "identity", rename_all = "snake_case")]
pub enum AssociationState {
    #[default]
    Missing,
    Invalid,
    Declared(Association),
}

/// Reviewed launch identity. It proves no native session association.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LaunchIdentity {
    pub source: Source,
    pub launch_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "identity", rename_all = "snake_case")]
pub enum LaunchState {
    #[default]
    Missing,
    Invalid,
    Declared(LaunchIdentity),
}

/// A successful, internally consistent Info exchange. It is an observation,
/// not an authorization to control a later process with the same host name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostObservation {
    pub summary: SessionSummary,
    pub association: AssociationState,
    /// A launcher-provided opaque instance nonce. PID/creation time are not one.
    pub instance_id: Option<String>,
    /// Only the authenticated/local host's response can confirm child exit.
    pub exited: bool,
    /// Explicit protocol capability from this Info reply, not inferred from
    /// metadata presence. Legacy peers leave this false.
    pub instance_guard_v1: bool,
    /// Launch identity is independent of AssociationState and native delivery.
    pub launch: LaunchState,
    /// Explicit integer-version capability from the Info reply, not metadata.
    pub launch_guard_v1: bool,
    /// Independently observed one-time native association, never read from disk.
    pub native_binding: crate::NativeBindingState,
}

/// Reviewed immutable metadata exactly as the private record on disk declares
/// it, read without connecting. A stale record can declare anything, so this is
/// neither association nor liveness evidence: it only names which native
/// session an unreachable or unverified host claimed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeclaredRecord {
    pub summary: SessionSummary,
    pub association: AssociationState,
    pub instance_id: Option<String>,
}

#[derive(Default, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub association: AssociationState,
    pub instance_id: Option<String>,
    pub launch: LaunchState,
    pub native_identity_present: bool,
}

pub(crate) fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}

pub(crate) fn deserialize_metadata<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Metadata, D::Error> {
    // The protocol is already line-bounded. Discard unknown fields immediately;
    // only the reviewed values below can survive in a HostRecord or public DTO.
    let value = Value::deserialize(deserializer)?;
    let Some(object) = value.as_object() else {
        return Ok(Metadata {
            association: AssociationState::Invalid,
            instance_id: None,
            launch: LaunchState::Invalid,
            native_identity_present: true,
        });
    };
    let string = |key: &str, max| -> Result<Option<String>, ()> {
        match object.get(key) {
            None => Ok(None),
            Some(Value::String(value)) if identifier(value, max) => Ok(Some(value.clone())),
            _ => Err(()),
        }
    };
    let instance_id = match string("instance_id", 128) {
        Ok(Some(value)) if value.len() >= 16 => Some(value),
        Ok(None) => None,
        _ => {
            return Ok(Metadata {
                association: AssociationState::Invalid,
                instance_id: None,
                launch: if object.contains_key("launch_id") {
                    LaunchState::Invalid
                } else {
                    LaunchState::Missing
                },
                native_identity_present: object.contains_key("sid") || object.contains_key("uid"),
            });
        }
    };
    let launch = if !object.contains_key("launch_id") {
        LaunchState::Missing
    } else {
        match (
            object
                .get("source")
                .and_then(Value::as_str)
                .and_then(Source::parse),
            string("launch_id", 128),
            instance_id.as_ref(),
        ) {
            (Some(source), Ok(Some(launch_id)), Some(_)) if launch_id.len() >= 16 => {
                LaunchState::Declared(LaunchIdentity { source, launch_id })
            }
            _ => LaunchState::Invalid,
        }
    };
    let association = if !["source", "sid", "uid"]
        .iter()
        .any(|key| object.contains_key(*key))
    {
        AssociationState::Missing
    } else {
        match (
            object
                .get("source")
                .and_then(Value::as_str)
                .and_then(Source::parse),
            string("sid", 256),
            string("uid", 256),
        ) {
            (Some(source), Ok(sid), Ok(uid)) if sid.is_some() || uid.is_some() => {
                if uid.as_deref().is_some_and(|uid| {
                    !uid.starts_with(&format!("{}:", source.as_str()))
                        || uid.len() == source.as_str().len() + 1
                }) {
                    AssociationState::Invalid
                } else {
                    AssociationState::Declared(Association { source, sid, uid })
                }
            }
            _ => AssociationState::Invalid,
        }
    };
    Ok(Metadata {
        association,
        instance_id,
        launch,
        native_identity_present: object.contains_key("sid") || object.contains_key("uid"),
    })
}
