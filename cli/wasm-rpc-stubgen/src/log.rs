use crate::fs::{OverwriteSafeAction, OverwriteSafeActionPlan, PathExtra};
use colored::{ColoredString, Colorize};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

static LOG_STATE: LazyLock<RwLock<LogState>> = LazyLock::new(RwLock::default);

#[derive(Debug, Clone, Copy)]
pub enum Output {
    Stdout,
    Stderr,
    None,
}

struct LogState {
    indent_count: usize,
    indent_prefix: String,
    output: Output,
}

impl LogState {
    pub fn new() -> Self {
        Self {
            indent_count: 0,
            indent_prefix: "".to_string(),
            output: Output::Stdout,
        }
    }

    pub fn inc_indent(&mut self) {
        self.indent_count += 1;
        self.regen_indent_prefix()
    }

    pub fn dec_indent(&mut self) {
        self.indent_count -= 1;
        self.regen_indent_prefix()
    }

    fn regen_indent_prefix(&mut self) {
        self.indent_prefix = "  ".repeat(self.indent_count);
    }

    fn set_output(&mut self, output: Output) {
        self.output = output;
    }
}

impl Default for LogState {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LogIndent;

impl LogIndent {
    pub fn new() -> Self {
        LOG_STATE.write().unwrap().inc_indent();
        Self {}
    }
}

impl Default for LogIndent {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LogIndent {
    fn drop(&mut self) {
        LOG_STATE.write().unwrap().dec_indent();
    }
}

pub struct LogOutput {
    prev_output: Output,
}

impl LogOutput {
    pub fn new(output: Output) -> Self {
        let prev_output = LOG_STATE.read().unwrap().output;
        LOG_STATE.write().unwrap().set_output(output);
        Self { prev_output }
    }
}

impl Drop for LogOutput {
    fn drop(&mut self) {
        LOG_STATE.write().unwrap().set_output(self.prev_output);
    }
}

pub fn set_log_output(output: Output) {
    LOG_STATE.write().unwrap().set_output(output);
}

pub fn log_action<T: AsRef<str>>(action: &str, subject: T) {
    let state = LOG_STATE.read().unwrap();
    let message = format!(
        "{}{} {}",
        state.indent_prefix,
        action.log_color_action(),
        subject.as_ref()
    );

    match state.output {
        Output::Stdout => {
            println!("{}", message);
        }
        Output::Stderr => {
            eprintln!("{}", message);
        }
        Output::None => {
            // NOP
        }
    }
}

pub fn log_warn_action<T: AsRef<str>>(action: &str, subject: T) {
    let state = LOG_STATE.read().unwrap();
    let message = format!(
        "{}{} {}",
        LOG_STATE.read().unwrap().indent_prefix,
        action.log_color_warn(),
        subject.as_ref(),
    );

    match state.output {
        Output::Stdout => {
            println!("{}", message)
        }
        Output::Stderr => {
            eprintln!("{}", message)
        }
        Output::None => {}
    }
}

pub fn log_skipping_up_to_date<T: AsRef<str>>(subject: T) {
    log_warn_action(
        "Skipping",
        format!(
            "{}, {}",
            subject.as_ref(),
            "UP-TO-DATE".log_color_ok_highlight()
        ),
    );
}

pub fn log_action_plan(action: &OverwriteSafeAction, plan: OverwriteSafeActionPlan) {
    match plan {
        OverwriteSafeActionPlan::Create => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_action(
                    "Copying",
                    format!(
                        "{} to {}",
                        source.log_color_highlight(),
                        target.log_color_highlight()
                    ),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_action(
                    "Copying",
                    format!(
                        "{} to {} transformed",
                        source.log_color_highlight(),
                        target.log_color_highlight()
                    ),
                );
            }
            OverwriteSafeAction::WriteFile { target, .. } => {
                log_action("Creating", format!("{}", target.log_color_highlight()));
            }
        },
        OverwriteSafeActionPlan::Overwrite => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_warn_action(
                    "Overwriting",
                    format!(
                        "{} with {}",
                        target.log_color_highlight(),
                        source.log_color_highlight()
                    ),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_warn_action(
                    "Overwriting",
                    format!(
                        "{} with {} transformed",
                        target.log_color_highlight(),
                        source.log_color_highlight()
                    ),
                );
            }
            OverwriteSafeAction::WriteFile { content: _, target } => {
                log_warn_action("Overwriting", format!("{}", target.log_color_highlight()));
            }
        },
        OverwriteSafeActionPlan::SkipSameContent => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "copying {} to {}, content already up-to-date",
                        source.log_color_highlight(),
                        target.log_color_highlight(),
                    ),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "copying {} to {} transformed, content already up-to-date",
                        source.log_color_highlight(),
                        target.log_color_highlight()
                    ),
                );
            }
            OverwriteSafeAction::WriteFile { content: _, target } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "generating {}, content already up-to-date",
                        target.log_color_highlight()
                    ),
                );
            }
        },
    }
}

pub trait LogColorize {
    fn as_str(&self) -> impl Colorize;

    fn log_color_action(&self) -> ColoredString {
        self.as_str().green()
    }

    fn log_color_warn(&self) -> ColoredString {
        self.as_str().yellow()
    }

    fn log_color_error(&self) -> ColoredString {
        self.as_str().red()
    }

    fn log_color_highlight(&self) -> ColoredString {
        self.as_str().bold()
    }

    fn log_color_help_group(&self) -> ColoredString {
        self.as_str().bold().underline()
    }

    fn log_color_error_highlight(&self) -> ColoredString {
        self.as_str().bold().red().underline()
    }

    fn log_color_ok_highlight(&self) -> ColoredString {
        self.as_str().bold().green()
    }
}

impl LogColorize for &str {
    fn as_str(&self) -> impl Colorize {
        *self
    }
}

impl LogColorize for String {
    fn as_str(&self) -> impl Colorize {
        self.as_str()
    }
}

impl LogColorize for &Path {
    fn as_str(&self) -> impl Colorize {
        ColoredString::from(self.display().to_string())
    }
}

impl LogColorize for PathBuf {
    fn as_str(&self) -> impl Colorize {
        ColoredString::from(self.display().to_string())
    }
}

impl<P: AsRef<Path>> LogColorize for PathExtra<P> {
    fn as_str(&self) -> impl Colorize {
        ColoredString::from(self.display().to_string())
    }
}
