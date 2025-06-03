// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::ReplBootstrapError;
use colored::Colorize;
use golem_wasm_ast::analysis::AnalysedType;
use rib::*;
use std::collections::BTreeMap;

pub trait ReplPrinter {
    fn print_bootstrap_error(&self, error: &ReplBootstrapError) {
        print_bootstrap_error(error);
    }

    fn print_clap_parse_error(&self, error: &clap::Error) {
        println!("{}", error.to_string().red());
    }

    fn print_custom_error(&self, error: &str) {
        println!("{} {}", "[message]".red(), error.red());
    }

    fn print_custom_message(&self, message: &str) {
        println!("{} {}", "[message]".yellow(), message.cyan());
    }

    fn print_exports(&self, exports: &FunctionDictionary) {
        print_function_dictionary(exports)
    }

    fn print_rib_compilation_error(&self, error: &RibCompilationError) {
        print_rib_compilation_error(error);
    }

    fn print_rib_result(&self, result: &RibResult) {
        println!("{}", result.to_string().yellow());
    }

    fn print_rib_runtime_error(&self, error: &RibRuntimeError) {
        println!("{} {}", "[runtime error]".red(), error.to_string().white());
    }

    fn print_wasm_value_type(&self, analysed_type: &AnalysedType) {
        println!(
            "{}",
            wasm_wave::wasm::DisplayType(analysed_type)
                .to_string()
                .yellow()
        );
    }
}

#[derive(Clone)]
pub struct DefaultReplResultPrinter;

impl ReplPrinter for DefaultReplResultPrinter {}

pub fn print_function_dictionary(dict: &FunctionDictionary) {
    let mut output = String::new();

    // Group entries by package and interface
    let mut hierarchy: BTreeMap<
        Option<PackageName>,
        BTreeMap<Option<InterfaceName>, HierarchyNode>,
    > = BTreeMap::new();

    for (name, ftype) in &dict.name_and_types {
        match name {
            FunctionName::Function(func) => {
                let node = hierarchy
                    .entry(func.package_name.clone())
                    .or_default()
                    .entry(func.interface_name.clone())
                    .or_default();
                node.functions.push((func.function_name.clone(), ftype));
            }

            FunctionName::ResourceConstructor(ctor) => {
                let node = hierarchy
                    .entry(ctor.package_name.clone())
                    .or_default()
                    .entry(ctor.interface_name.clone())
                    .or_default();
                node.resources
                    .entry(ctor.resource_name.clone())
                    .or_default()
                    .constructor = Some(ftype);
            }

            FunctionName::ResourceMethod(method) => {
                let node = hierarchy
                    .entry(method.package_name.clone())
                    .or_default()
                    .entry(method.interface_name.clone())
                    .or_default();
                node.resources
                    .entry(method.resource_name.clone())
                    .or_default()
                    .methods
                    .push((method.method_name.clone(), ftype));
            }
        }
    }

    for (pkg, interfaces) in hierarchy {
        match pkg {
            Some(pkg) => {
                output.push_str(&format!(
                    "{} {}\n",
                    "📦 Package:".bold().bright_yellow(),
                    format!("{}::{}", pkg.namespace, pkg.package_name)
                        .bold()
                        .truecolor(180, 180, 180)
                ));
            }
            None => {
                output.push_str(&format!("{}\n", "📦 Global Scope".bold().bright_yellow()));
            }
        }

        for (iface, node) in interfaces {
            if let Some(iface) = iface {
                output.push_str(&format!(
                    "  {} {}\n",
                    "📄 Interface:".bold().bright_cyan(),
                    iface.name.bold().truecolor(180, 180, 180)
                ));
            }

            if !node.functions.is_empty() {
                output.push_str(&format!("    {}\n", "🔧 Functions:".bold().bright_green(),));
            }

            for (fname, ftype) in &node.functions {
                output.push_str(&format!("         {}\n", fname.bright_magenta()));
                output.push_str(&format!(
                    "           ↳ {}: {}\n",
                    "Args".blue(),
                    format_type_list(&ftype.parameter_types).truecolor(180, 180, 180)
                ));
                output.push_str(&format!(
                    "           ↳ {}: {}\n",
                    "Returns".blue(),
                    format_type_list(&ftype.return_type).truecolor(180, 180, 180)
                ));
            }

            for (res_name, res) in &node.resources {
                output.push_str(&format!(
                    "    {} {}\n",
                    "🧩️ Resource:".bold().bright_yellow(),
                    res_name.truecolor(180, 180, 180)
                ));

                if let Some(ftype) = res.constructor {
                    output.push_str(&format!(
                        "      ↳ {}: {}\n",
                        "Args".blue(),
                        format_type_list(&ftype.parameter_types).truecolor(180, 180, 180)
                    ));

                    output.push_str(&format!("      {} \n", "🔧 Methods:".bold().bright_green(),));

                    for (mname, mtype) in &res.methods {
                        output.push_str(&format!("           {}\n", mname.bright_magenta()));

                        let parameter_types = &mtype.parameter_types;

                        let formatted = if !parameter_types.is_empty() {
                            format_type_list(&parameter_types[1..]).truecolor(180, 180, 180)
                        } else {
                            format_type_list(&[]).truecolor(180, 180, 180)
                        };

                        output.push_str(&format!(
                            "             ↳ {}: {}\n",
                            "Args".blue(),
                            formatted
                        ));

                        output.push_str(&format!(
                            "             ↳ {}: {}\n",
                            "Returns".blue(),
                            format_type_list(&mtype.return_type).truecolor(180, 180, 180)
                        ));
                    }
                }
            }
        }
    }

    println!("{output}");
}

fn format_type_list(types: &[InferredType]) -> String {
    if types.is_empty() {
        "()".to_string()
    } else {
        types
            .iter()
            .map(|t| wasm_wave::wasm::DisplayType(&AnalysedType::try_from(t).unwrap()).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Default)]
struct HierarchyNode<'a> {
    functions: Vec<(String, &'a FunctionType)>,
    resources: BTreeMap<String, ResourceNode<'a>>,
}

#[derive(Default)]
struct ResourceNode<'a> {
    constructor: Option<&'a FunctionType>,
    methods: Vec<(String, &'a FunctionType)>,
}

fn print_rib_compilation_error(error: &RibCompilationError) {
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
            let position = compilation_error.expr.source_span();

            println!("{}", "[compilation error]".red().bold());
            println!("{} {}", "[position]".yellow(), position.start_column());
            println!(
                "{} {}",
                "[expression]".yellow(),
                compilation_error.expr.to_string().white()
            );
            println!("{} {}", "[cause]".yellow(), cause.bright_red().bold());

            if !compilation_error.additional_error_details.is_empty() {
                for detail in &compilation_error.additional_error_details {
                    println!("{} {}", "[help]".yellow(), detail.cyan());
                }
            }

            if !compilation_error.help_messages.is_empty() {
                for message in &compilation_error.help_messages {
                    println!("{} {}", "[help]".yellow(), message.cyan());
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

fn print_bootstrap_error(error: &ReplBootstrapError) {
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
