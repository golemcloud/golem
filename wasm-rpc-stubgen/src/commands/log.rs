use crate::fs::{OverwriteSafeAction, OverwriteSafeActionPlan};
use crate::model::validation::ValidatedResult;
use colored::Colorize;
use std::sync::{LazyLock, RwLock};

static LOG_STATE: LazyLock<RwLock<LogState>> = LazyLock::new(RwLock::default);

struct LogState {
    indent_count: usize,
    indent_prefix: String,
}

impl LogState {
    pub fn new() -> Self {
        Self {
            indent_count: 0,
            indent_prefix: "".to_string(),
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

pub fn log_action<T: AsRef<str>>(action: &str, subject: T) {
    println!(
        "{}{} {}",
        LOG_STATE.read().unwrap().indent_prefix,
        action.green(),
        subject.as_ref()
    )
}

pub fn log_warn_action<T: AsRef<str>>(action: &str, subject: T) {
    println!(
        "{}{} {}",
        LOG_STATE.read().unwrap().indent_prefix,
        action.yellow(),
        subject.as_ref(),
    )
}

pub fn log_skipping_up_to_date<T: AsRef<str>>(subject: T) {
    log_warn_action(
        "Skipping",
        format!("{}, already up-to-date", subject.as_ref()),
    );
}

pub fn log_validated_action_result<T, F>(action: &str, result: &ValidatedResult<T>, to_log: F)
where
    F: FnOnce(&T) -> String,
{
    if let Some(value) = result.as_ok_ref() {
        log_action(action, to_log(value))
    }
}

pub fn log_action_plan(action: &OverwriteSafeAction, plan: OverwriteSafeActionPlan) {
    match plan {
        OverwriteSafeActionPlan::Create => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_action(
                    "Copying",
                    format!("{} to {}", source.display(), target.display()),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_action(
                    "Copying",
                    format!("{} to {} transformed", source.display(), target.display()),
                );
            }
            OverwriteSafeAction::WriteFile { target, .. } => {
                log_action("Creating", format!("{}", target.display()));
            }
        },
        OverwriteSafeActionPlan::Overwrite => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_warn_action(
                    "Overwriting",
                    format!("{} with {}", target.display(), source.display()),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_warn_action(
                    "Overwriting",
                    format!("{} with {} transformed", target.display(), source.display()),
                );
            }
            OverwriteSafeAction::WriteFile { content: _, target } => {
                log_warn_action("Overwriting", format!("{}", target.display()));
            }
        },
        OverwriteSafeActionPlan::SkipSameContent => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "copying {} to {}, content already up-to-date",
                        source.display(),
                        target.display(),
                    ),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "copying {} to {} transformed, content already up-to-date",
                        source.display(),
                        target.display()
                    ),
                );
            }
            OverwriteSafeAction::WriteFile { content: _, target } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "generating {}, content already up-to-date",
                        target.display()
                    ),
                );
            }
        },
    }
}
