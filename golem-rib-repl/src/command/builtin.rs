use crate::{Command, ReplContext};
use golem_wasm_ast::analysis::AnalysedType;
use rib::{CompilerOutput, Expr, RibCompilationError};

#[derive(Clone)]
pub struct TypeInfo;

impl Command for TypeInfo {
    type Input = CompilerOutput;
    type Output = AnalysedType;
    type InputParseError = RibCompilationError;
    type ExecutionError = RibCompilationError;

    fn parse(
        &self,
        prompt_input: &str,
        repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        let mut existing_raw_script = repl_context.get_rib_script().clone();
        existing_raw_script.push(prompt_input);

        let expr = Expr::from_text(&existing_raw_script.as_text())
            .map_err(|e| RibCompilationError::InvalidSyntax(e.to_string()))?;

        let compiler_output: CompilerOutput = repl_context.get_rib_compiler().compile(expr)?;

        Ok(compiler_output)
    }

    fn execute(
        &self,
        input: Self::Input,
        _repl_context: &ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        let result = input
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
