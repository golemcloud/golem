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

//! Agent-only enrichment of `--help` output.
//!
//! When the CLI detects that it is being driven by an automated coding agent
//! (Amp, Claude Code, Codex, Gemini CLI, OpenCode, etc.) and the current
//! working directory is inside a Golem application that has skills installed
//! at `<app_dir>/.agents/skills/`, this module appends a `Relevant skills:`
//! block to the `--help` (long help) of selected commands. The block links
//! to the matching `SKILL.md` files via `file://` URLs that agents can
//! follow to load deeper, task-specific instructions.
//!
//! The module is a no-op when no agent is detected, when no application
//! directory is found, or when no skill files match. Humans never see this
//! section.
//!
//! Behavior can be overridden via `GOLEM_CLI_AGENT_HINTS=0|1`.

mod builtin_skill_map;
mod detect;
mod inject;
mod skill_discovery;

pub use detect::is_agent_help_enabled;
pub use inject::augment_command_with_skill_links;
