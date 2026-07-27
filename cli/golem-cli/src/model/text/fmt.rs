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

use crate::fuzzy::Match;
pub use crate::log::log_table;
pub use crate::log::logln;
pub use crate::log::terminal_width;
use crate::log::{
    INDENT, LogColorize, LogIndent, WRAP_PADDING, current_indent_width, log_warn_action,
};
use crate::model::app::ComponentLayerId;
use crate::model::format::Format;
use crate::model::masking::{Masked, MaskingConfig};
use anyhow::anyhow;
use colored::Colorize;
use colored::control::SHOULD_COLORIZE;
pub use comfy_table::Table as ComfyTable;
use comfy_table::{
    Cell, CellAlignment, Color as ComfyColor, ColumnConstraint, ContentArrangement, Width,
};
use golem_common::model::AgentStatus;
use golem_common::model::component::{InitialAgentFile, InstalledPlugin};
use golem_common::model::worker::TypedAgentConfigEntry;
use itertools::Itertools;
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;
use synoptic::TokOpt;

pub trait TextOutput {
    fn log(&self) {}

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        // Most text views do not contain secret-bearing fields. Views that can
        // include known secrets or sensitive user values must override this and
        // render from a masked representation before writing any text.
        let _ = config;
        self.log();
        Ok(())
    }
}

pub trait NoTextOutput {}

pub trait TruncatableTextOutput: TextOutput {
    fn render_truncated(&self, max_lines: usize, colorize: bool) -> String;

    // Truncated rendering returns an already-rendered string, so there is no
    // safe generic post-processing default. Each implementation must decide
    // whether it carries sensitive data and render from masked data if needed.
    fn render_truncated_masked(
        &self,
        max_lines: usize,
        colorize: bool,
        config: MaskingConfig,
    ) -> anyhow::Result<String>;
}

/// Truncates a pre-rendered string to `max_lines` terminal lines.
/// If truncated, appends a notice: `... N more lines (resize terminal to see all)`.
pub fn truncate_rendered(rendered: String, max_lines: usize) -> String {
    let lines: Vec<&str> = rendered.lines().collect();
    if lines.len() <= max_lines {
        rendered
    } else {
        let shown = max_lines.saturating_sub(1);
        let mut out = lines[..shown].join("\n");
        out.push('\n');
        out.push_str(&format!(
            "... {} more lines (resize terminal to see all)",
            lines.len() - shown
        ));
        out
    }
}

pub enum MessageWithFieldsIndentMode {
    None,
    IdentFields,
    NestedIdentAll,
}

pub trait MessageWithFields: Masked {
    fn message(&self) -> String;
    fn fields(&self) -> Vec<(String, String)>;

    fn fields_masked(self, config: MaskingConfig) -> anyhow::Result<Vec<(String, String)>>
    where
        Self: Sized,
    {
        Ok(self.masked(config)?.fields())
    }

    fn indent_mode() -> MessageWithFieldsIndentMode {
        MessageWithFieldsIndentMode::NestedIdentAll
    }

    fn format_field_name(name: String) -> String {
        name
    }
}

impl<T: MessageWithFields> TextOutput for T {
    fn log(&self) {
        log_message_with_fields::<T>(self.message(), self.fields());
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        let message = self.message();
        log_message_with_fields::<T>(message, self.fields_masked(config)?);
        Ok(())
    }
}

/// Columns a multi-line field value can use. `fields()` builds values before the
/// indents they print inside exist, so all are subtracted here: the ambient
/// indent, the view's own indent, the per-line indent of a multi-line value, and
/// [`WRAP_PADDING`] (reaching into which gets the value wrapped again).
pub fn field_value_width<T: MessageWithFields>() -> usize {
    let view_indent = match T::indent_mode() {
        MessageWithFieldsIndentMode::None => 0,
        MessageWithFieldsIndentMode::IdentFields | MessageWithFieldsIndentMode::NestedIdentAll => {
            INDENT.len()
        }
    };

    (terminal_width() as usize)
        .saturating_sub(current_indent_width() + view_indent + INDENT.len() + WRAP_PADDING)
}

fn log_message_with_fields<T: MessageWithFields>(message: String, fields: Vec<(String, String)>) {
    let _ident = match T::indent_mode() {
        MessageWithFieldsIndentMode::None => None,
        MessageWithFieldsIndentMode::IdentFields => None,
        MessageWithFieldsIndentMode::NestedIdentAll => {
            Some(DecoratedIndent::new_primary(Format::Text))
        }
    };

    logln(message);
    logln("");

    let padding = fields.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 1;

    let _indent = match T::indent_mode() {
        MessageWithFieldsIndentMode::None => None,
        MessageWithFieldsIndentMode::IdentFields => Some(LogIndent::new()),
        MessageWithFieldsIndentMode::NestedIdentAll => None,
    };

    for (name, value) in fields {
        let lines: Vec<_> = value.split("\n").collect();
        if lines.len() == 1 {
            logln(format!(
                "{:<padding$} {}",
                format!("{}:", T::format_field_name(name)),
                lines[0]
            ));
        } else {
            logln(format!("{}:", T::format_field_name(name)));
            for line in lines {
                logln(format!("{INDENT}{line}"))
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
    stack.lines().map(format_worker_error_line).join("\n")
}

pub fn format_error(error: &str) -> String {
    if error.contains("error while executing at wasm backtrace") {
        format_stack(error)
    } else {
        error.yellow().to_string()
    }
}

pub fn format_stderr(stderr: &str) -> String {
    stderr
        .lines()
        .map(|line| {
            if line.starts_with("JavaScript exception:")
                || line.starts_with("JavaScript error:")
                || line.starts_with("Error:")
            {
                line.red().bold().to_string()
            } else if is_wasm_frame(line) || line.contains("RUST_BACKTRACE=1") {
                line.bright_black().to_string()
            } else {
                line.yellow().to_string()
            }
        })
        .join("\n")
}

fn format_worker_error_line(line: &str) -> String {
    if line.contains("called without being linked with an implementation") {
        line.red().bold().to_string()
    } else if is_wasm_frame(line) {
        line.bright_black().to_string()
    } else {
        line.yellow().to_string()
    }
}

fn is_wasm_frame(line: &str) -> bool {
    line.contains("<unknown>!<wasm function") || line.contains("agent_guest.wasm!")
}

pub fn format_binary_size(size: &u64) -> String {
    humansize::format_size(*size, humansize::BINARY)
}

pub fn format_status(status: &AgentStatus) -> String {
    let status_name = status.to_string();
    match status {
        AgentStatus::Running => status_name.green(),
        AgentStatus::Idle => status_name.cyan(),
        AgentStatus::Suspended => status_name.yellow(),
        AgentStatus::Interrupted => status_name.red(),
        AgentStatus::Retrying => status_name.yellow(),
        AgentStatus::Failed => status_name.bright_red(),
        AgentStatus::Exited => status_name.white(),
    }
    .to_string()
}

pub fn format_retry_count(retry_count: &u32) -> String {
    if *retry_count == 0 {
        retry_count.to_string()
    } else {
        format_warn(&retry_count.to_string())
    }
}

static BUILTIN_TYPES: phf::Set<&'static str> = phf::phf_set! {
    // WIT primitives
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
    // Rust types
    "String",
    "Option",
    "Vec",
    "Result",
    "Some",
    "None",
    "i8", "i16", "i32", "i64",
    "enum",
    "flags",
    // TypeScript types
    "number",
    "boolean",
    "undefined",
    "Uint8Array",
    "void",
    "never",
    "true",
};

// A naive highlighter for basic coloring of builtin types and user defined names
pub fn format_export(export: &str) -> String {
    if !SHOULD_COLORIZE.should_colorize() {
        return export.to_string();
    }

    let separator = Regex::new(r#"[\s:/.{}()\[\]<>,;|?"]"#)
        .expect("Failed to compile export separator pattern");
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

pub fn format_files(files: &[InitialAgentFile]) -> String {
    files
        .iter()
        .map(|file| {
            format!(
                "{} {} {}",
                file.permissions.as_compact_str(),
                file.path.as_path().as_str().log_color_highlight(),
                file.content_hash.0.to_string().black()
            )
        })
        .join("\n")
}

pub fn format_plugins(plugins: &[InstalledPlugin]) -> String {
    plugins
        .iter()
        .map(|plugin| {
            let plugin_id = format!(
                "{}: {}/{}",
                plugin.priority,
                plugin.plugin_name.log_color_highlight(),
                plugin.plugin_version.log_color_highlight(),
            );

            if plugin.parameters.is_empty() {
                plugin_id
            } else {
                format!(
                    "{}:\n{}",
                    plugin_id,
                    plugin
                        .parameters
                        .iter()
                        .map(|(k, v)| format!("  {}={}", k, v))
                        .join("\n")
                )
            }
        })
        .join("\n")
}

pub fn format_env(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(k, v)| format!("{}={}", k, v.log_color_highlight()))
        .join("\n")
}

pub fn format_typed_config(config: &[TypedAgentConfigEntry]) -> String {
    config
        .iter()
        .map(|entry| {
            let key = entry.path.join(".");
            let value = golem_common::schema::render::to_json_value(
                entry.value.graph(),
                entry.value.root_type(),
                entry.value.value(),
            )
            .map(|v| v.to_string())
            .unwrap_or_else(|_| "<invalid>".to_string());
            format!("{}={}", key.log_color_highlight(), value)
        })
        .join("\n")
}

/// Describes a single table column: its header title, whether it is pinned to content
/// width, and whether its data rows should be right-aligned.
pub struct Column {
    title: String,
    right_aligned: bool,
    width: ColumnWidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnWidth {
    Auto,
    Content,
    Exact(u16),
    Min(u16),
    Max(u16),
    Range { min: u16, max: u16 },
}

impl Column {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            right_aligned: false,
            width: ColumnWidth::Auto,
        }
    }

    /// Pin the column to its content width — it will not expand to fill surplus space.
    pub fn content(mut self) -> Self {
        self.width = ColumnWidth::Content;
        self
    }

    /// Backward-compatible alias for content width columns.
    pub fn fixed(self) -> Self {
        self.content()
    }

    /// Right-align the data rows of this column.
    pub fn right(mut self) -> Self {
        self.right_aligned = true;
        self
    }

    /// Pin to content width and right-align — the common case for numeric/fixed columns.
    pub fn content_right(mut self) -> Self {
        self.width = ColumnWidth::Content;
        self.right_aligned = true;
        self
    }

    /// Backward-compatible alias for content width + right alignment.
    pub fn fixed_right(self) -> Self {
        self.content_right()
    }

    /// Set the minimum width for the column.
    pub fn min_width(mut self, min_width: usize) -> Self {
        self.width = ColumnWidth::Min(min_width.min(u16::MAX as usize) as u16);
        self
    }

    /// Set an exact width for the column.
    pub fn exact_width(mut self, width: usize) -> Self {
        self.width = ColumnWidth::Exact(width.min(u16::MAX as usize) as u16);
        self
    }

    /// Set the maximum width for the column.
    pub fn max_width(mut self, max_width: usize) -> Self {
        self.width = ColumnWidth::Max(max_width.min(u16::MAX as usize) as u16);
        self
    }

    /// Set both minimum and maximum width for the column.
    pub fn width_range(mut self, min_width: usize, max_width: usize) -> Self {
        let min = min_width.min(u16::MAX as usize) as u16;
        let max = max_width.min(u16::MAX as usize) as u16;
        self.width = ColumnWidth::Range {
            min: min.min(max),
            max: max.max(min),
        };
        self
    }

    pub fn as_str(&self) -> &str {
        &self.title
    }

    pub fn total_width_for_content_width(content_width: u16) -> u16 {
        content_width.saturating_add(2)
    }
}

impl std::fmt::Display for Column {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.title)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablePreset {
    Full,
    FullCondensed,
}

/// Creates a comfy-table pre-configured with Dynamic arrangement, terminal width, and
/// a preset chosen from the global colorize flag. Column constraints and alignment are
/// applied from the `headers` descriptors.
///
/// The terminal width is automatically reduced by the current log indent width so that
/// tables render correctly when called inside an indented context.
pub fn new_table(preset: TablePreset, headers: Vec<Column>) -> ComfyTable {
    let colorize = SHOULD_COLORIZE.should_colorize();
    let indent_width = current_indent_width();
    let term_width = (terminal_width() as usize).saturating_sub(indent_width) as u16;
    let mut table = ComfyTable::new();
    table
        .load_preset(preset_str(preset, colorize))
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_width(term_width)
        .set_header(
            headers
                .iter()
                .map(|c| Cell::new(&c.title))
                .collect::<Vec<_>>(),
        );
    for (i, col) in headers.iter().enumerate() {
        let column = table.column_mut(i).unwrap();
        apply_column_width(column, col.width);
        if col.right_aligned {
            column.set_cell_alignment(CellAlignment::Right);
        }
    }
    table
}

fn apply_column_width(column: &mut comfy_table::Column, width: ColumnWidth) {
    match width {
        ColumnWidth::Auto => {}
        ColumnWidth::Content => {
            column.set_constraint(ColumnConstraint::ContentWidth);
        }
        ColumnWidth::Exact(width) => {
            column.set_constraint(ColumnConstraint::Absolute(Width::Fixed(
                Column::total_width_for_content_width(width),
            )));
        }
        ColumnWidth::Min(min_width) => {
            column.set_constraint(ColumnConstraint::LowerBoundary(Width::Fixed(min_width)));
        }
        ColumnWidth::Max(max_width) => {
            column.set_constraint(ColumnConstraint::UpperBoundary(Width::Fixed(max_width)));
        }
        ColumnWidth::Range { min, max } => {
            column.set_constraint(ColumnConstraint::Boundaries {
                lower: Width::Fixed(Column::total_width_for_content_width(min)),
                upper: Width::Fixed(Column::total_width_for_content_width(max)),
            });
        }
    }
}

pub fn new_table_full(headers: Vec<Column>) -> ComfyTable {
    new_table(TablePreset::Full, headers)
}

pub fn new_table_full_condensed(headers: Vec<Column>) -> ComfyTable {
    new_table(TablePreset::FullCondensed, headers)
}

/// Space a comfy-table cell reserves around its content: one column on each
/// side. Kept in sync with [`Column::total_width_for_content_width`].
const CELL_PADDING: usize = 2;

/// One cell of a [`self_formatting_table`] row. The cell in the flex column
/// ([`FlexColumn::index`]) carries the raw value and is reformatted to the
/// budgeted width; every other cell is rendered from its text as-is.
pub struct TableCell {
    text: String,
    align_right: bool,
    color: Option<ComfyColor>,
}

impl TableCell {
    /// A cell holding the given text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            align_right: false,
            color: None,
        }
    }

    pub fn right(mut self) -> Self {
        self.align_right = true;
        self
    }

    pub fn color(mut self, color: ComfyColor) -> Self {
        self.color = Some(color);
        self
    }

    fn content_width(&self) -> usize {
        self.text.chars().count()
    }
}

/// The self-formatting column of a [`self_formatting_table`].
pub struct FlexColumn<'a> {
    /// Index of the column among `headers`.
    pub index: usize,
    /// Smallest content width worth formatting to; below it the formatter is
    /// called with `None` and the value rendered as-is for the engine to wrap.
    pub min_width: usize,
    /// Formats a raw cell value to the budgeted content width (`None` = as-is).
    pub format: &'a dyn Fn(&str, Option<usize>) -> String,
}

/// Inputs for [`self_formatting_table`].
pub struct SelfFormattingTableSpec<'a> {
    pub preset: TablePreset,
    pub term_width: u16,
    pub full_width: bool,
    pub headers: Vec<Column>,
    pub flex: FlexColumn<'a>,
    pub rows: Vec<Vec<TableCell>>,
}

/// Builds a table where one column's cells are formatted to a width the builder
/// computes, instead of one the engine derives from the content.
///
/// The normal flow is content → width: cells go in, comfy-table sizes columns
/// after. That breaks for a cell already laid out to a width (a structurally
/// broken agent id) — the engine re-wraps it mid-token. So the flow is inverted:
/// measure the fixed columns, budget the leftover to the flex column, format its
/// cells to exactly that, and pin the column so the engine cannot resize it. The
/// flex column is never wider than its longest cell needs, so it wraps only when
/// the terminal cannot fit it and otherwise uses the full width.
///
/// A non-flex [`ColumnWidth::Range`] column is *soft*: capped at its upper bound,
/// with the lower bound only the budget's shrink-to floor (no hard lower
/// constraint, so the engine frees space for the pinned flex column). Other
/// width kinds keep their usual meaning.
pub fn self_formatting_table(spec: SelfFormattingTableSpec) -> ComfyTable {
    let flex_width = flex_content_width(&spec);

    let colorize = SHOULD_COLORIZE.should_colorize();
    let mut table = ComfyTable::new();
    table
        .load_preset(preset_str(spec.preset, colorize))
        .set_content_arrangement(if spec.full_width {
            ContentArrangement::DynamicFullWidth
        } else {
            ContentArrangement::Dynamic
        })
        .set_width(spec.term_width)
        .set_header(
            spec.headers
                .iter()
                .map(|header| Cell::new(&header.title))
                .collect::<Vec<_>>(),
        );

    for (index, header) in spec.headers.iter().enumerate() {
        let column = table.column_mut(index).unwrap();
        if index == spec.flex.index {
            match flex_width {
                // Pin it so the engine cannot shrink it and re-wrap the cells.
                Some(width) => {
                    column.set_constraint(ColumnConstraint::Absolute(Width::Fixed(
                        Column::total_width_for_content_width(width as u16),
                    )));
                }
                None => apply_column_width(column, header.width),
            }
        } else if let ColumnWidth::Range { max, .. } = header.width {
            // Soft column: upper bound only, no hard lower one, so the engine
            // shrinks it freely to fit the pinned flex column.
            column.set_constraint(ColumnConstraint::UpperBoundary(Width::Fixed(
                Column::total_width_for_content_width(max),
            )));
        } else {
            apply_column_width(column, header.width);
        }
        if header.right_aligned {
            column.set_cell_alignment(CellAlignment::Right);
        }
    }

    for row in &spec.rows {
        table.add_row(
            row.iter()
                .enumerate()
                .map(|(index, cell)| {
                    // `flex.index` is the single source of truth for which cell is
                    // reformatted, shared with the constraint above.
                    let text = if index == spec.flex.index {
                        (spec.flex.format)(&cell.text, flex_width)
                    } else {
                        cell.text.clone()
                    };
                    let mut comfy = Cell::new(text);
                    if cell.align_right {
                        comfy = comfy.set_alignment(CellAlignment::Right);
                    }
                    if let Some(color) = cell.color {
                        comfy = comfy.fg(color);
                    }
                    comfy
                })
                .collect::<Vec<_>>(),
        );
    }

    table
}

/// Budgeted content width for the flex column, or `None` when too little room is
/// left to format structurally.
///
/// Other columns are subtracted at their content width (capped by any
/// `Max`/`Range` upper bound); when that starves the flex column, `Range` columns
/// are assumed to shrink to their lower bound. This assumes non-flex columns
/// render at their content width — true for `Content`/`Exact`/`Range` but not
/// `Min`/`Auto`, which can grow past content and overshoot.
fn flex_content_width(spec: &SelfFormattingTableSpec) -> Option<usize> {
    let borders = spec.headers.len() + 1;
    let flex_min = spec.flex.min_width + CELL_PADDING;

    let mut fixed_total = 0usize;
    let mut reclaimable = 0usize;
    for (index, header) in spec.headers.iter().enumerate() {
        if index == spec.flex.index {
            continue;
        }
        let content = column_content_width(spec, index);
        let effective = match header.width {
            ColumnWidth::Exact(width) => width as usize,
            ColumnWidth::Max(max) | ColumnWidth::Range { max, .. } => content.min(max as usize),
            _ => content,
        } + CELL_PADDING;
        if let ColumnWidth::Range { min, .. } = header.width {
            reclaimable += effective.saturating_sub(min as usize + CELL_PADDING);
        }
        fixed_total += effective;
    }

    let mut budget = (spec.term_width as usize).checked_sub(fixed_total + borders)?;
    if budget < flex_min {
        budget += reclaimable.min(flex_min - budget);
    }

    // Never wider than the longest cell needs: claiming more would only pad the
    // table out with empty space.
    let needed = column_content_width(spec, spec.flex.index) + CELL_PADDING;
    let budget = budget.min(needed);

    (budget >= flex_min).then(|| budget - CELL_PADDING)
}

/// Widest content in a column, including its header title.
fn column_content_width(spec: &SelfFormattingTableSpec, index: usize) -> usize {
    spec.rows
        .iter()
        .filter_map(|row| row.get(index))
        .map(TableCell::content_width)
        .chain(std::iter::once(spec.headers[index].title.chars().count()))
        .max()
        .unwrap_or(0)
}

fn preset_str(preset: TablePreset, colorize: bool) -> &'static str {
    use comfy_table::presets::{ASCII_FULL, ASCII_FULL_CONDENSED, UTF8_FULL, UTF8_FULL_CONDENSED};
    match (preset, colorize) {
        (TablePreset::Full, true) => UTF8_FULL,
        (TablePreset::FullCondensed, true) => UTF8_FULL_CONDENSED,
        (TablePreset::Full, false) => ASCII_FULL,
        (TablePreset::FullCondensed, false) => ASCII_FULL_CONDENSED,
    }
}

pub fn log_text_view<View: TextOutput>(view: &View) {
    view.log();
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

pub struct DecoratedIndent {
    close_line: Option<String>,
    log_indent: Option<LogIndent>,
}

impl DecoratedIndent {
    pub fn new_primary(format: Format) -> Self {
        match format {
            Format::Text if SHOULD_COLORIZE.should_colorize() => {
                logln("╔═");
                Self {
                    close_line: Some("╚═".to_string()),
                    log_indent: Some(LogIndent::prefix("║ ")),
                }
            }
            _ => Self {
                close_line: None,
                log_indent: Some(LogIndent::new()),
            },
        }
    }

    pub fn new_secondary(format: Format) -> Self {
        match format {
            Format::Text if SHOULD_COLORIZE.should_colorize() => {
                logln("┏━".bright_black().bold().to_string());
                Self {
                    close_line: Some("┗━".bright_black().bold().to_string()),
                    log_indent: Some(LogIndent::prefix(format!("{} ", "┃".bright_black().bold()))),
                }
            }
            _ => Self {
                close_line: None,
                log_indent: Some(LogIndent::new()),
            },
        }
    }
}

impl Drop for DecoratedIndent {
    fn drop(&mut self) {
        if let Some(ident) = self.log_indent.take() {
            drop(ident);
            if let Some(close_line) = self.close_line.take() {
                logln(close_line);
            }
        }
    }
}

pub fn to_colored_json<T: Serialize>(value: &T) -> anyhow::Result<String> {
    let mut highlighter =
        synoptic::from_extension("js", 2).ok_or_else(|| anyhow!("Failed to get JS highlighter"))?;

    let serialized_lines: Vec<String> = serde_json::to_string_pretty(value)?
        .lines()
        .map(|line| line.to_string())
        .collect();

    highlighter.run(serialized_lines.as_slice());

    let mut output = String::new();

    for (idx, line) in serialized_lines.iter().enumerate() {
        let lines = highlighter.line(idx, line);
        let mut tokens = lines.iter().peekable();
        while let Some(token) = tokens.next() {
            match token {
                TokOpt::Some(text, kind) => {
                    let mut style_kind = kind.as_str();

                    if kind == "string"
                        && let Some(TokOpt::None(next)) = tokens.peek()
                        && next.trim_start().starts_with(':')
                    {
                        style_kind = "key";
                    }

                    match style_kind {
                        "key" => write!(output, "{}", text.blue().bold())?,
                        "string" => write!(output, "{}", text.green())?,
                        "keyword" => write!(output, "{}", text.magenta().bold())?,
                        "digit" => write!(output, "{}", text.cyan())?,
                        "boolean" => write!(output, "{}", text.yellow())?,
                        _ => write!(output, "{}", text)?,
                    }
                }
                TokOpt::None(text) => {
                    write!(output, "{}", text)?;
                }
            }
        }
        output.push('\n');
    }

    Ok(output)
}

pub fn to_colored_yaml<T: Serialize>(value: &T) -> anyhow::Result<String> {
    let mut highlighter = synoptic::from_extension("yaml", 2)
        .ok_or_else(|| anyhow!("Failed to get YAML highlighter"))?;

    let serialized_lines: Vec<String> = serde_yaml::to_string(value)?
        .lines()
        .map(|line| line.to_string())
        .collect();

    highlighter.run(serialized_lines.as_slice());

    let mut output = String::new();

    for (idx, line) in serialized_lines.iter().enumerate() {
        for token in highlighter.line(idx, line) {
            match token {
                TokOpt::Some(text, kind) => match kind.as_str() {
                    "string" => write!(output, "{}", text.green())?,
                    "comment" => write!(output, "{}", text.yellow())?,
                    "key" => write!(output, "{}", text.blue().bold())?,
                    "digit" => write!(output, "{}", text.cyan())?,
                    "tag" => write!(output, "{}", text.magenta().bold())?,
                    _ => write!(output, "{}", text)?,
                },
                TokOpt::None(text) => {
                    write!(output, "{}", text)?;
                }
            }
        }
        output.push('\n');
    }

    Ok(output)
}

pub fn format_component_applied_layers(
    applied_layers: &[(ComponentLayerId, Option<String>)],
) -> String {
    applied_layers
        .iter()
        .map(|(id, selection)| match selection {
            Some(selection) => {
                format!("{}[{}]", id.name(), selection.as_str())
            }
            None => id.name().to_string(),
        })
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn flex_table(
        term_width: u16,
        component_names: &[&str],
        ids: &[&str],
    ) -> SelfFormattingTableSpec<'static> {
        let headers = vec![
            Column::new("Component name").width_range(12, 28),
            Column::new("Agent name"),
            Column::new("Status").content_right(),
        ];
        let rows = component_names
            .iter()
            .zip(ids)
            .map(|(component, id)| {
                vec![
                    TableCell::new(component.to_string()),
                    TableCell::new(id.to_string()),
                    TableCell::new("Idle"),
                ]
            })
            .collect();
        SelfFormattingTableSpec {
            preset: TablePreset::FullCondensed,
            term_width,
            full_width: false,
            headers,
            flex: FlexColumn {
                index: 1,
                min_width: 24,
                format: &|raw, _| raw.to_string(),
            },
            rows,
        }
    }

    /// A wide terminal gives the flex column all the leftover room, but never
    /// more than its longest cell needs.
    #[test]
    fn flex_budget_fills_leftover_but_not_beyond_content() {
        let long = "ShoppingCart(\"a-fairly-long-user-identifier\", [1, 2, 3])";
        // Plenty of room: the column takes exactly what the longest id needs.
        assert_eq!(
            flex_content_width(&flex_table(
                200,
                &["comp:one", "comp:two"],
                &[long, "Counter(\"x\")"]
            )),
            Some(long.chars().count())
        );
    }

    /// A tight terminal squeezes the `Range` component column before the flex
    /// column drops below its minimum.
    #[test]
    fn flex_budget_squeezes_range_column_when_tight() {
        let ids = ["ShoppingCart(\"a-long-id-that-needs-wrapping-here\")"; 2];
        let width = flex_content_width(&flex_table(70, &["a-long-component-name-x", "b"], &ids))
            .expect("should still format");
        assert!(width >= 24, "flex dropped below its minimum: {width}");
    }

    /// Too narrow for both columns at their floor: the flex column opts out and
    /// the ids are left for the engine to wrap.
    #[test]
    fn flex_budget_gives_up_when_no_room() {
        let ids = ["ShoppingCart(\"x\")"; 2];
        assert_eq!(
            flex_content_width(&flex_table(30, &["component-name-here", "b"], &ids)),
            None
        );
    }
}
