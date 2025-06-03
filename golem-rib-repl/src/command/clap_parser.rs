use clap::error::ErrorKind;
use clap::{Error, Parser};

/// Helper for implementing `parse` function in `golem_rib_repl::Command` trait to obtain `Input`,
/// given `Input` has an instance of `clap::Parser`.
///
/// If there are explicit command name annotations for `Input, they will be discarded.
/// The command name is or should be consistent with `name()` function of the `Command` trait.
///
/// # Parameters
/// - `command_name`: The name of the command (without the `:` prefix), such as `"type-info"`.
///   This is mostly used for help messages. The command-name is automatically available when implementing
///   golem_rib_repl::Command` and can be passed through directly.
///
/// - `input`: The space-separated argument string for the command, such as `"foo bar"`.
///   Example: For `:type-info foo`, the `input` is `foo` and command name is `type-info`.
///   Typically parse_with_clap is called when implementing Command, and the `input` is already available
///   at that point and can be passed through directly.
///
pub fn parse_with_clap<Input: Parser>(command_name: &str, input: &str) -> Result<Input, Error> {
    let args = shell_words::split(input).map_err(|e| {
        Error::raw(
            ErrorKind::InvalidValue,
            format!("Failed to parse input to command: {}", e),
        )
    })?;

    let command = format!(":{}", command_name);

    Input::try_parse_from(std::iter::once(command).chain(args))
}
