use std::collections::HashMap;
use std::sync::Arc;
use golem_wasm_ast::analysis::AnalysedType;
use rib::{CompilerOutput, Expr, RibCompilationError};
use crate::{Command};
use crate::rib_context::ReplContext;

#[derive(Default)]
pub struct CommandRegistry {
    commands: HashMap<String, Arc<dyn UntypedCommand>>,
}

impl CommandRegistry {
    pub fn built_in() -> Self {
        let mut registry = Self::default();
        registry.register(TypeInfo);
        registry
    }
    pub fn register<T>(&mut self, command: T)
    where T: UntypedCommand + 'static,
    {
        let name = command.name().to_string();
        self.commands.insert(name, Arc::new(command));
    }

    pub fn get_command(&self, name: &str) -> Option<Arc<dyn UntypedCommand>> {
        let result = self.commands.get(name);
        match result {
            Some(command) => Some(command.clone()),
            None => None,
        }
    }
}

pub trait UntypedCommand {
    fn run(&self, prompt_input: &str, repl_context: &ReplContext);
    fn name(&self) -> &str;
}

impl<T: Clone> UntypedCommand for T
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

#[derive(Clone)]
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