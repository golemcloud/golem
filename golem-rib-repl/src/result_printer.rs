use crate::rib_repl::ReplBootstrapError;
use colored::Colorize;
use rib::{RibError, RibResult};

pub trait ReplPrinter {
    fn print_rib_result(&self, result: &RibResult);
    fn print_compilation_error(&self, error: &RibError);
    fn print_bootstrap_error(&self, error: &ReplBootstrapError);
    fn print_runtime_error(&self, error: &str);
}

pub struct DefaultResultPrinter;

impl ReplPrinter for DefaultResultPrinter {
    fn print_rib_result(&self, result: &RibResult) {
        println!("{}", result.to_string().green());
    }

    fn print_compilation_error(&self, error: &RibError) {
        match error {
            RibError::InternalError(msg) => {
                println!("{} {}", "[internal rib error]".red(), msg.red());
            }
            RibError::RibCompilationError(compilation_error) => {
                let cause = &compilation_error.cause;
                let position = compilation_error.expr.source_span().start_column();

                println!("{}", "[compilation error]".red());
                println!("{} {}", "position:".yellow(), position.to_string().white());
                println!("{} {}", "cause:".yellow(), cause.white());

                if !compilation_error.additional_error_details.is_empty() {
                    for detail in &compilation_error.additional_error_details {
                        println!("{}", detail.white());
                    }
                }

                if !compilation_error.help_messages.is_empty() {
                    for message in &compilation_error.help_messages {
                        println!("{} {}", "help:".blue(), message.white());
                    }
                }
            }
            RibError::InvalidRibScript(script) => {
                println!("{} {}", "[invalid script]".red(), script.white());
            }
        }
    }

    fn print_bootstrap_error(&self, error: &str) {
        println!("{} {}", "warn: bootstrap error".yellow(), error.white());
    }

    fn print_runtime_error(&self, error: &str) {
        println!("{} {}", "[runtime error]".red(), error.white());
    }
}
