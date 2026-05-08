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

use lenient_bool::LenientBool;
use std::env;

/// Manual override env var. Parsed leniently via [`LenientBool`], so any of
/// `1` / `true` / `t` / `yes` / `y` (case-insensitive) means "force on", and
/// the corresponding negative spellings (`0` / `false` / `f` / `no` / `n`)
/// mean "force off". Any other value is ignored and detection falls through
/// to the env-var fingerprints.
const OVERRIDE_ENV_VAR: &str = "GOLEM_CLI_AGENT_HINTS";

/// Returns `true` when the CLI should emit agent-only help additions.
///
/// Decision order:
///   1. If `GOLEM_CLI_AGENT_HINTS` is set to a recognized boolean, honor it.
///   2. Otherwise return `true` if any of the known agent fingerprints is
///      detected in the process environment.
pub fn is_agent_help_enabled() -> bool {
    if let Ok(raw) = env::var(OVERRIDE_ENV_VAR)
        && let Ok(value) = raw.trim().parse::<LenientBool>()
    {
        return value.into();
    }
    detect_known_agent()
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

    use lenient_bool::LenientBool;
    use test_r::test;

    fn parse(v: &str) -> Option<bool> {
        v.trim().parse::<LenientBool>().ok().map(Into::into)
    }

    #[test]
    fn override_parse_truthy() {
        for v in ["1", "true", "TRUE", "yes", "YES", "y", "t"] {
            assert_eq!(parse(v), Some(true), "{v}");
        }
    }

    #[test]
    fn override_parse_falsy() {
        for v in ["0", "false", "FALSE", "no", "NO", "n", "f"] {
            assert_eq!(parse(v), Some(false), "{v}");
        }
    }

    #[test]
    fn override_parse_unknown_falls_through_to_detection() {
        // Anything LenientBool does not recognize must be ignored, including
        // values like `on`/`off` that this crate does not accept.
        for v in ["", "maybe", "on", "off", "agent", "please"] {
            assert_eq!(parse(v), None, "{v}");
        }
    }
}
