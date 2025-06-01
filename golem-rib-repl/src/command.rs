use crate::rib_context::ReplContext;

/// A trait representing a REPL command that can:
/// - Parse user input from a string
/// - Execute logic based on that input
/// - Print results or errors to the user
///
/// Commands are invoked via a `:command-name` in a REPL, and everything following the
/// command name is passed as raw input to `parse`.
pub trait Command {
    /// The structured input type resulting from parsing the raw REPL string.
    type Input;

    /// The output produced after successful execution of the command.
    type Output;

    /// Error type returned when parsing the user input fails.
    type InputParseError;

    /// Error type returned when command execution fails.
    type ExecutionError;

    /// Parses user input into a structured `Input` type.
    ///
    /// # Parameters
    /// - `prompt_input`: The raw string entered by the user after the command name in the REPL.
    ///   For example, if the user types `:my-command foo bar`, then `prompt_input` will be
    ///   `"foo bar"`.
    /// - `repl_context`: Shared context that may include state, configuration, or environment
    ///   data needed during parsing.
    ///
    /// # Returns
    /// - `Ok(Self::Input)` if parsing is successful.
    /// - `Err(Self::InputParseError)` if the input is malformed or invalid.
    fn parse(
        &self,
        prompt_input: &str,
        repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError>;

    /// Executes the command with the parsed input and REPL context.
    ///
    /// # Parameters
    /// - `input`: The structured input previously returned by `parse`.
    /// - `repl_context`: The REPL context, providing necessary shared state for execution.
    ///
    /// # Returns
    /// - `Ok(Self::Output)` if execution is successful.
    /// - `Err(Self::ExecutionError)` if execution fails.
    fn execute(
        &self,
        input: Self::Input,
        repl_context: &ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError>;

    /// Prints the output produced by the command after successful execution.
    ///
    /// # Parameters
    /// - `output`: The result returned by `execute` if it completed successfully.
    /// - `repl_context`: The REPL context, providing necessary shared state for execution.
    fn print_output(&self, output: &Self::Output, repl_context: &ReplContext, );

    /// Prints an error that occurred during input parsing.
    ///
    /// # Parameters
    /// - `error`: The error returned by `parse` when the user input is invalid.
    /// - `repl_context`: The REPL context, providing necessary shared state for execution.
    fn print_input_parse_error(&self,  error: &Self::InputParseError, repl_context: &ReplContext);

    /// Prints an error that occurred during command execution.
    ///
    /// # Parameters
    /// - `error`: The error returned by `execute` when something goes wrong during execution.
    /// - `repl_context`: The REPL context, providing necessary shared state for execution.
    fn print_execution_error(&self,  error: &Self::ExecutionError, repl_context: &ReplContext, );
}
