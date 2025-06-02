use crate::{Command, ReplContext};
use golem_wasm_ast::analysis::AnalysedType;
use rib::{CompilerOutput, Expr, RibCompilationError};
use crossterm::{execute, terminal::{ClearType, Clear as TermClear}};
use std::io::{stdout};
use crossterm::cursor::MoveTo;

#[derive(Clone)]
pub struct TypeInfo;

impl Command for TypeInfo {
    type Input = Expr;
    type Output = AnalysedType;
    type InputParseError = RibCompilationError;
    type ExecutionError = RibCompilationError;

    fn parse(
        &self,
        prompt_input: &str,
        repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {

        if prompt_input.is_empty() {
            return Err(RibCompilationError::InvalidSyntax(
                "Input cannot be empty".to_string(),
            ));
        }
        let  existing_raw_script =
            repl_context.get_new_rib_script(prompt_input);

        let expr = Expr::from_text(&existing_raw_script.as_text())
            .map_err(|e| RibCompilationError::InvalidSyntax(e.to_string()))?;

        Ok(expr)
    }

    fn execute(
        &self,
        input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        let compiler_output: CompilerOutput =
            repl_context.get_rib_compiler().compile(input)?;

        let result = compiler_output
            .rib_output_type_info
            .ok_or(RibCompilationError::RibStaticAnalysisError(
                "Rib output type info is not available".to_string(),
            ))
            .map(|info| info.analysed_type)?;

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


#[derive(Clone)]
pub struct Clear;

impl Command for Clear {
    type Input = ();
    type Output = ();
    type InputParseError = ();
    type ExecutionError = ();

    fn parse(
        &self,
        _prompt_input: &str,
        _repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        Ok(())
    }

    fn execute(
        &self,
        _input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        repl_context.clear();
        execute!(stdout(), TermClear(ClearType::All), MoveTo(0, 0)).unwrap();
        Ok(())
    }

    fn print_output(&self, _output: &Self::Output, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_custom_message("Rib REPL has been cleared");
    }

    fn print_input_parse_error(&self, _error: &Self::InputParseError, _repl_context: &ReplContext) {}

    fn print_execution_error(&self, _error: &Self::ExecutionError, _repl_context: &ReplContext) {}
}

#[derive(Clone)]
pub struct ClearHistory;

impl Command for ClearHistory {
    type Input = ();
    type Output = ();
    type InputParseError = ();
    type ExecutionError = ();

    fn parse(
        &self,
        _prompt_input: &str,
        _repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        Ok(())
    }

    fn execute(
        &self,
        _input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        repl_context.clear_history();
        Ok(())
    }

    fn print_output(&self, _output: &Self::Output, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_custom_message("Rib REPL history has been cleared. To clear complete REPL state, use clear command");
    }

    fn print_input_parse_error(&self, _error: &Self::InputParseError, _repl_context: &ReplContext) {}

    fn print_execution_error(&self, _error: &Self::ExecutionError, _repl_context: &ReplContext) {}
}
