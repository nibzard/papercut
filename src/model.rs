//! The on-disk data model: events (Layer 2 reports / Layer 1-derived) and the
//! shared signal schema (Layer 1).
//!
//! Observation ≠ hypothesis ≠ fix: these are separate fields and must never be
//! merged. `summary` is evidence; `hypothesis` and `suggested_fix` may be wrong.

use serde::{Deserialize, Serialize};

/// How an event entered the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[value(name = "in_moment")]
    InMoment,
    #[value(name = "sweep")]
    Sweep,
    #[value(name = "triage")]
    Triage,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::InMoment => "in_moment",
            Source::Sweep => "sweep",
            Source::Triage => "triage",
        }
    }
}

/// Lifecycle of a papercut. Terminal statuses require a resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[value(name = "candidate")]
    Candidate,
    #[value(name = "open")]
    Open,
    #[value(name = "fixed")]
    Fixed,
    #[value(name = "promoted")]
    Promoted,
    #[value(name = "duplicate")]
    Duplicate,
    #[value(name = "dismissed")]
    Dismissed,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Candidate => "candidate",
            Status::Open => "open",
            Status::Fixed => "fixed",
            Status::Promoted => "promoted",
            Status::Duplicate => "duplicate",
            Status::Dismissed => "dismissed",
        }
    }

    /// Terminal statuses are done with the triage loop.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Status::Fixed | Status::Promoted | Status::Duplicate | Status::Dismissed
        )
    }

    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "candidate" => Some(Status::Candidate),
            "open" => Some(Status::Open),
            "fixed" => Some(Status::Fixed),
            "promoted" => Some(Status::Promoted),
            "duplicate" => Some(Status::Duplicate),
            "dismissed" => Some(Status::Dismissed),
            _ => None,
        }
    }
}

/// Repo + agent context surrounding a papercut. No env-var values, transcripts,
/// or source files ever live here — only pointers (repo, relative cwd, sha,
/// task id, agent, timestamp).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventContext {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub git_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub task: Option<String>,
}

/// Why an event reached its terminal status, and where the fix landed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resolution {
    pub reason: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none", default)]
    pub ref_: Option<String>,
}

/// One papercut event. `schema_version` pins the on-disk shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub schema_version: u32,
    pub id: String,
    pub created_at: String,
    pub source: Source,
    pub status: Status,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hypothesis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub suggested_fix: Option<String>,
    /// Free-form tag from the reporter (e.g. "tooling", "docs"). Optional
    /// extension over the documented v1 core; omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category: Option<String>,
    pub context: EventContext,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resolution: Option<Resolution>,
}

/// The current on-disk schema version.
pub const SCHEMA_VERSION: u32 = 1;

impl Event {
    /// Validate status/resolution invariants. Every terminal status requires a
    /// `resolution.reason`; `fixed` additionally requires a `resolution.ref`.
    pub fn validate(&self) -> Result<(), String> {
        if self.status.is_terminal() {
            match &self.resolution {
                None => Err(format!(
                    "status `{}` is terminal but has no resolution.reason",
                    self.status.label()
                )),
                Some(r) if r.reason.trim().is_empty() => Err(format!(
                    "status `{}` is terminal but resolution.reason is empty",
                    self.status.label()
                )),
                Some(_)
                    if self.status == Status::Fixed
                        && self.resolution.as_ref().unwrap().ref_.is_none() =>
                {
                    Err("status `fixed` requires a resolution.ref".into())
                }
                _ => Ok(()),
            }
        } else if self.resolution.is_some() {
            // Non-terminal events should not carry a resolution.
            Err(format!(
                "status `{}` is not terminal but carries a resolution",
                self.status.label()
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_event(status: Status, resolution: Option<Resolution>) -> Event {
        Event {
            schema_version: SCHEMA_VERSION,
            id: "pc_01K0000000000000000000000".into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status,
            summary: "x".into(),
            hypothesis: None,
            suggested_fix: None,
            category: None,
            context: EventContext::default(),
            resolution,
        }
    }

    #[test]
    fn open_without_resolution_is_valid() {
        assert!(base_event(Status::Open, None).validate().is_ok());
    }

    #[test]
    fn terminal_requires_reason() {
        let e = base_event(Status::Dismissed, None);
        assert!(e.validate().is_err());
    }

    #[test]
    fn fixed_requires_ref() {
        let e = base_event(
            Status::Fixed,
            Some(Resolution {
                reason: "done".into(),
                ref_: None,
            }),
        );
        assert!(e.validate().is_err());
    }

    #[test]
    fn fixed_with_ref_is_valid() {
        let e = base_event(
            Status::Fixed,
            Some(Resolution {
                reason: "done".into(),
                ref_: Some("abc1234".into()),
            }),
        );
        assert!(e.validate().is_ok());
    }

    #[test]
    fn non_terminal_with_resolution_is_invalid() {
        let e = base_event(
            Status::Open,
            Some(Resolution {
                reason: "x".into(),
                ref_: None,
            }),
        );
        assert!(e.validate().is_err());
    }

    #[test]
    fn json_roundtrip_preserves_fields() {
        let e = base_event(
            Status::Fixed,
            Some(Resolution {
                reason: "pinned".into(),
                ref_: Some("abc1234".into()),
            }),
        );
        let s = serde_json::to_string(&e).unwrap();
        // `ref_` serializes as `ref`.
        assert!(s.contains("\"ref\":\"abc1234\""));
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back.status, Status::Fixed);
        assert_eq!(
            back.resolution.as_ref().unwrap().ref_.as_deref(),
            Some("abc1234")
        );
    }
}
