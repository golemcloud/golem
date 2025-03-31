use colored::Colorize;
use rib::{RibError, RibResult};

pub trait ResultPrinter {
    fn print_rib_result(&self, result: &RibResult);
    fn print_compilation_error(&self, error: &RibError);
    fn print_bootstrap_error(&self, error: &str);
    fn print_runtime_error(&self, error: &str);
}

pub struct DefaultResultPrinter;

impl ResultPrinter for DefaultResultPrinter {
    fn print_rib_result(&self, result: &RibResult) {
        println!("{}", result.to_string().green())
    }

    fn print_compilation_error(&self, error: &RibError) {
       match error {
           RibError::InternalError(error) => {
               print!("{}: ", "[internal rib error]".red().bold());
               println!("{}", error.red())
           }
           RibError::RibCompilationError(compilation_error) => {
               let cause = &compilation_error.cause;
               let position =  compilation_error.expr.source_span().start_column();

               println!("{}", "[compile error]".red().bold());
               println!("{}", format!("{}, {}", "pos:".yellow(), position.to_string().white()));
               println!("{}", format!("{}: {}", "cause:".yellow(), cause.white()));


               if !compilation_error.additional_error_details.is_empty() {
                   for message in &compilation_error.additional_error_details {
                       println!("{}", message.white().bold())
                   }
               }

               if !compilation_error.help_messages.is_empty() {
                   for message in &compilation_error.help_messages {
                      let str = format!("{}: {}", "help:".yellow(), message.white().bold());
                       println!("{}", str)
                   }
               }
           }
           RibError::InvalidRibScript(string) => {
                println!("{}: {}", "invalid rib script".red().bold(), string.white())
           }
       }
    }

    fn print_bootstrap_error(&self, error: &str) {
        println!("{}", format!("[repl bootstrap error]: {}", error).red())
    }

    fn print_runtime_error(&self, error: &str) {
        println!("[error: !!]: {}", error.red())
    }
}
