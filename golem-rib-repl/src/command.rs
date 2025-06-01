use std::collections::HashMap;
use golem_wasm_ast::analysis::AnalysedType;
use rib::{CompilerOutput, Expr, RibCompilationError, RibCompiler};
use crate::{ReplPrinter};
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

pub struct TypeInfo;

impl Command for TypeInfo {
    type Input = CompilerOutput;
    type Output = AnalysedType;
    type InputParseError = RibCompilationError;
    type ExecutionError = RibCompilationError;

    fn parse(&self, prompt_input: &str, repl_context: &ReplContext) -> Result<Self::Input, Self::InputParseError> {
        let mut existing_raw_script = repl_context.get_rib_script().clone();
        existing_raw_script.push(prompt_input);

        let expr = Expr::from_text(&existing_raw_script.as_text())
            .map_err(|e| RibCompilationError::InvalidSyntax(e.to_string()))?;

        let compiler_output: CompilerOutput =
            repl_context.get_compiler().compile(expr)?;

        Ok(compiler_output)
    }

    fn execute(&self, input: Self::Input, _repl_context: &ReplContext) -> Result<Self::Output, Self::ExecutionError> {
       let result =
           input.rib_output_type_info.ok_or(RibCompilationError::RibStaticAnalysisError(
               "Rib output type info is not available".to_string(),
           )).map(|info| info.analysed_type)?;

        Ok(result)
    }

    fn print_output(&self, output: &Self::Output, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_wasm_value_type(output);
    }

    fn print_input_parse_error(&self, error: &Self::InputParseError, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_rib_compilation_error(error);
    }

    fn print_execution_error(&self, error: &Self::ExecutionError, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_rib_compilation_error(error);
    }
}


pub struct CommandRegistry {
    commands: HashMap<String, Box<dyn ErasedCommand>>,
}

impl CommandRegistry {
    pub fn register<T>(&mut self, command: T)
    where T: ErasedCommand + 'static,
    {
        let name = command.name().to_string();
        self.commands.insert(name, Box::new(command));
    }
}

trait ErasedCommand {
    fn run(&self, prompt_input: &str, repl_context: &ReplContext);
    fn name(&self) -> &str;
}

impl<T> ErasedCommand for T
where
    T: Command,
    T::Input: 'static,
    T::Output: 'static,
    T::InputParseError: 'static,
    T::ExecutionError: 'static,
{
    fn run(&self, prompt_input: &str, repl_context: &ReplContext) {
        match self.parse(prompt_input, repl_context) {
            Ok(input) => match self.execute(input, repl_context) {
                Ok(output) => self.print_output(&output, repl_context),
                Err(e) => self.print_execution_error(&e, repl_context),
            },
            Err(e) => self.print_input_parse_error(&e, repl_context),
        }
    }

    fn name(&self) -> &str {
        std::any::type_name::<T>()
    }
}