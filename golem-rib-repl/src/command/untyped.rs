use crate::rib_context::ReplContext;
use crate::Command;

pub trait UntypedCommand {
    fn run(&self, input: &str, repl_context: &mut ReplContext);
    fn command_name(&self) -> String;
}

impl<T> UntypedCommand for T
where
    T: Command,
    T::Input: 'static,
    T::Output: 'static,
    T::InputParseError: 'static,
    T::ExecutionError: 'static,
{
    fn run(&self, input: &str, repl_context: &mut ReplContext) {
        match self.parse(input, repl_context) {
            Ok(input) => match self.execute(input, repl_context) {
                Ok(output) => self.print_output(&output, repl_context),
                Err(e) => self.print_execution_error(&e, repl_context),
            },
            Err(e) => self.print_input_parse_error(&e, repl_context),
        }
    }

    fn command_name(&self) -> String {
        T::name(self)
    }
}
