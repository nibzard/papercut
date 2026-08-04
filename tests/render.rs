//! Render invariants: byte-identical determinism, input-order independence,
//! no timestamps leaking into the projection.

mod common;

use papercut::model::{EventContext, Resolution, Source, Status};
use papercut::paths::RepoScope;
use papercut::projection::render_markdown;

fn ev(id: &str, status: Status, summary: &str) -> papercut::model::Event {
    let mut e = common::test_event(id, summary);
    e.status = status;
    e.source = Source::InMoment;
    e.context = EventContext {
        agent: Some("claude-code".into()),
        cwd: Some("src/lib.rs".into()),
        git_sha: Some("abc1234".into()),
        ..Default::default()
    };
    e
}

#[test]
fn same_events_are_byte_identical() {
    let events = vec![
        ev("pc_01K000000000000000000000A", Status::Open, "a"),
        ev("pc_01K000000000000000000000B", Status::Fixed, "b"),
    ];
    let a = render_markdown(&RepoScope::All, &events);
    let b = render_markdown(&RepoScope::All, &events);
    assert_eq!(a, b);
}

/// Output must be independent of the order events arrive in: the renderer sorts
/// by id internally. This guards the "same events in, byte-identical out" rule
/// against callers that hand events in arbitrary order.
#[test]
fn input_order_does_not_change_output() {
    let one = ev("pc_01K000000000000000000000A", Status::Open, "first");
    let two = ev("pc_01K000000000000000000000B", Status::Open, "second");
    let fwd = render_markdown(&RepoScope::All, &[one.clone(), two.clone()]);
    let rev = render_markdown(&RepoScope::All, &[two, one]);
    assert_eq!(fwd, rev);
}

#[test]
fn open_section_precedes_terminal_sections() {
    let events = vec![
        ev("pc_01K000000000000000000000A", Status::Fixed, "fixed"),
        ev("pc_01K000000000000000000000B", Status::Open, "open"),
    ];
    let md = render_markdown(&RepoScope::All, &events);
    let open_idx = md.find("## open").unwrap();
    let fixed_idx = md.find("## fixed").unwrap();
    assert!(open_idx < fixed_idx);
}

/// Timestamps are deliberately absent from the projection — they would make
/// "same events in, byte-identical out" impossible across runs.
#[test]
fn no_timestamps_in_output() {
    let e = ev("pc_01K000000000000000000000C", Status::Open, "x");
    let md = render_markdown(&RepoScope::All, std::slice::from_ref(&e));
    assert!(
        !md.contains("2026-08-04"),
        "render must not emit timestamps; got: {md}"
    );
}

#[test]
fn resolution_ref_is_rendered() {
    let mut e = ev("pc_01K000000000000000000000D", Status::Fixed, "done");
    e.resolution = Some(Resolution {
        reason: "pinned dep".into(),
        ref_: Some("abc1234".into()),
    });
    let md = render_markdown(&RepoScope::All, std::slice::from_ref(&e));
    assert!(md.contains("resolved: pinned dep (`abc1234`)"));
}
