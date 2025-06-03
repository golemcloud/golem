pub use clap_parser::*;
use convert_case::{Case, Casing};
pub use registry::*;
pub use untyped::*;

mod builtin;
mod clap_parser;
mod registry;
mod untyped;

use crate::rib_context::ReplContext;

/// A Command implementation will do the following:
///  - Parse user input from a string that follows the command which is of the pattern `:command-name input` and gets a structured Input
///  - The Input will soon require a Display for documentation purpose
///  - The Input will be executed to get a structured Output or an ExecutionError
///  - Print results or errors to the user
///
///
///  To register new commands to the REPL from any client (Ex: golem-cli),
///  create a struct that implements the `Command` trait and register it using `let mut registry = CommandRegistry::default()`,
///  and `registry.register(MyCommand);`, and pass it the config when bootstrapping the REPL.
pub trait Command {
    /// The structured input type resulting from parsing the raw REPL string.
    /// If using Clap, this should be a type that derives `clap::Parser`, and the error
    /// is typically `clap::Error`.
    type Input;

    /// The output produced after successful execution of the command.
    type Output;

    /// Error type returned when parsing the user input fails.
    type InputParseError;

    /// Error type returned when command execution fails.
    type ExecutionError;

    fn name(&self) -> String {
        let full = std::any::type_name::<Self>();
        let last = full.rsplit("::").next().unwrap_or(full);
        last.to_case(Case::Kebab)
    }

    /// Parses user input into a structured `Input` type.
    ///
    /// Parse implementation can internally make use of Clap or any other parsing library
    /// If using Clap, using helpers like `parse_with_clap` is recommended to handle shell-style splitting
    /// and all you need is derived `clap:Parser` trait on the `Input` type.
    ///
    /// # Parameters
    /// - `prompt_input`: The raw string entered by the user after the command name in the REPL.
    ///   For example, if the user types `:my-command foo bar`, then `prompt_input` will be
    ///   `"foo bar"`.
    /// - `repl_context`: An immutable projection of internal ReplState.
    ///   This gives access to printer, current session of rib script etc
    //
    fn parse(
        &self,
        input: &str,
        repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError>;

    /// Executes the command with the parsed input and REPL context.
    ///
    /// # Parameters
    /// - `input`: The structured input previously returned by `parse`.
    /// - `repl_context`: An immutable projection of internal ReplState
    fn execute(
        &self,
        input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError>;

    /// Prints the output produced by the command after successful execution.
    ///
    /// # Parameters
    /// - `output`: The result returned by `execute` if it completed successfully.
    /// - `repl_context`: An immutable projection of internal ReplState
    fn print_output(&self, output: &Self::Output, repl_context: &ReplContext);

    /// Prints an error that occurred during input parsing.
    ///
    /// # Parameters
    /// - `error`: The error returned by `parse` when the user input is invalid.
    /// - `repl_context`: An immutable projection of internal ReplState
    fn print_input_parse_error(&self, error: &Self::InputParseError, repl_context: &ReplContext);

    /// Prints an error that occurred during command execution.
    ///
    /// # Parameters
    /// - `error`: The error returned by `execute` when something goes wrong during execution.
    /// - `repl_context`: An immutable projection of internal ReplState
    fn print_execution_error(&self, error: &Self::ExecutionError, repl_context: &ReplContext);
}
