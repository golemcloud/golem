// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Heuristics for detecting that the CLI was invoked by an automated agent.

use std::env;

/// Manual override env var. Values:
///   - `1` / `true` / `on`  — force agent help enrichment on
///   - `0` / `false` / `off` — force agent help enrichment off
const OVERRIDE_ENV_VAR: &str = "GOLEM_CLI_AGENT_HINTS";

/// Returns `true` when the CLI should emit agent-only help additions.
///
/// Decision order:
///   1. If `GOLEM_CLI_AGENT_HINTS` is set, honor it.
///   2. Otherwise return `true` if any of the known agent fingerprints is
///      detected in the process environment.
pub fn is_agent_help_enabled() -> bool {
    if let Ok(raw) = env::var(OVERRIDE_ENV_VAR) {
        match parse_override(&raw) {
            Some(value) => return value,
            None => {} // fall through to detection
        }
    }
    detect_known_agent()
}

fn parse_override(raw: &str) -> Option<bool> {
    let v = raw.trim();
    if v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on")
    {
        Some(true)
    } else if v.eq_ignore_ascii_case("0")
        || v.eq_ignore_ascii_case("false")
        || v.eq_ignore_ascii_case("off")
    {
        Some(false)
    } else {
        None
    }
}

fn detect_known_agent() -> bool {
    // Amp (Sourcegraph): sets AGENT=amp and AMP_CURRENT_THREAD_ID.
    if matches!(env::var("AGENT").as_deref(), Ok("amp")) {
        return true;
    }
    if env::var_os("AMP_CURRENT_THREAD_ID").is_some() {
        return true;
    }

    // Claude Code (Anthropic): sets CLAUDECODE=1, CLAUDE_CODE_ENTRYPOINT.
    if env::var_os("CLAUDECODE").is_some() {
        return true;
    }
    if env::var_os("CLAUDE_CODE_ENTRYPOINT").is_some() {
        return true;
    }

    // Codex CLI (OpenAI): sets CODEX_HOME and CODEX_SANDBOX_NETWORK_DISABLED.
    if env::var_os("CODEX_HOME").is_some() {
        return true;
    }
    if env::var_os("CODEX_SANDBOX_NETWORK_DISABLED").is_some() {
        return true;
    }

    // Gemini CLI (Google).
    if env::var_os("GEMINI_CLI").is_some() {
        return true;
    }

    // OpenCode.
    if env::var_os("OPENCODE").is_some() {
        return true;
    }

    // Cursor's coding agent.
    if env::var_os("CURSOR_AGENT").is_some() {
        return true;
    }

    false
}

#[cfg(test)]
mod test {
    use super::*;
    use test_r::test;

    #[test]
    fn override_parse_truthy() {
        for v in ["1", "true", "TRUE", "on", "ON"] {
            assert_eq!(parse_override(v), Some(true), "{v}");
        }
    }

    #[test]
    fn override_parse_falsy() {
        for v in ["0", "false", "FALSE", "off", "OFF"] {
            assert_eq!(parse_override(v), Some(false), "{v}");
        }
    }

    #[test]
    fn override_parse_unknown() {
        for v in ["", "yes", "no", "maybe"] {
            assert_eq!(parse_override(v), None, "{v}");
        }
    }
}
