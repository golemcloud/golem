use crate::{parse_with_clap, Command, FunctionSignaturePrintConfig, ReplContext};
use clap::{CommandFactory, Parser};
use crossterm::cursor::MoveTo;
use crossterm::{
    execute,
    terminal::{Clear as TermClear, ClearType},
};
use golem_wasm_ast::analysis::AnalysedType;
use rib::{CompilerOutput, ComponentDependencies, Expr, RibCompilationError};
use std::io::stdout;

#[derive(Parser, Debug)]
#[command(about = "Display type of a rib expression")]
pub struct TypeInfoInput {
    /// Rib expression.
    /// Multiline rib expressions are not supported.
    #[arg(required = true)]
    pub expr: Vec<String>,
}

impl TypeInfoInput {
    pub fn as_text(&self) -> String {
        self.expr.join(" ")
    }
}

pub struct TypeInfo;

impl Command for TypeInfo {
    type Input = TypeInfoInput;
    type Output = AnalysedType;
    type InputParseError = clap::Error;
    type ExecutionError = RibCompilationError;

    fn parse(
        &self,
        input: &str,
        _repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        let parse_result = parse_with_clap::<TypeInfoInput>(self.name().as_str(), input)?;

        Ok(parse_result)
    }

    fn execute(
        &self,
        input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        let existing_raw_script = repl_context.get_new_rib_script(input.as_text().as_str());

        let expr = Expr::from_text(&existing_raw_script.as_text())
            .map_err(|e| RibCompilationError::InvalidSyntax(e.to_string()))?;

        let compiler_output: CompilerOutput = repl_context.get_rib_compiler().compile(expr)?;

        let result = compiler_output
            .rib_output_type_info
            .ok_or(RibCompilationError::RibStaticAnalysisError(
                "Rib output type info is not available".to_string(),
            ))
            .map(|info| info.analysed_type)?;

        Ok(result)
    }

    fn print_output(&self, output: Self::Output, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_wasm_value_type(&output);
    }

    fn print_input_parse_error(&self, error: Self::InputParseError, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_clap_parse_error(&error);
    }

    fn print_execution_error(&self, error: Self::ExecutionError, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_rib_compilation_error(&error);
    }
}

pub struct Clear;

impl Command for Clear {
    type Input = ();
    type Output = ();
    type InputParseError = ();
    type ExecutionError = ();

    fn parse(
        &self,
        _input: &str,
        _repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        Ok(())
    }

    fn execute<'a>(
        &self,
        _input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        repl_context.clear_state();
        repl_context.clear_history();
        execute!(stdout(), TermClear(ClearType::All), MoveTo(0, 0)).unwrap();
        Ok(())
    }

    fn print_output(&self, _output: Self::Output, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_custom_message("Rib REPL has been cleared");
    }

    fn print_input_parse_error(&self, _error: Self::InputParseError, _repl_context: &ReplContext) {}

    fn print_execution_error(&self, _error: Self::ExecutionError, _repl_context: &ReplContext) {}
}

#[derive(Parser, Debug)]
#[command(about = "Configuration to customise printing exports")]
pub struct ExportPrintConfig {
    /// Allows you to skip printing function argument types in the exports.
    #[arg(required = false, long, default_value_t = false)]
    pub skip_function_args: bool,

    /// Allows you to skip printing function return types in the exports.
    #[arg(required = false, long, default_value_t = false)]
    pub skip_function_return_type: bool,
}

pub struct ExportOutput {
    pub component_dependencies: ComponentDependencies,
    pub printer_config: ExportPrintConfig,
}

pub struct Exports;

impl Command for Exports {
    type Input = ExportPrintConfig;
    type Output = ExportOutput;
    type InputParseError = clap::Error;
    type ExecutionError = RibCompilationError;

    fn command_argument_names() -> Vec<String> {
        let command = ExportPrintConfig::command();

        command
            .get_arguments()
            .map(|arg| arg.get_id().to_string())
            .collect()
    }

    fn parse(
        &self,
        input: &str,
        _repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        let parse_result = parse_with_clap::<ExportPrintConfig>(self.name().as_str(), input)?;

        Ok(parse_result)
    }

    fn execute(
        &self,
        input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        let dependencies = repl_context.get_rib_compiler().get_component_dependencies();

        Ok(ExportOutput {
            component_dependencies: dependencies,
            printer_config: input,
        })
    }

    fn print_output(&self, output: Self::Output, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();
        printer.print_components_and_exports(
            &output.component_dependencies,
            &FunctionSignaturePrintConfig {
                print_args: !output.printer_config.skip_function_args,
                print_return_type: !output.printer_config.skip_function_return_type,
            },
        );
    }

    fn print_input_parse_error(&self, _error: Self::InputParseError, _repl_context: &ReplContext) {}

    fn print_execution_error(&self, error: Self::ExecutionError, repl_context: &ReplContext) {
        repl_context
            .get_printer()
            .print_rib_compilation_error(&error)
    }
}

pub struct ExportsConcise;

impl Command for ExportsConcise {
    type Input = ();
    type Output = ComponentDependencies;
    type InputParseError = ();
    type ExecutionError = RibCompilationError;

    fn parse(
        &self,
        _input: &str,
        _repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        Ok(())
    }

    fn execute(
        &self,
        _input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        let dependencies = repl_context.get_rib_compiler().get_component_dependencies();
        Ok(dependencies)
    }

    fn print_output(&self, output: Self::Output, repl_context: &ReplContext) {
        let printer = repl_context.get_printer();

        printer.print_components_and_exports(
            &output,
            &FunctionSignaturePrintConfig {
                print_args: false,
                print_return_type: false,
            },
        );
    }

    fn print_input_parse_error(&self, _error: Self::InputParseError, _repl_context: &ReplContext) {}

    fn print_execution_error(&self, error: Self::ExecutionError, repl_context: &ReplContext) {
        repl_context
            .get_printer()
            .print_rib_compilation_error(&error)
    }
}
