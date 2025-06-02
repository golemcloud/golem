use std::fmt::format;
use clap::{Error, Parser};
use clap::error::ErrorKind;

/// Parses a shell-style argument string into a [`clap::Parser`] type.
///
/// Designed for REPL commands where the command name (e.g., `:type-info`) is already stripped.
/// The input should contain only the arguments (e.g., `"foo bar"`).
///
/// # Errors
/// Returns a [`clap::Error`] if argument splitting or parsing fails
pub fn parse_with_clap<T: Parser>(command_name: &str, input: &str) -> Result<T, Error> {
    let args =
        shell_words::split(input).map_err(|e|
            Error::raw(ErrorKind::InvalidValue, format!("Failed to parse input to command: {}", e))
        )?;

    let command = format!(":{}", command_name);

    T::try_parse_from(std::iter::once(command).chain(args))
}