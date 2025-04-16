// Copyright 2024-2025 Golem Cloud
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

use crate::fuzzy::Match;
use crate::log::{log_warn_action, logln, LogColorize, LogIndent};
use crate::model::{Format, WorkerNameMatch};
use cli_table::{Row, Title, WithTitle};
use colored::control::SHOULD_COLORIZE;
use colored::Colorize;
use golem_client::model::{InitialComponentFile, WorkerStatus};
use itertools::Itertools;
use regex::Regex;
use std::collections::BTreeMap;

pub trait TextView {
    fn log(&self);
}

pub trait MessageWithFields {
    fn message(&self) -> String;
    fn fields(&self) -> Vec<(String, String)>;

    fn indent_fields() -> bool {
        false
    }

    fn nest_ident_fields() -> bool {
        false
    }

    fn format_field_name(name: String) -> String {
        name
    }
}

impl<T: MessageWithFields> TextView for T {
    fn log(&self) {
        logln(self.message());
        if !Self::nest_ident_fields() {
            logln("");
        }

        let fields = self.fields();
        let padding = fields.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 1;

        let _indent = Self::indent_fields().then(LogIndent::new);
        let _nest_indent =
            Self::nest_ident_fields().then(|| NestedTextViewIndent::new(Format::Text));

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
                    logln(format!("  {}", line))
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

pub struct NestedTextViewIndent {
    format: Format,
    log_indent: Option<LogIndent>,
}

impl NestedTextViewIndent {
    pub fn new(format: Format) -> Self {
        match format {
            Format::Json | Format::Yaml => Self {
                format,
                log_indent: Some(LogIndent::new()),
            },
            Format::Text => {
                logln("╔═");
                Self {
                    format,
                    log_indent: Some(LogIndent::prefix("║ ")),
                }
            }
        }
    }
}

impl Drop for NestedTextViewIndent {
    fn drop(&mut self) {
        if let Some(ident) = self.log_indent.take() {
            drop(ident);
            match self.format {
                Format::Json | Format::Yaml => {
                    // NOP
                }
                Format::Text => logln("╚═"),
            }
        }
    }
}

pub fn format_worker_name_match(worker_name_match: &WorkerNameMatch) -> String {
    format!(
        "{}{}{}/{}",
        match &worker_name_match.account_id {
            Some(account_id) => {
                format!("{}/", account_id.0.blue().bold())
            }
            None => "".to_string(),
        },
        match &worker_name_match.project {
            Some(project) => {
                format!("{}/", project.project_name.0.blue().bold())
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
