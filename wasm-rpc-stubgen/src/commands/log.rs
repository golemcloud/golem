use crate::model::validation::ValidatedResult;
use colored::Colorize;

pub fn log_action<T: AsRef<str>>(action: &str, subject: T) {
    println!("{} {}", action.green(), subject.as_ref())
}

pub fn log_warn_action<T: AsRef<str>>(action: &str, subject: T) {
    println!("{} {}", action.yellow(), subject.as_ref())
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
