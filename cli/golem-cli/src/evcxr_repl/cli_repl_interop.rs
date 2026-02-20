// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::evcxr_repl::config::{ClientConfig, ReplResolvedConfig};
use crate::evcxr_repl::log::logln;
use crate::model::cli_command_metadata::{CliArgMetadata, CliCommandMetadata};
use crate::GOLEM_EVCXR_REPL;
use anyhow::Context;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub struct CompletionResult {
    pub start: usize,
    pub values: Vec<String>,
}

pub struct CliReplInterop {
    commands: Vec<ReplCliCommand>,
    commands_by_name: HashMap<String, ReplCliCommand>,
    cli: GolemCli,
    builtin_commands: HashMap<String, String>,
}

impl CliReplInterop {
    pub fn new(config: &ReplResolvedConfig, builtin_commands: HashMap<String, String>) -> Self {
        let commands = collect_repl_cli_commands(&config.cli_command_metadata);
        let commands_by_name = commands
            .iter()
            .map(|command| (command.repl_command.clone(), command.clone()))
            .collect();
        let cli = GolemCli::new(
            config.base_config.binary.clone(),
            config.base_config.app_main_dir.clone().into(),
            config.client_config.clone(),
        );
        Self {
            commands,
            commands_by_name,
            cli,
            builtin_commands,
        }
    }

    pub fn complete(&self, line: &str, pos: usize) -> Option<CompletionResult> {
        let line_prefix = line.get(..pos)?;
        let trimmed_start = line_prefix.trim_start();
        let start_trimmed = trimmed_start.strip_prefix(':')?;

        let ends_with_space = line_prefix.chars().last().is_some_and(char::is_whitespace);
        let tokens = parse_raw_args(start_trimmed);
        let last_token = tokens.last().cloned().unwrap_or_default();
        let ends_with_separator = ends_with_space && !last_token.is_empty();

        let current_token = if ends_with_separator {
            String::new()
        } else {
            last_token.clone()
        };
        let consumed_tokens: Vec<String> = if ends_with_separator {
            tokens
        } else if tokens.is_empty() {
            Vec::new()
        } else {
            tokens[..tokens.len() - 1].to_vec()
        };

        if consumed_tokens.is_empty() {
            let mut completions = filter_by_prefix(
                self.commands_by_name.keys().cloned().collect(),
                &current_token,
            );
            let builtin_completions = filter_by_prefix(
                self.builtin_commands.keys().cloned().collect(),
                &current_token,
            );
            completions.extend(builtin_completions);
            let completions = unique(completions);
            if completions.is_empty() {
                return None;
            }
            return Some(CompletionResult {
                start: pos.saturating_sub(current_token.len()),
                values: completions,
            });
        }

        let command_name = &consumed_tokens[0];
        let command = self.commands_by_name.get(command_name)?;
        let arg_tokens = consumed_tokens[1..].to_vec();
        let parsed = parse_args(command, &arg_tokens);

        if let Some(expecting_value_for) = parsed.expecting_value_for {
            let result = self.complete_arg_value(&expecting_value_for, &current_token);
            if result.values.is_empty() {
                return None;
            }
            return Some(CompletionResult {
                start: pos.saturating_sub(current_token.len()),
                values: result.values,
            });
        }

        if current_token.starts_with('-') {
            let flags = complete_flags(command, &parsed.used_arg_ids, &current_token);
            if flags.is_empty() {
                return None;
            }
            return Some(CompletionResult {
                start: pos.saturating_sub(current_token.len()),
                values: flags,
            });
        }

        let next_positional = command.positional_args.get(parsed.positional_values.len());
        if !current_token.is_empty() {
            if let Some(next_positional) = next_positional {
                let result = self.complete_arg_value(next_positional, &current_token);
                if result.values.is_empty() {
                    return None;
                }
                return Some(CompletionResult {
                    start: pos.saturating_sub(current_token.len()),
                    values: result.values,
                });
            }
        }

        let positional_values_list = if let Some(next_positional) = next_positional {
            self.complete_arg_value(next_positional, &current_token)
                .values
        } else {
            Vec::new()
        };
        let flag_values_list = complete_flags(command, &parsed.used_arg_ids, &current_token);
        let completions = merge_unique(positional_values_list, flag_values_list);
        if completions.is_empty() {
            return None;
        }
        Some(CompletionResult {
            start: pos.saturating_sub(current_token.len()),
            values: completions,
        })
    }

    pub fn try_run_command(&self, line: &str) -> anyhow::Result<bool> {
        let trimmed_start = line.trim_start();
        let start_trimmed = match trimmed_start.strip_prefix(':') {
            Some(value) => value.trim_start(),
            None => return Ok(false),
        };
        if start_trimmed.is_empty() {
            return Ok(false);
        }

        let tokens = parse_raw_args(start_trimmed);
        let (command_name, arg_tokens) = match tokens.split_first() {
            Some((name, rest)) => (name.clone(), rest.to_vec()),
            None => return Ok(false),
        };

        if command_name == "help" {
            self.print_help();
            return Ok(true);
        }

        if self.builtin_commands.contains_key(&command_name) {
            return Ok(false);
        }

        let command = match self.commands_by_name.get(&command_name) {
            Some(command) => command,
            None => return Ok(false),
        };

        let mut args = arg_tokens;
        if let Some(hook) = command_hook(&command.repl_command) {
            args = (hook.adapt_args)(args);
        }

        let full_args = command
            .command_path
            .iter()
            .cloned()
            .chain(args.iter().cloned())
            .collect::<Vec<_>>();

        let result = self.cli.run(RunOptions {
            args: full_args.clone(),
            mode: RunMode::Inherit,
        })?;

        if let Some(hook) = command_hook(&command.repl_command) {
            (hook.handle_result)(full_args.clone(), result.ok, result.code)?;
        }

        Ok(true)
    }

    fn print_help(&self) {
        let mut entries = BTreeMap::<&str, &str>::new();
        for (name, desc) in &self.builtin_commands {
            entries.insert(name, desc);
        }
        for command in &self.commands {
            entries.insert(&command.repl_command, &command.about);
        }
        let name_width = entries.keys().map(|name| name.len()).max().unwrap_or(0);
        for (name, desc) in entries {
            logln(format!(":{name:width$} {desc}", width = name_width));
        }
    }

    fn complete_arg_value(&self, arg: &CliArgMetadata, current_token: &str) -> CompletionValues {
        if !arg.possible_values.is_empty() {
            let values = filter_by_prefix(
                arg.possible_values
                    .iter()
                    .map(|value| value.name.clone())
                    .collect(),
                current_token,
            );
            return CompletionValues { values };
        }

        let hook = find_arg_completion_hook(arg);
        let Some(hook) = hook else {
            return CompletionValues { values: Vec::new() };
        };

        let values = (hook.complete)(self.cli.clone(), current_token.to_string());
        CompletionValues { values }
    }
}

#[derive(Clone)]
struct ReplCliCommand {
    repl_command: String,
    command_path: Vec<String>,
    about: String,
    flag_args: HashMap<String, CliArgMetadata>,
    positional_args: Vec<CliArgMetadata>,
}

struct CompletionValues {
    values: Vec<String>,
}

struct ParsedArgs {
    used_arg_ids: HashSet<String>,
    positional_values: Vec<String>,
    expecting_value_for: Option<CliArgMetadata>,
}

struct CommandHook {
    adapt_args: fn(Vec<String>) -> Vec<String>,
    handle_result: fn(Vec<String>, bool, Option<i32>) -> anyhow::Result<()>,
}

fn command_hook(command: &str) -> Option<CommandHook> {
    match command {
        "deploy" => Some(CommandHook {
            adapt_args: |mut args| {
                let mut adapted = Vec::with_capacity(args.len() + 2);
                adapted.push("--repl-bridge-sdk-target".to_string());
                adapted.push("ts".to_string());
                adapted.append(&mut args);
                adapted
            },
            handle_result: |args, ok, _code| {
                if args.iter().any(|arg| arg == "--plan" || arg == "stage") {
                    return Ok(());
                }
                if !ok {
                    return Ok(());
                }
                exit_with_reload_code()
            },
        }),
        _ => None,
    }
}

struct CompletionHook {
    complete: fn(GolemCli, String) -> Vec<String>,
}

fn find_arg_completion_hook(arg: &CliArgMetadata) -> Option<CompletionHook> {
    let mut candidates = Vec::with_capacity(1 + arg.value_names.len());
    candidates.push(arg.id.clone());
    candidates.extend(arg.value_names.clone());
    for candidate in candidates {
        if let Some(hook) = completion_hook(&candidate) {
            return Some(hook);
        }
    }
    None
}

fn completion_hook(id: &str) -> Option<CompletionHook> {
    match id {
        "AGENT_ID" => Some(CompletionHook {
            complete: |cli, current_token| {
                let result = cli.run_json(&["agent", "list"]);
                let Ok(result) = result else {
                    return Vec::new();
                };
                if !result.ok {
                    return Vec::new();
                }
                let Some(workers) = result
                    .json
                    .get("workers")
                    .and_then(|value| value.as_array())
                else {
                    return Vec::new();
                };
                let values = workers
                    .iter()
                    .filter_map(|worker| worker.get("workerName"))
                    .filter_map(|value| value.as_str().map(|value| value.to_string()))
                    .collect::<Vec<_>>();
                filter_by_prefix(values, &current_token)
            },
        }),
        "COMPONENT_NAME" => Some(CompletionHook {
            complete: |cli, current_token| {
                let result = cli.run_json(&["component", "list"]);
                let Ok(result) = result else {
                    return Vec::new();
                };
                if !result.ok {
                    return Vec::new();
                }
                let Some(components) = result.json.as_array() else {
                    return Vec::new();
                };
                let values = components
                    .iter()
                    .filter_map(|component| component.get("componentName"))
                    .filter_map(|value| value.as_str().map(|value| value.to_string()))
                    .collect::<Vec<_>>();
                filter_by_prefix(values, &current_token)
            },
        }),
        _ => None,
    }
}

#[derive(Clone)]
struct GolemCli {
    binary_name: String,
    cwd: PathBuf,
    client_config: ClientConfig,
}

impl GolemCli {
    fn new(binary_name: String, cwd: PathBuf, client_config: ClientConfig) -> Self {
        Self {
            binary_name,
            cwd,
            client_config,
        }
    }

    fn run(&self, opts: RunOptions) -> anyhow::Result<RunResult> {
        let mut command = Command::new(&self.binary_name);
        command
            .current_dir(&self.cwd)
            .env(GOLEM_EVCXR_REPL, "0")
            .arg("--environment")
            .arg(&self.client_config.environment)
            .args(&opts.args);

        match opts.mode {
            RunMode::Inherit => {
                command.stdin(Stdio::inherit());
                command.stdout(Stdio::inherit());
                command.stderr(Stdio::inherit());
                let status = command
                    .status()
                    .with_context(|| "Failed to execute golem cli")?;
                Ok(RunResult {
                    ok: status.success(),
                    code: status.code(),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
            RunMode::Collect => {
                let output = command
                    .output()
                    .with_context(|| "Failed to spawn golem cli")?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                Ok(RunResult {
                    ok: output.status.success(),
                    code: output.status.code(),
                    stdout,
                    stderr,
                })
            }
        }
    }

    fn run_json(&self, args: &[&str]) -> anyhow::Result<RunJsonResult> {
        let mut full_args = vec!["--format".to_string(), "json".to_string()];
        full_args.extend(args.iter().map(|value| value.to_string()));
        let result = self.run(RunOptions {
            args: full_args,
            mode: RunMode::Collect,
        })?;
        let json = serde_json::from_str(&result.stdout).unwrap_or(Value::Null);
        Ok(RunJsonResult {
            ok: result.ok,
            code: result.code,
            json,
        })
    }
}

#[derive(Clone, Copy)]
enum RunMode {
    Inherit,
    Collect,
}

struct RunOptions {
    args: Vec<String>,
    mode: RunMode,
}

struct RunResult {
    ok: bool,
    code: Option<i32>,
    #[allow(unused)]
    stdout: String,
    #[allow(unused)]
    stderr: String,
}

struct RunJsonResult {
    ok: bool,
    #[allow(unused)]
    code: Option<i32>,
    json: Value,
}

fn collect_repl_cli_commands(root: &CliCommandMetadata) -> Vec<ReplCliCommand> {
    let mut commands = Vec::new();
    collect(&mut commands, HashMap::new(), root);
    commands.sort_by(|left, right| left.repl_command.cmp(&right.repl_command));
    commands
}

fn collect(
    commands: &mut Vec<ReplCliCommand>,
    parent_global_flags_args: HashMap<String, CliArgMetadata>,
    command: &CliCommandMetadata,
) {
    let repl_command = command_path_to_repl_command_name(&command.path);
    let about = command
        .about
        .clone()
        .or_else(|| command.long_about.clone())
        .unwrap_or_else(|| command.name.clone());
    let (global_flag_args, mut flag_args, positional_args) = partition_args(&command.args);

    flag_args.insert(
        "--help".to_string(),
        CliArgMetadata {
            id: "help".to_string(),
            help: None,
            long_help: None,
            value_names: Vec::new(),
            value_hint: String::new(),
            possible_values: Vec::new(),
            action: String::new(),
            num_args: None,
            is_positional: false,
            is_required: false,
            is_global: false,
            is_hidden: false,
            index: None,
            long: Vec::new(),
            short: Vec::new(),
            default_values: Vec::new(),
            takes_value: false,
        },
    );

    for (flag_name, flag_arg) in parent_global_flags_args.iter() {
        flag_args.insert(flag_name.clone(), flag_arg.clone());
    }

    let subcommand_global_flag_args = if global_flag_args.is_empty() {
        parent_global_flags_args
    } else {
        let mut merged = parent_global_flags_args;
        merged.extend(global_flag_args);
        merged
    };

    if command.subcommands.is_empty() {
        commands.push(ReplCliCommand {
            repl_command,
            command_path: command.path.clone(),
            about,
            flag_args,
            positional_args,
        });
    } else {
        for subcommand in command.subcommands.iter() {
            collect(commands, subcommand_global_flag_args.clone(), subcommand);
        }
    }
}

fn partition_args(
    args: &[CliArgMetadata],
) -> (
    HashMap<String, CliArgMetadata>,
    HashMap<String, CliArgMetadata>,
    Vec<CliArgMetadata>,
) {
    let mut global_flag_args = HashMap::new();
    let mut flag_args = HashMap::new();
    let mut positional_args = args
        .iter()
        .filter(|arg| arg.is_positional)
        .cloned()
        .collect::<Vec<_>>();
    positional_args.sort_by_key(|arg| arg.index.unwrap_or(0));

    for arg in args.iter() {
        if arg.is_positional {
            continue;
        }

        if !arg.long.is_empty() {
            for long in arg.long.iter() {
                let key = format!("--{long}");
                flag_args.insert(key.clone(), arg.clone());
                if arg.is_global {
                    global_flag_args.insert(key, arg.clone());
                }
            }
        } else {
            for short in arg.short.iter() {
                let key = format!("-{short}");
                flag_args.insert(key.clone(), arg.clone());
                if arg.is_global {
                    global_flag_args.insert(key, arg.clone());
                }
            }
        }
    }

    (global_flag_args, flag_args, positional_args)
}

fn parse_args(command: &ReplCliCommand, tokens: &[String]) -> ParsedArgs {
    let mut used_arg_ids = HashSet::new();
    let mut positional_values = Vec::new();
    let mut expecting_value_for: Option<CliArgMetadata> = None;

    for token in tokens {
        if expecting_value_for.is_some() {
            expecting_value_for = None;
            continue;
        }

        if token == "--" {
            positional_values.extend(
                tokens
                    .iter()
                    .skip_while(|arg| *arg != token)
                    .skip(1)
                    .cloned(),
            );
            break;
        }

        if let Some(flag_arg) = command.flag_args.get(token) {
            used_arg_ids.insert(flag_arg.id.clone());
            if flag_arg.takes_value {
                expecting_value_for = Some(flag_arg.clone());
            }
            continue;
        }

        positional_values.push(token.clone());
    }

    ParsedArgs {
        used_arg_ids,
        positional_values,
        expecting_value_for,
    }
}

fn complete_flags(
    command: &ReplCliCommand,
    used_arg_ids: &HashSet<String>,
    prefix: &str,
) -> Vec<String> {
    let mut flags = Vec::new();
    for (flag, arg) in command.flag_args.iter() {
        let allow_multiple = arg.action == "Append" || arg.action == "Count";
        if !allow_multiple && used_arg_ids.contains(&arg.id) {
            continue;
        }
        flags.push(flag.clone());
    }

    filter_by_prefix(flags, prefix)
}

fn filter_by_prefix(values: Vec<String>, prefix: &str) -> Vec<String> {
    if prefix.is_empty() {
        return unique(values);
    }
    unique(
        values
            .into_iter()
            .filter(|value| value.starts_with(prefix))
            .collect(),
    )
}

fn unique(values: Vec<String>) -> Vec<String> {
    let mut set = HashSet::new();
    let mut unique = Vec::new();
    for value in values {
        if set.insert(value.clone()) {
            unique.push(value);
        }
    }
    unique
}

fn merge_unique(left: Vec<String>, right: Vec<String>) -> Vec<String> {
    let mut set = HashSet::new();
    left.into_iter().for_each(|value| {
        set.insert(value);
    });
    right.into_iter().for_each(|value| {
        set.insert(value);
    });
    set.into_iter().collect()
}

fn command_path_to_repl_command_name(segments: &[String]) -> String {
    let parts: Vec<String> = segments
        .iter()
        .flat_map(|segment| segment.split(&['-', '_'][..]))
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .collect();

    let mut result = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index == 0 {
            result.push_str(&part.to_lowercase());
        } else {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                result.push(first.to_ascii_uppercase());
                result.push_str(&chars.as_str().to_ascii_lowercase());
            }
        }
    }
    result
}

fn parse_raw_args(raw_args: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaping = false;
    let mut in_agent = false;
    let mut agent_depth = 0;
    let mut agent_in_single = false;
    let mut agent_in_double = false;
    let mut agent_escaping = false;

    fn push_current(args: &mut Vec<String>, current: &mut String) {
        if !current.is_empty() {
            args.push(std::mem::take(current));
        }
    }

    fn is_ident_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
    }

    let chars: Vec<char> = raw_args.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];

        if in_agent {
            current.push(ch);

            if agent_escaping {
                agent_escaping = false;
                i += 1;
                continue;
            }

            if agent_in_single {
                if ch == '\'' {
                    agent_in_single = false;
                }
                i += 1;
                continue;
            }

            if agent_in_double {
                if ch == '\\' {
                    agent_escaping = true;
                } else if ch == '"' {
                    agent_in_double = false;
                }
                i += 1;
                continue;
            }

            if ch == '\'' {
                agent_in_single = true;
                i += 1;
                continue;
            }
            if ch == '"' {
                agent_in_double = true;
                i += 1;
                continue;
            }

            if ch == '(' {
                agent_depth += 1;
            } else if ch == ')' {
                agent_depth -= 1;
                if agent_depth == 0 {
                    in_agent = false;
                }
            }
            i += 1;
            continue;
        }

        if escaping {
            current.push(ch);
            escaping = false;
            i += 1;
            continue;
        }

        if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
            i += 1;
            continue;
        }

        if in_double {
            if ch == '\\' {
                escaping = true;
            } else if ch == '"' {
                in_double = false;
            } else {
                current.push(ch);
            }
            i += 1;
            continue;
        }

        if ch.is_whitespace() {
            push_current(&mut args, &mut current);
            i += 1;
            continue;
        }

        if ch == '\'' {
            in_single = true;
            i += 1;
            continue;
        }

        if ch == '"' {
            in_double = true;
            i += 1;
            continue;
        }

        if ch == '\\' {
            escaping = true;
            i += 1;
            continue;
        }

        if current.is_empty() && is_ident_char(ch) {
            let mut j = i;
            while j < chars.len() && is_ident_char(chars[j]) {
                j += 1;
            }
            if j < chars.len() && chars[j] == '(' {
                current.extend(chars[i..j].iter());
                current.push('(');
                in_agent = true;
                agent_depth = 1;
                i = j + 1;
                continue;
            }
        }

        current.push(ch);
        i += 1;
    }

    push_current(&mut args, &mut current);
    args
}

fn exit_with_reload_code() -> anyhow::Result<()> {
    std::process::exit(75);
}
