// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::rib_repl::ReplBootstrapError;
use colored::Colorize;
use rib::{RibCompilationError, RibResult, RibRuntimeError};

pub trait ReplPrinter {
    fn print_rib_result(&self, result: &RibResult);
    fn print_rib_compilation_error(&self, error: &RibCompilationError);
    fn print_bootstrap_error(&self, error: &ReplBootstrapError);
    fn print_rib_runtime_error(&self, error: &RibRuntimeError);
}

#[derive(Clone)]
pub struct DefaultReplResultPrinter;

impl ReplPrinter for DefaultReplResultPrinter {
    fn print_rib_result(&self, result: &RibResult) {
        println!("{}", result.to_string().green());
    }

    fn print_rib_compilation_error(&self, error: &RibCompilationError) {
        match error {
            RibCompilationError::RibStaticAnalysisError(msg) => {
                println!("{} {}", "[rib static analysis error]".red(), msg.red());
            }

            RibCompilationError::UnsupportedGlobalInput {
                invalid_global_inputs: found,
                valid_global_inputs: expected,
            } => {
                println!(
                    "{} {} {}",
                    "[unsupported input]".red(),
                    "found:".yellow(),
                    found.join(", ").white()
                );
                println!(
                    "{} {} {}",
                    "[supported inputs]".green(),
                    "expected:".yellow(),
                    expected.join(", ").white()
                );
            }
            RibCompilationError::RibTypeError(compilation_error) => {
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
            RibCompilationError::InvalidSyntax(script) => {
                println!("{} {}", "[invalid script]".red(), script.white());
            }
            RibCompilationError::ByteCodeGenerationFail(error) => {
                println!(
                    "{} {}",
                    "[internal bytecode generation error]".red(),
                    error.to_string().red()
                );
            }
        }
    }

    fn print_bootstrap_error(&self, error: &ReplBootstrapError) {
        match error {
            ReplBootstrapError::ReplHistoryFileError(msg) => {
                println!("{} {}", "[warn]".yellow(), msg);
            }
            ReplBootstrapError::ComponentLoadError(msg) => {
                println!("{} {}", "[error]".red(), msg);
            }
            ReplBootstrapError::MultipleComponentsFound(msg) => {
                println!("{} {}", "[error]".red(), msg);
                println!(
                    "{}",
                    "specify the component name when bootstrapping repl".yellow()
                );
            }
            ReplBootstrapError::NoComponentsFound => {
                println!(
                    "{} no components found in the repl context",
                    "[warn]".yellow()
                );
            }
        }
    }

    fn print_rib_runtime_error(&self, error: &RibRuntimeError) {
        println!("{} {}", "[runtime error]".red(), error.to_string().white());
    }
}
