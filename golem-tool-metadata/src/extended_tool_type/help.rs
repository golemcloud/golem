// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;

/// Resolve a command path (names or aliases) to a command-tree index using
/// checked lookups, so a malformed tree cannot panic.
fn resolve_command_path(
    tool: &ExtendedToolType,
    command_path: &[String],
) -> Result<usize, ToolBuildError> {
    let mut idx = 0usize;
    if tool.commands.is_empty() {
        return Err(ToolBuildError::EmptyCommandTree);
    }
    for part in command_path {
        let node = tool
            .commands
            .get(idx)
            .ok_or_else(|| ToolBuildError::CommandNotFound(part.clone()))?;
        let next = node.subcommands.iter().find_map(|i| {
            let child_idx = usize::try_from(*i).ok()?;
            let child = tool.commands.get(child_idx)?;
            if &child.name == part || child.aliases.iter().any(|a| a == part) {
                Some(child_idx)
            } else {
                None
            }
        });
        idx = next.ok_or_else(|| ToolBuildError::CommandNotFound(part.clone()))?;
    }
    Ok(idx)
}

/// Render help text for a command node addressed by `command_path` (empty path
/// = root). Lists inherited globals, the body's positionals/options/flags, and
/// subcommands.
pub fn render_help(
    tool: &ExtendedToolType,
    command_path: &[String],
) -> Result<String, ToolBuildError> {
    let idx = resolve_command_path(tool, command_path)?;
    let n = tool
        .commands
        .get(idx)
        .ok_or(ToolBuildError::EmptyCommandTree)?;
    let mut out = format!(
        "Usage: {}\n\n{}\n{}\n",
        n.name, n.doc.summary, n.doc.description
    );
    let globals = tool.effective_globals(idx);
    if !globals.is_empty() {
        out.push_str("\nGlobals:\n");
        for g in globals {
            match g {
                EffectiveCommandField::Option(o) => {
                    out.push_str(&format!("  --{}\t{}\n", o.long, o.doc.summary))
                }
                EffectiveCommandField::Flag(f) => {
                    out.push_str(&format!("  --{}\t{}\n", f.long, f.doc.summary))
                }
            }
        }
    }
    if let Some(b) = &n.body {
        if !b.positionals.fixed.is_empty() {
            out.push_str("\nPositionals:\n");
            for p in &b.positionals.fixed {
                out.push_str(&format!("  {}\t{}\n", p.name, p.doc.summary));
            }
        }
        if let Some(t) = &b.positionals.tail {
            out.push_str("\nTail:\n");
            out.push_str(&format!("  {}...\t{}\n", t.name, t.doc.summary));
        }
        if !b.options.is_empty() {
            out.push_str("\nOptions:\n");
            for o in &b.options {
                out.push_str(&format!("  --{}\t{}\n", o.long, o.doc.summary));
            }
        }
        if !b.flags.is_empty() {
            out.push_str("\nFlags:\n");
            for f in &b.flags {
                out.push_str(&format!("  --{}\t{}\n", f.long, f.doc.summary));
            }
        }
    }
    if !n.subcommands.is_empty() {
        out.push_str("\nSubcommands:\n");
        for i in &n.subcommands {
            if let Some(c) = usize::try_from(*i).ok().and_then(|j| tool.commands.get(j)) {
                out.push_str(&format!("  {}\t{}\n", c.name, c.doc.summary));
            }
        }
    }
    Ok(out)
}

/// Render help text for a single argument of the command addressed by
/// `command_path`. Searches inherited globals, then the body's positionals,
/// tail, options, and flags (in canonical order), matching the long name or an
/// alias. Returns [`ToolBuildError::CommandNotFound`] if no such argument
/// exists on that command.
pub fn render_argument_help(
    tool: &ExtendedToolType,
    command_path: &[String],
    arg_name: &str,
) -> Result<String, ToolBuildError> {
    let idx = resolve_command_path(tool, command_path)?;

    for g in tool.effective_globals(idx) {
        match g {
            EffectiveCommandField::Option(o)
                if o.long == arg_name || o.aliases.iter().any(|a| a == arg_name) =>
            {
                return Ok(render_option_help(&o, true));
            }
            EffectiveCommandField::Flag(f)
                if f.long == arg_name || f.aliases.iter().any(|a| a == arg_name) =>
            {
                return Ok(render_flag_help(&f, true));
            }
            _ => {}
        }
    }

    if let Some(body) = tool.commands.get(idx).and_then(|c| c.body.as_ref()) {
        for p in &body.positionals.fixed {
            if p.name == arg_name {
                return Ok(format!(
                    "{} (positional{})\n{}\n{}\n",
                    p.name,
                    if p.required { ", required" } else { "" },
                    p.doc.summary,
                    p.doc.description
                ));
            }
        }
        if let Some(t) = &body.positionals.tail
            && t.name == arg_name
        {
            return Ok(format!(
                "{}... (tail positional)\n{}\n{}\n",
                t.name, t.doc.summary, t.doc.description
            ));
        }
        for o in &body.options {
            if o.long == arg_name || o.aliases.iter().any(|a| a == arg_name) {
                return Ok(render_option_help(o, false));
            }
        }
        for f in &body.flags {
            if f.long == arg_name || f.aliases.iter().any(|a| a == arg_name) {
                return Ok(render_flag_help(f, false));
            }
        }
    }

    Err(ToolBuildError::CommandNotFound(arg_name.to_string()))
}

fn render_option_help(o: &ExtendedOptionSpec, global: bool) -> String {
    format!(
        "--{} (option{})\n{}\n{}\n",
        o.long,
        if global { ", global" } else { "" },
        o.doc.summary,
        o.doc.description
    )
}

fn render_flag_help(f: &FlagSpec, global: bool) -> String {
    format!(
        "--{} (flag{})\n{}\n{}\n",
        f.long,
        if global { ", global" } else { "" },
        f.doc.summary,
        f.doc.description
    )
}
