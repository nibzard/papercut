//! Agent detection from the environment.
//!
//! Verified against the installed harness (2026-08-04), not memory:
//! Verified against the installed harness (2026-08-04), not memory: Claude Code
//! sets `CLAUDECODE=1` and `CLAUDE_CODE_SESSION_ID`; the `AI_AGENT` env carries
//! `<agent>_<version>...` for several harnesses.
//!
//! Harness env vars drift — re-verify on implementation, never hardcode blindly.

use std::env;

#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub agent: String,
    pub session: Option<String>,
}

impl AgentInfo {
    pub fn unknown() -> Self {
        Self {
            agent: "unknown".into(),
            session: None,
        }
    }
}

/// Sniff the environment to identify the calling agent and its session id.
pub fn detect() -> AgentInfo {
    // 1. The dedicated `AI_AGENT` marker carries "<name>_<ver>...".
    if let Some(ai) = env::var("AI_AGENT").ok().filter(|s| !s.is_empty()) {
        let head = ai.split('_').next().unwrap_or("").to_lowercase();
        if let Some(agent) = classify(&head) {
            return AgentInfo {
                agent: agent.into(),
                session: session_for(agent),
            };
        }
    }

    // 2. Claude Code's dedicated flag (verified live).
    if env::var_os("CLAUDECODE").is_some() {
        return AgentInfo {
            agent: "claude-code".into(),
            session: env::var("CLAUDE_CODE_SESSION_ID")
                .ok()
                .filter(|s| !s.is_empty()),
        };
    }

    // 3. Codex / OpenCode via their home-dir env vars.
    if env::var_os("CODEX_HOME").is_some() {
        return AgentInfo {
            agent: "codex".into(),
            session: env::var("CODEX_SESSION_ID").ok().filter(|s| !s.is_empty()),
        };
    }
    if env::var_os("OPENCODE_CONFIG").is_some() {
        return AgentInfo {
            agent: "opencode".into(),
            session: None,
        };
    }

    AgentInfo::unknown()
}

fn classify(head: &str) -> Option<&'static str> {
    match head {
        "claude-code" | "claudecode" => Some("claude-code"),
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

fn session_for(agent: &str) -> Option<String> {
    match agent {
        "claude-code" => env::var("CLAUDE_CODE_SESSION_ID")
            .ok()
            .filter(|s| !s.is_empty()),
        "codex" => env::var("CODEX_SESSION_ID").ok().filter(|s| !s.is_empty()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::EnvGuard;

    const DETECT_VARS: &[&str] = &[
        "AI_AGENT",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_HOME",
        "CODEX_SESSION_ID",
        "OPENCODE_CONFIG",
    ];

    /// Guard every detection var and start each test from a clean slate, so a
    /// test run inside a real harness (which sets these vars) stays
    /// deterministic.
    fn clean_guard() -> EnvGuard {
        let g = EnvGuard::acquire(DETECT_VARS);
        for k in DETECT_VARS {
            env::remove_var(k);
        }
        g
    }

    #[test]
    fn unknown_when_nothing_set() {
        let _g = clean_guard();
        let info = detect();
        assert_eq!(info.agent, "unknown");
    }

    #[test]
    fn detects_claude_code() {
        let _g = clean_guard();
        env::set_var("CLAUDECODE", "1");
        env::set_var("CLAUDE_CODE_SESSION_ID", "sess-123");
        let info = detect();
        assert_eq!(info.agent, "claude-code");
        assert_eq!(info.session.as_deref(), Some("sess-123"));
    }

    #[test]
    fn parses_ai_agent_marker() {
        let _g = clean_guard();
        env::set_var("AI_AGENT", "codex_0.105.0_x86");
        let info = detect();
        assert_eq!(info.agent, "codex");
    }
}
