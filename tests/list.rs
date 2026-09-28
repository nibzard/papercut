//! Human listing order and resolution visibility through the public CLI.

mod common;

use common::IsolatedEnv;
use papercut::model::{Resolution, Status};
use std::process::Command;

fn run(args: &[&str]) -> (i32, String) {
    let output = Command::new(common::bin()).args(args).output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).unwrap(),
    )
}

#[test]
fn active_reports_lead_and_terminal_reasons_explain_old_observations() {
    let _env = IsolatedEnv::new();
    let mut fixed_old = common::test_event("pc_01K000000000000000000000A", "old failure");
    fixed_old.status = Status::Fixed;
    fixed_old.resolution = Some(Resolution {
        reason: "installed missing tool".into(),
        ref_: Some("abc1234".into()),
    });
    let mut dismissed = common::test_event("pc_01K000000000000000000000B", "invalid flag");
    dismissed.status = Status::Dismissed;
    dismissed.resolution = Some(Resolution {
        reason: "ordinary command usage".into(),
        ref_: None,
    });
    let mut open = common::test_event("pc_01K000000000000000000000C", "current problem");
    open.hypothesis = Some("maybe a race".into());
    open.resolution = Some(Resolution {
        reason: "stale resolution from reopening".into(),
        ref_: Some("old-ref".into()),
    });
    let mut candidate = common::test_event("pc_01K000000000000000000000D", "needs review");
    candidate.status = Status::Candidate;
    let mut fixed_new = common::test_event("pc_01K000000000000000000000E", "another old failure");
    fixed_new.status = Status::Fixed;
    fixed_new.resolution = Some(Resolution {
        reason: "updated instructions".into(),
        ref_: Some("def5678".into()),
    });
    for mut event in [fixed_old, dismissed, open, candidate, fixed_new] {
        event.context.agent = Some("claude-code".into());
        papercut::store::write_event(&event).unwrap();
    }

    let (code, text) = run(&["list", "--repo", "all"]);
    assert_eq!(code, 0);
    assert!(text.starts_with("Papercuts · all repos\n"), "{text}");
    assert!(text.contains("1 open · 1 candidate · 2 fixed · 1 dismissed"));
    assert!(text.contains("Needs attention (2)"));
    assert!(text.contains("Reviewed (3)"));
    assert_eq!(text.matches("claude-code").count(), 1);
    assert!(text.contains("● OPEN"));
    assert!(text.contains("✓ FIXED"));
    assert!(text.contains("? Hypothesis: maybe a race"));
    assert!(
        !text.contains("\x1b["),
        "captured stdout stays plain: {text}"
    );
    let positions: Vec<usize> = [
        "current problem",
        "needs review",
        "old failure",
        "another old failure",
        "invalid flag",
    ]
    .into_iter()
    .map(|needle| text.find(needle).unwrap())
    .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    assert!(text.contains("↳ Resolution: installed missing tool"));
    assert!(text.contains("↳ Resolution: ordinary command usage"));
    assert!(!text.contains("stale resolution from reopening"));

    let (code, raw) = run(&["list", "--repo", "all", "--output", "json"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let statuses: Vec<&str> = value["data"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        ["open", "candidate", "fixed", "fixed", "dismissed"]
    );
}
