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

use crate::fuzzy::Match;
use crate::log::{log_warn_action, logln, LogColorize, LogIndent};
use crate::model::deploy_diff::DiffSerialize;
use crate::model::text::component::is_sensitive_env_var_name;
use crate::model::{Format, WorkerNameMatch};
use anyhow::Context;
use cli_table::{Row, Title, WithTitle};
use colored::control::SHOULD_COLORIZE;
use colored::Colorize;
use golem_client::model::{InitialComponentFile, WorkerStatus};
use itertools::Itertools;
use regex::Regex;
use similar::TextDiff;
use std::collections::BTreeMap;

pub trait TextView {
    fn log(&self);
}

pub enum MessageWithFieldsIndentMode {
    None,
    IdentFields,
    NestedIdentAll,
}

pub trait MessageWithFields {
    fn message(&self) -> String;
    fn fields(&self) -> Vec<(String, String)>;

    fn indent_mode() -> MessageWithFieldsIndentMode {
        MessageWithFieldsIndentMode::NestedIdentAll
    }

    fn format_field_name(name: String) -> String {
        name
    }
}

impl<T: MessageWithFields> TextView for T {
    fn log(&self) {
        let _ident = match Self::indent_mode() {
            MessageWithFieldsIndentMode::None => None,
            MessageWithFieldsIndentMode::IdentFields => None,
            MessageWithFieldsIndentMode::NestedIdentAll => {
                Some(NestedTextViewIndent::new(Format::Text))
            }
        };

        logln(self.message());
        logln("");

        let fields = self.fields();
        let padding = fields.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 1;

        let _indent = match Self::indent_mode() {
            MessageWithFieldsIndentMode::None => None,
            MessageWithFieldsIndentMode::IdentFields => Some(LogIndent::new()),
            MessageWithFieldsIndentMode::NestedIdentAll => None,
        };

        for (name, value) in self.fields() {
            let lines: Vec<_> = value.split("\n").collect();
            if lines.len() == 1 {
                logln(format!(
                    "{:<padding$} {}",
                    format!("{}:", Self::format_field_name(name)),
                    lines[0]
                ));
            } else {
                logln(format!("{}:", Self::format_field_name(name)));
                for line in lines {
                    logln(format!("  {line}"))
                }
            }
        }
    }
}

pub struct FieldsBuilder(Vec<(String, String)>);

impl FieldsBuilder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(vec![])
    }

    pub fn field<T: ToString>(&mut self, name: &str, value: &T) -> &mut Self {
        self.0.push((name.to_string(), value.to_string()));
        self
    }

    pub fn fmt_field<T: ?Sized>(
        &mut self,
        name: &str,
        value: &T,
        format: impl Fn(&T) -> String,
    ) -> &mut Self {
        self.0.push((name.to_string(), format(value)));
        self
    }

    pub fn fmt_field_optional<T: ?Sized>(
        &mut self,
        name: &str,
        value: &T,
        cond: bool,
        format: impl Fn(&T) -> String,
    ) -> &mut Self {
        if cond {
            self.0.push((name.to_string(), format(value)));
        }
        self
    }

    pub fn fmt_field_option<T>(
        &mut self,
        name: &str,
        value: &Option<T>,
        format: impl Fn(&T) -> String,
    ) -> &mut Self {
        if let Some(value) = &value {
            self.0.push((name.to_string(), format(value)));
        }
        self
    }

    pub fn build(self) -> Vec<(String, String)> {
        self.0
    }
}

pub fn format_main_id<T: ToString + ?Sized>(id: &T) -> String {
    id.to_string().bold().underline().to_string()
}

pub fn format_id<T: ToString + ?Sized>(id: &T) -> String {
    id.to_string().bold().to_string()
}

pub fn format_warn<T: ToString + ?Sized>(s: &T) -> String {
    s.to_string().yellow().to_string()
}

pub fn format_message_highlight<T: ToString + ?Sized>(s: &T) -> String {
    s.to_string().green().bold().to_string()
}

pub fn format_stack(stack: &str) -> String {
    stack
        .lines()
        .map(|line| {
            if line.contains("<unknown>!<wasm function") {
                line.bright_black().to_string()
            } else {
                line.yellow().to_string()
            }
        })
        .join("\n")
}

pub fn format_error(error: &str) -> String {
    if error.contains("error while executing at wasm backtrace") {
        format_stack(error)
    } else {
        error.yellow().to_string()
    }
}

pub fn format_binary_size(size: &u64) -> String {
    humansize::format_size(*size, humansize::BINARY)
}

pub fn format_status(status: &WorkerStatus) -> String {
    let status_name = status.to_string();
    match status {
        WorkerStatus::Running => status_name.green(),
        WorkerStatus::Idle => status_name.cyan(),
        WorkerStatus::Suspended => status_name.yellow(),
        WorkerStatus::Interrupted => status_name.red(),
        WorkerStatus::Retrying => status_name.yellow(),
        WorkerStatus::Failed => status_name.bright_red(),
        WorkerStatus::Exited => status_name.white(),
    }
    .to_string()
}

pub fn format_retry_count(retry_count: &u64) -> String {
    if *retry_count == 0 {
        retry_count.to_string()
    } else {
        format_warn(&retry_count.to_string())
    }
}

static BUILTIN_TYPES: phf::Set<&'static str> = phf::phf_set! {
    "bool",
    "s8", "s16", "s32", "s64",
    "u8", "u16", "u32", "u64",
    "f32", "f64",
    "char",
    "string",
    "list",
    "option",
    "result",
    "tuple",
    "record",
};

// A very naive highlighter for basic coloring of builtin types and user defined names
pub fn format_export(export: &str) -> String {
    if !SHOULD_COLORIZE.should_colorize() {
        return export.to_string();
    }

    let separator =
        Regex::new(r"[ :/.{}()<>,]").expect("Failed to compile export separator pattern");
    let mut formatted = String::with_capacity(export.len());

    fn format_token(target: &mut String, token: &str) {
        let trimmed_token = token.trim_ascii_start();
        let starts_with_ascii = trimmed_token
            .chars()
            .next()
            .map(|c| c.is_ascii())
            .unwrap_or(false);
        if starts_with_ascii {
            if BUILTIN_TYPES.contains(trimmed_token) {
                target.push_str(&token.green().to_string());
            } else {
                target.push_str(&token.cyan().to_string());
            }
        } else {
            target.push_str(token);
        }
    }

    let mut last_end = 0;
    for separator in separator.find_iter(export) {
        if separator.start() != last_end {
            format_token(&mut formatted, &export[last_end..separator.start()]);
        }
        formatted.push_str(separator.as_str());
        last_end = separator.end();
    }
    if last_end != export.len() {
        format_token(&mut formatted, &export[last_end..])
    }

    formatted
}

pub fn format_exports(exports: &[String]) -> String {
    exports.iter().map(|e| format_export(e.as_str())).join("\n")
}

pub fn format_dynamic_links(links: &BTreeMap<String, BTreeMap<String, String>>) -> String {
    links
        .iter()
        .map(|(name, link)| {
            let padding = link.keys().map(|name| name.len()).max().unwrap_or_default() + 1;

            format!(
                "{}:\n{}",
                name,
                link.iter()
                    .map(|(resource, interface)| format!(
                        "  {:<padding$} {}",
                        format!("{}:", resource),
                        format_export(interface)
                    ))
                    .join("\n")
            )
        })
        .join("\n")
}

pub fn format_ifs_entry(files: &[InitialComponentFile]) -> String {
    files
        .iter()
        .map(|file| {
            format!(
                "{} {} {}",
                file.permissions.as_compact_str(),
                file.path.as_path().as_str().log_color_highlight(),
                file.key.0.as_str().black()
            )
        })
        .join("\n")
}

pub fn format_env(show_sensitive: bool, env: &BTreeMap<String, String>) -> String {
    let hidden = "*****".log_color_highlight();
    env.iter()
        .map(|(k, v)| {
            if is_sensitive_env_var_name(show_sensitive, k) {
                format!("{k}={hidden}")
            } else {
                format!("{}={}", k, v.log_color_highlight())
            }
        })
        .join("\n")
}

pub fn format_table<E, R>(table: &[E]) -> String
where
    R: Title + 'static + for<'b> From<&'b E>,
    for<'a> &'a R: Row,
{
    let rows: Vec<R> = table.iter().map(R::from).collect();
    let rows = &rows;

    format!(
        "{}",
        rows.with_title()
            .display()
            .expect("Failed to display table")
    )
}

pub fn log_table<E, R>(table: &[E])
where
    R: Title + 'static + for<'b> From<&'b E>,
    for<'a> &'a R: Row,
{
    logln(format_table(table));
}

pub fn log_text_view<View: TextView>(view: &View) {
    view.log();
}

pub fn log_error<S: AsRef<str>>(message: S) {
    logln(format!(
        "{} {}",
        "error:".log_color_error(),
        message.as_ref()
    ));
}

pub fn log_warn<S: AsRef<str>>(message: S) {
    logln(format!("{} {}", "warn:".log_color_warn(), message.as_ref()));
}

pub fn log_fuzzy_matches(matches: &[Match]) {
    for m in matches {
        if !m.exact_match {
            log_fuzzy_match(m);
        }
    }
}

pub fn log_fuzzy_match(m: &Match) {
    log_warn_action(
        "Fuzzy matched",
        format!(
            "pattern {} as {}",
            m.pattern.log_color_highlight(),
            m.option.log_color_ok_highlight()
        ),
    );
}

pub fn log_deploy_diff<T: DiffSerialize>(server: &T, manifest: &T) -> anyhow::Result<()> {
    let server = server
        .to_diffable_string()
        .context("failed to serialize server entity")?;
    let manifest = manifest
        .to_diffable_string()
        .context("failed to serialize manifest entity")?;

    log_unified_diff(
        &TextDiff::from_lines(&server, &manifest)
            .unified_diff()
            .context_radius(4)
            .header("sever", "manifest")
            .to_string(),
    );

    Ok(())
}

pub fn log_unified_diff(diff: &str) {
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            logln(line.green().bold().to_string());
        } else if line.starts_with('-') && !line.starts_with("---") {
            logln(line.red().bold().to_string());
        } else if line.starts_with("@@") {
            logln(line.bold().to_string());
        } else {
            logln(line);
        }
    }
}

pub fn format_rib_source_for_error(source: &str, error: &str) -> String {
    const CONTEXT_SIZE: usize = 3;
    const LINE_COUNT_PADDING: usize = 4;

    let parse_error_at_line_regex =
        Regex::new("Parse error at line: (\\d+), column: (\\d+)").unwrap();

    let source_info = match parse_error_at_line_regex.captures(error) {
        Some(captures) => match (captures[1].parse::<usize>(), captures[2].parse::<usize>()) {
            (Ok(line), Ok(column)) => Some((line, Some(column))),
            _ => None,
        },
        None => None,
    };

    match source_info {
        Some((err_line, err_column)) => {
            let from = err_line.saturating_sub(CONTEXT_SIZE);
            let to = err_line.saturating_add(CONTEXT_SIZE);

            source
                .lines()
                .enumerate()
                .filter_map(|(idx, line)| {
                    let line_no = idx + 1;
                    if from <= line_no && line_no <= to {
                        Some(if line_no == err_line {
                            let underline = format!(
                                " {: >LINE_COUNT_PADDING$} | {}",
                                "",
                                match err_column {
                                    Some(err_column) => {
                                        let padding = err_column - 1;
                                        format!("{: >padding$}^", "").red()
                                    }
                                    None => {
                                        "^".repeat(line.len()).red().bold()
                                    }
                                }
                            );
                            format!(
                                "{}{: >LINE_COUNT_PADDING$} | {}\n{}",
                                ">".red().bold(),
                                line_no,
                                line.red().bold(),
                                underline
                            )
                        } else {
                            format!(" {line_no: >LINE_COUNT_PADDING$} | {line}")
                        })
                    } else {
                        None
                    }
                })
                .join("\n")
        }
        None => source
            .lines()
            .enumerate()
            .map(|(idx, line)| format!(" {: >LINE_COUNT_PADDING$} | {}", idx + 1, line))
            .join("\n"),
    }
}

pub struct NestedTextViewIndent {
    decorated: bool,
    log_indent: Option<LogIndent>,
}

impl NestedTextViewIndent {
    pub fn new(format: Format) -> Self {
        match format {
            Format::Text if SHOULD_COLORIZE.should_colorize() => {
                logln("╔═");
                Self {
                    decorated: true,
                    log_indent: Some(LogIndent::prefix("║ ")),
                }
            }
            _ => Self {
                decorated: false,
                log_indent: Some(LogIndent::new()),
            },
        }
    }
}

impl Drop for NestedTextViewIndent {
    fn drop(&mut self) {
        if let Some(ident) = self.log_indent.take() {
            drop(ident);
            if self.decorated {
                logln("╚═");
            }
        }
    }
}

pub fn format_worker_name_match(worker_name_match: &WorkerNameMatch) -> String {
    format!(
        "{}{}{}/{}",
        match &worker_name_match.account {
            Some(account) => {
                format!("{}/", account.email.blue().bold())
            }
            None => "".to_string(),
        },
        match &worker_name_match.project {
            Some(project) => {
                format!("{}/", project.project_ref.to_string().blue().bold())
            }
            None => "".to_string(),
        },
        worker_name_match.component_name.0.blue().bold(),
        worker_name_match
            .worker_name
            .as_ref()
            .map(|wn| wn.0.as_str())
            .unwrap_or("-")
            .green()
            .bold(),
    )
}
