//! Render invariants: byte-identical determinism, input-order independence,
//! no timestamps leaking into the projection.

mod common;

use common::IsolatedEnv;
use papercut::app::RunResult;
use papercut::cli::RenderArgs;
use papercut::commands;
use papercut::model::{EventContext, Resolution, Source, Status};
use papercut::paths::RepoScope;
use papercut::projection::render_markdown;
use std::process::{Command, Stdio};

/// Removes a directory on drop — including during panic unwinding — so a test
/// that builds a throwaway git repo cannot strand a `pc-*<ULID>/` tree (with a
/// `.git/` and PAPERCUTS.md) in `$TMPDIR` when one of its assertions fails.
struct RemoveOnDrop(std::path::PathBuf);
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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
        ..Default::default()
    });
    let md = render_markdown(&RepoScope::All, std::slice::from_ref(&e));
    assert!(md.contains("resolved: pinned dep (`abc1234`)"));
}

/// Quarantined event files are listed inline with file + reason (not an opaque
/// count pointing at `doctor`, which cannot see events). Deterministic for a
/// given store.
#[test]
fn render_surfaces_skipped_file_reasons() {
    let _env = IsolatedEnv::new();
    papercut::store::ensure_store().unwrap();
    std::fs::write(
        papercut::store::events_dir()
            .unwrap()
            .join("pc_01KGARBAGE0000000000000Z.json"),
        "{ broken",
    )
    .unwrap();

    let md = match commands::render::run(RenderArgs {
        repo: "all".into(),
        write: false,
    }) {
        RunResult::Ok { text, .. } => text,
        other => panic!("render failed: {other:?}"),
    };
    assert!(md.contains("1 event file(s) skipped"), "count shown: {md}");
    assert!(
        md.contains("pc_01KGARBAGE0000000000000Z.json"),
        "file name shown: {md}"
    );
    assert!(md.contains("parse error"), "reason shown: {md}");
    assert!(
        !md.contains("papercut doctor"),
        "must not misdirect to doctor (it cannot see events): {md}"
    );
}

/// A quarantined event whose `context.repo` carries markdown (backticks, a
/// link, an embedded newline) must NOT inject into the published PAPERCUTS.md
/// skipped listing — the label is neutralized into a single code span. Before
/// the md_code_span/md_single_line wrap, a crafted repo could forge a heading
/// or link or break out of the skipped bullet when `render --write` committed
/// the projection into a repo.
#[test]
fn render_neutralizes_markdown_in_skipped_repo() {
    let _env = IsolatedEnv::new();
    papercut::store::ensure_store().unwrap();
    let dir = papercut::store::events_dir().unwrap();
    // Parses, then is quarantined for an unsupported schema version — carrying
    // its repo through to the skipped listing. The repo is laced with markdown.
    // The `\n` inside the JSON string is a real newline in the value.
    std::fs::write(
        dir.join("pc_01K000000000000000000000X.json"),
        r#"{
            "schema_version": 99,
            "id": "pc_01K000000000000000000000X",
            "created_at": "2026-08-04T20:42:00Z",
            "source": "in_moment",
            "status": "open",
            "summary": "x",
            "context": { "repo": "`code` [link](http://evil)\n## evil-heading" }
        }"#,
    )
    .unwrap();

    let md = match commands::render::run(RenderArgs {
        repo: "all".into(),
        write: false,
    }) {
        RunResult::Ok { text, .. } => text,
        other => panic!("render failed: {other:?}"),
    };
    // The repo's embedded newline did not forge a new heading line.
    assert!(
        !md.lines()
            .any(|l| l.trim_start().starts_with("## evil-heading")),
        "repo markdown must not forge a heading: {md}"
    );
    // The skipped entry stays one bullet line (newline collapsed into the span).
    let bullets: Vec<&str> = md.lines().filter(|l| l.starts_with("> - ")).collect();
    assert_eq!(bullets.len(), 1, "one collapsed skipped bullet: {md}");
    // The repo text survives as literal text inside the code span.
    assert!(md.contains("link"), "repo text still visible: {md}");
}

/// An event whose `id` field carries markdown (the lenient loader admits a
/// non-ULID id — it validates status/resolution, never id format) must not forge
/// structure in the published PAPERCUTS.md projection. The id is neutralized
/// like every other rendered field; without the wrap a crafted id could forge a
/// duplicate section heading and a peer bullet when `render --write` commits the
/// projection into a repo.
#[test]
fn render_neutralizes_markdown_in_event_id() {
    let _env = IsolatedEnv::new();
    papercut::store::ensure_store().unwrap();
    let dir = papercut::store::events_dir().unwrap();
    std::fs::write(
        dir.join("pc_01K000000000000000000000Y.json"),
        r#"{
            "schema_version": 1,
            "id": "pc_evil\n## open (1)\n- fake",
            "created_at": "2026-08-04T20:42:00Z",
            "source": "in_moment",
            "status": "open",
            "summary": "real event",
            "context": {}
        }"#,
    )
    .unwrap();

    let md = match commands::render::run(RenderArgs {
        repo: "all".into(),
        write: false,
    }) {
        RunResult::Ok { text, .. } => text,
        other => panic!("render failed: {other:?}"),
    };
    // Exactly one `## open` heading (the real section header); the id's embedded
    // newline must not forge a duplicate at column 0.
    let open_headings = md.lines().filter(|l| l.starts_with("## open")).count();
    assert_eq!(open_headings, 1, "no forged open heading from the id: {md}");
    assert!(
        !md.lines().any(|l| l.starts_with("- fake")),
        "no forged bullet from the id: {md}"
    );
    assert!(
        md.contains("real event"),
        "the real summary is still present"
    );
}

/// `render --write --repo <other>` from inside a different repo must REFUSE
/// (exit 1) rather than drop a stray PAPERCUTS.md into the unrelated tree. We
/// build a throwaway git repo so the cwd is detected as a real repo.
#[test]
fn render_write_refuses_unrelated_repo() {
    let _env = IsolatedEnv::new();

    let repo = std::env::temp_dir().join(format!("pc-render-{}", papercut::id::new_id()));
    std::fs::create_dir_all(&repo).unwrap();
    let restore = std::env::current_dir().unwrap();

    let git_ok = || {
        Command::new("git")
            .args(["init"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    std::env::set_current_dir(&repo).unwrap();
    if !git_ok() {
        // git unavailable in this environment — cannot exercise the path.
        std::env::set_current_dir(&restore).unwrap();
        let _ = std::fs::remove_dir_all(&repo);
        eprintln!("skipping render_write_refuses_unrelated_repo: git unavailable");
        return;
    }
    let _ = Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://example.com/me/thisrepo.git",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // cwd resolves to thisrepo; requesting another repo's projection + --write
    // must be refused.
    let r = commands::render::run(RenderArgs {
        repo: "example.com/other/repo".into(),
        write: true,
    });

    // Restore cwd BEFORE any assertion so a panic can't strand later tests.
    std::env::set_current_dir(&restore).unwrap();
    let _ = std::fs::remove_dir_all(&repo);

    match r {
        RunResult::Err { errors, .. } => {
            assert_eq!(
                errors[0].code, "write_refused",
                "wrong error code: {:?}",
                errors[0]
            );
        }
        other => panic!("expected write_refused Err, got {other:?}"),
    }
    // Nothing was written into the unrelated repo tree.
    assert!(
        !repo.join("PAPERCUTS.md").exists(),
        "no stray PAPERCUTS.md in the unrelated repo"
    );
}

/// A repo-scoped view must not surface other repos' quarantined files — and an
/// unattributable skip (a corrupt file that never parsed) must never appear
/// there at all, since `render --write` would otherwise commit other repos'
/// (unknowable) corrupt-file names into this repo's published PAPERCUTS.md.
/// The global view shows everything.
#[test]
fn render_repo_scope_excludes_unattributable_skips() {
    let _env = IsolatedEnv::new();
    papercut::store::ensure_store().unwrap();
    let dir = papercut::store::events_dir().unwrap();

    // Unattributable: never parses → repo unknown.
    std::fs::write(dir.join("pc_01KGARBAGE0000000000000A.json"), "{ broken").unwrap();
    // Attributable to github.com/a/b: parses, then fails validation (fixed w/o ref).
    std::fs::write(
        dir.join("pc_01K000000000000000000000B.json"),
        r#"{
            "schema_version": 1,
            "id": "pc_01K000000000000000000000B",
            "created_at": "2026-08-04T20:42:00Z",
            "source": "in_moment",
            "status": "fixed",
            "summary": "x",
            "context": { "repo": "github.com/a/b" },
            "resolution": { "reason": "done" }
        }"#,
    )
    .unwrap();

    let md_all = match commands::render::run(RenderArgs {
        repo: "all".into(),
        write: false,
    }) {
        RunResult::Ok { text, .. } => text,
        other => panic!("render all failed: {other:?}"),
    };
    // Global view: both quarantined files surfaced.
    assert!(
        md_all.contains("pc_01KGARBAGE0000000000000A.json"),
        "global shows unattributable skip: {md_all}"
    );
    assert!(
        md_all.contains("pc_01K000000000000000000000B.json"),
        "global shows attributable skip: {md_all}"
    );

    let md_a = match commands::render::run(RenderArgs {
        repo: "github.com/a/b".into(),
        write: false,
    }) {
        RunResult::Ok { text, .. } => text,
        other => panic!("render scoped failed: {other:?}"),
    };
    // Scoped to github.com/a/b: only the attributable skip appears; the
    // unknowable parse error does NOT leak into this repo's view.
    assert!(
        md_a.contains("pc_01K000000000000000000000B.json"),
        "scoped shows its own attributable skip: {md_a}"
    );
    assert!(
        !md_a.contains("pc_01KGARBAGE0000000000000A.json"),
        "unattributable skip must not leak into a repo-scoped view: {md_a}"
    );

    let md_other = match commands::render::run(RenderArgs {
        repo: "github.com/other/repo".into(),
        write: false,
    }) {
        RunResult::Ok { text, .. } => text,
        other => panic!("render other failed: {other:?}"),
    };
    // A different repo sees neither skip.
    assert!(
        !md_other.contains("skipped"),
        "other repo's view must contain no quarantine data: {md_other}"
    );
}

/// Run git in cwd; true on success. Used to gate the git-dependent tests.
fn git_silent(args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `render --write --repo <X>` run OUTSIDE any repo must refuse (exit 1) with
/// the no-repo hint — "drop --repo to scope to the current repo" would be a lie
/// here, since dropping --repo selects the global projection, not a repo.
#[test]
fn render_write_refuses_outside_any_repo() {
    let _env = IsolatedEnv::new();
    let tmp = std::env::temp_dir().join(format!("pc-norepo-{}", papercut::id::new_id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let restore = std::env::current_dir().unwrap();
    std::env::set_current_dir(&tmp).unwrap();

    let r = commands::render::run(RenderArgs {
        repo: "github.com/x/y".into(),
        write: true,
    });

    std::env::set_current_dir(&restore).unwrap();
    let _ = std::fs::remove_dir_all(&tmp);

    match r {
        RunResult::Err { errors, .. } => {
            assert_eq!(errors[0].code, "write_refused", "{:?}", errors[0]);
            assert!(
                errors[0].message.contains("outside any repo"),
                "message must explain there is no repo: {}",
                errors[0].message
            );
            assert!(
                !errors[0]
                    .hint
                    .contains("drop --repo to scope to the current repo"),
                "no-repo hint must not promise a current repo: {}",
                errors[0].hint
            );
        }
        other => panic!("expected write_refused Err outside any repo, got {other:?}"),
    }
    assert!(
        !tmp.join("PAPERCUTS.md").exists(),
        "nothing written outside a repo"
    );
}

/// `render --write --repo <thisrepo>` run INSIDE repo <thisrepo> must write the
/// projection into that repo's working-tree root — the regression guard for the
/// in-repo write branch (the branch the cross-repo refuse must not also catch).
#[test]
fn render_write_lands_in_current_repo() {
    let _env = IsolatedEnv::new();
    let repo = std::env::temp_dir().join(format!("pc-inrepo-{}", papercut::id::new_id()));
    std::fs::create_dir_all(&repo).unwrap();
    // Clean up the temp repo on any exit (including an Ok-arm assertion panic),
    // so a failing run cannot strand a pc-inrepo-<ULID>/ tree in $TMPDIR.
    let _cleanup = RemoveOnDrop(repo.clone());
    let restore = std::env::current_dir().unwrap();
    std::env::set_current_dir(&repo).unwrap();
    if !git_silent(&["init"]) {
        std::env::set_current_dir(&restore).unwrap();
        eprintln!("skipping render_write_lands_in_current_repo: git unavailable");
        return;
    }
    let _ = git_silent(&[
        "remote",
        "add",
        "origin",
        "https://example.com/me/thisrepo.git",
    ]);

    let r = commands::render::run(RenderArgs {
        repo: "example.com/me/thisrepo".into(),
        write: true,
    });

    // Restore cwd BEFORE inspecting `r`, so a panic on the Ok arm (e.g. a
    // missing `write` field) cannot strand later tests inside the temp repo.
    std::env::set_current_dir(&restore).unwrap();

    let md_path = repo.join("PAPERCUTS.md");
    match r {
        RunResult::Ok { data, .. } => {
            let wrote = data["write"]
                .as_str()
                .expect("write path recorded")
                .to_string();
            // `wrote` derives from `git rev-parse --show-toplevel`, which
            // canonicalizes symlinks; the temp dir from `std::env::temp_dir()`
            // may not. Compare canonical forms so a symlinked /tmp can't flake
            // the equality (both refer to the same on-disk file).
            let wrote_canon =
                std::fs::canonicalize(&wrote).unwrap_or_else(|_| std::path::PathBuf::from(&wrote));
            let md_canon = std::fs::canonicalize(&md_path)
                .unwrap_or_else(|_| std::path::PathBuf::from(&md_path));
            assert_eq!(wrote_canon, md_canon, "wrote to the repo root");
        }
        other => {
            panic!("expected in-repo write Ok, got {other:?}");
        }
    }
    assert!(md_path.exists(), "PAPERCUTS.md created at repo root");
    let body = std::fs::read_to_string(&md_path).unwrap();
    assert!(body.contains("# Papercuts"), "projection written: {body}");
    assert!(
        body.contains("repo: `example.com/me/thisrepo`"),
        "scoped header present: {body}"
    );
}

/// `render --write --repo all` writes the global projection into the private
/// store (data_root/PAPERCUTS.md), not into any repo — the global write branch.
#[test]
fn render_write_global_to_data_root() {
    let env = IsolatedEnv::new();
    let r = commands::render::run(RenderArgs {
        repo: "all".into(),
        write: true,
    });
    let wrote = match r {
        RunResult::Ok { data, .. } => data["write"]
            .as_str()
            .expect("write path recorded")
            .to_string(),
        other => panic!("expected global write Ok, got {other:?}"),
    };
    let expected = env.data.join("papercuts").join("PAPERCUTS.md");
    assert_eq!(wrote, expected.to_string_lossy(), "global write target");
    assert!(
        expected.exists(),
        "PAPERCUTS.md created in the private store"
    );
    let body = std::fs::read_to_string(&expected).unwrap();
    assert!(body.contains("# Papercuts"), "projection written: {body}");
    assert!(
        body.contains("scope: global (all repos)"),
        "global header present: {body}"
    );
}
