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

/// The product whose public usage caused the observation. The consuming repo
/// remains in `context` and may be unrelated to the product's source repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Product {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub surface: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product: Option<Product>,
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
pub const LEGACY_SCHEMA_VERSION: u32 = 1;
pub const SCHEMA_VERSION: u32 = 2;

impl Event {
    /// Only v2 records can claim product attribution. A stray product key on
    /// a legacy record does not turn it into a product report.
    pub fn attributed_product(&self) -> Option<&Product> {
        (self.schema_version == SCHEMA_VERSION)
            .then_some(self.product.as_ref())
            .flatten()
    }

    /// Validate status/resolution invariants. Every terminal status requires a
    /// non-empty `resolution.reason`; `fixed` additionally requires a non-empty
    /// `resolution.ref`. Non-terminal events are allowed to carry a resolution.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version == SCHEMA_VERSION {
            let product = self.product.as_ref().ok_or("schema v2 requires product")?;
            if product.id.trim().is_empty() || product.id != product.id.trim() {
                return Err("product.id must be nonblank and trimmed".into());
            }
            for (name, value) in [
                ("product.version", &product.version),
                ("product.surface", &product.surface),
            ] {
                if value.as_ref().is_some_and(|v| v.trim().is_empty()) {
                    return Err(format!("{name} must be nonblank when supplied"));
                }
            }
        }
        if !self.status.is_terminal() {
            // PLAN documents only the forward rule (terminal ⇒ resolution). The
            // documented reopen workflow — "status changes are edits to one JSON
            // field" — leaves the prior resolution in place when a fixed papercut
            // is flipped back to `open`. Rejecting that here would quarantine a
            // previously-valid event out of `list` and the triage pack with no
            // command able to surface or repair it. So the stale resolution is
            // allowed to ride along; `render`/`list` project off `status`, not
            // `resolution`.
            return Ok(());
        }
        let r = match &self.resolution {
            None => {
                return Err(format!(
                    "status `{}` is terminal but has no resolution.reason",
                    self.status.label()
                ))
            }
            Some(r) => r,
        };
        if r.reason.trim().is_empty() {
            return Err(format!(
                "status `{}` is terminal but resolution.reason is empty",
                self.status.label()
            ));
        }
        // `fixed` requires a ref that is present AND non-blank. An absent ref and
        // a wiped-to-`""` ref are the same defect: `serde` deserializes `"ref":""`
        // as `Some("")` (not `None`), so an `is_none()` check alone is bypassed.
        if self.status == Status::Fixed && r.ref_.as_ref().is_none_or(|rf| rf.trim().is_empty()) {
            return Err("status `fixed` requires a non-empty resolution.ref".into());
        }
        Ok(())
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
            product: Some(Product {
                id: "test-product".into(),
                version: None,
                surface: None,
            }),
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

    /// `serde` turns `"ref":""` into `Some("")`, not `None`. A blank ref must
    /// fail the "fixed requires a ref" rule just like an absent one — otherwise
    /// a wiped field defeats the invariant.
    #[test]
    fn fixed_with_blank_ref_is_invalid() {
        for blank in ["", "   ", "\t"] {
            let e = base_event(
                Status::Fixed,
                Some(Resolution {
                    reason: "done".into(),
                    ref_: Some(blank.into()),
                }),
            );
            assert!(
                e.validate().is_err(),
                "blank ref {blank:?} must fail validation"
            );
        }
    }

    /// Reopening a fixed papercut (editing `status` back to `open`) is the
    /// documented one-field-edit workflow and leaves the prior resolution in
    /// place. Such an event must NOT be quarantined.
    #[test]
    fn non_terminal_with_resolution_is_valid() {
        let e = base_event(
            Status::Open,
            Some(Resolution {
                reason: "previously fixed".into(),
                ref_: Some("abc1234".into()),
            }),
        );
        assert!(e.validate().is_ok());
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
