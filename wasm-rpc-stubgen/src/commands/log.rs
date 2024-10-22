use crate::fs::{OverwriteSafeAction, OverwriteSafeActionPlan};
use crate::model::validation::ValidatedResult;
use colored::Colorize;

pub fn log_action<T: AsRef<str>>(action: &str, subject: T) {
    println!("{} {}", action.green(), subject.as_ref())
}

pub fn log_warn_action<T: AsRef<str>>(action: &str, subject: T) {
    println!("{} {}", action.yellow(), subject.as_ref())
}

pub fn log_skipping_up_to_date<T: AsRef<str>>(subject: T) {
    log_warn_action(
        "Skipping",
        format!("{}, already up-to-date", subject.as_ref()),
    );
}

pub fn log_validated_action_result<T, F>(action: &str, result: &ValidatedResult<T>, to_log: F)
where
    F: Fn(&T) -> String,
{
    result
        .as_ok_ref()
        .iter()
        .for_each(|value| log_action(action, to_log(value)));
}

pub fn log_action_plan(action: &OverwriteSafeAction, plan: OverwriteSafeActionPlan) {
    match plan {
        OverwriteSafeActionPlan::Create => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_action(
                    "Copying",
                    format!(
                        "{} to {}",
                        source.to_string_lossy(),
                        target.to_string_lossy()
                    ),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_action(
                    "Copying",
                    format!(
                        "{} to {} transformed",
                        source.to_string_lossy(),
                        target.to_string_lossy()
                    ),
                );
            }
            OverwriteSafeAction::WriteFile { target, .. } => {
                log_action("Creating", format!("{}", target.to_string_lossy()));
            }
        },
        OverwriteSafeActionPlan::Overwrite => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_warn_action(
                    "Overwriting",
                    format!(
                        "{} with {}",
                        target.to_string_lossy(),
                        source.to_string_lossy()
                    ),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_warn_action(
                    "Overwriting",
                    format!(
                        "{} with {} transformed",
                        target.to_string_lossy(),
                        source.to_string_lossy()
                    ),
                );
            }
            OverwriteSafeAction::WriteFile { content: _, target } => {
                log_warn_action("Overwriting", format!("{}", target.to_string_lossy()));
            }
        },
        OverwriteSafeActionPlan::SkipSameContent => match action {
            OverwriteSafeAction::CopyFile { source, target } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "copying {} to {}, content already up-to-date",
                        source.to_string_lossy(),
                        target.to_string_lossy(),
                    ),
                );
            }
            OverwriteSafeAction::CopyFileTransformed { source, target, .. } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "copying {} to {} transformed, content already up-to-date",
                        source.to_string_lossy(),
                        target.to_string_lossy()
                    ),
                );
            }
            OverwriteSafeAction::WriteFile { content: _, target } => {
                log_warn_action(
                    "Skipping",
                    format!(
                        "generating {}, content already up-to-date",
                        target.to_string_lossy()
                    ),
                );
            }
        },
    }
}
