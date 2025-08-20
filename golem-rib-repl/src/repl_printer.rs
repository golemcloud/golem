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
use golem_wasm_ast::analysis::analysed_type::{record, str, u64};
use golem_wasm_ast::analysis::{AnalysedResourceMode, AnalysedType, NameTypePair, TypeHandle};
use golem_wasm_rpc::{Value, ValueAndType};
use rib::*;
use std::collections::BTreeMap;
use std::fmt::Display;

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

    fn print_components_and_exports(&self, exports: &ComponentDependencies) {
        for (component_dependency_key, component) in &exports.dependencies {
            let mut indent = Indent::new();

            println!(
                "{} {}",
                "📦 Component:".bold().bright_yellow(),
                component_dependency_key
                    .component_name
                    .bold()
                    .truecolor(180, 180, 180)
            );

            indent.add();

            if let Some(root_package) = &component_dependency_key.root_package_name {
                println!(
                    "{} {} {}",
                    indent,
                    "Root Package:".bold().bright_cyan(),
                    root_package.bold().truecolor(180, 180, 180)
                );

                indent.add();
            }

            if let Some(root_interface) = &component_dependency_key.root_package_version {
                println!(
                    "{} {} {}",
                    indent,
                    "Root Package Version:".bold().bright_cyan(),
                    root_interface.bold().truecolor(180, 180, 180)
                );

                indent.add();
            }

            print_function_dictionary(&mut indent, component)
        }
    }

    fn print_rib_compilation_error(&self, error: &RibCompilationError) {
        print_rib_compilation_error(error);
    }

    fn print_rib_result(&self, result: &RibResult) {
        match result {
            RibResult::Unit => {
                println!("{}", "()".yellow());
            }

            RibResult::Val(value_and_type) => {
                let value_str = display_for_value_and_type(value_and_type);
                let formatted = try_formatting(&value_str, 2);
                println!("{}", formatted.yellow());
            }
        }
    }

    fn print_rib_runtime_error(&self, error: &RibRuntimeError) {
        println!("{} {}", "[runtime error]".red(), error.to_string().white());
    }

    fn print_wasm_value_type(&self, analysed_type: &AnalysedType) {
        match analysed_type {
            AnalysedType::Handle(type_handle) => {
                let text = display_for_resource_handle_type(type_handle);
                println!("{} {}", "[warn]".magenta(), "the syntax below to show the resource-handle type is only used for display purposes".to_string().white());

                println!();

                println!("{}", text.yellow());
            }

            _ => println!(
                "{}",
                wasm_wave::wasm::DisplayType(analysed_type)
                    .to_string()
                    .yellow()
            ),
        }
    }
}

#[derive(Clone)]
pub struct DefaultReplResultPrinter;

impl ReplPrinter for DefaultReplResultPrinter {}

pub fn print_function_dictionary(indent: &mut Indent, dict: &FunctionDictionary) {
    let mut output = String::new();

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
            FunctionName::Variant(_) => {
                continue;
            }
            FunctionName::Enum(_) => {
                continue;
            }
        }
    }

    for (pkg, interfaces) in hierarchy {
        match pkg {
            Some(pkg) => {
                output.push_str(&format!(
                    "{} {} {}\n",
                    indent,
                    "📦 Package:".bold().bright_yellow(),
                    format!("{}:{}", pkg.namespace, pkg.package_name)
                        .bold()
                        .truecolor(180, 180, 180)
                ));

                indent.add();
            }
            None => {
                output.push_str(&format!("{}\n", "📦 Global Scope".bold().bright_yellow()));
                indent.add();
            }
        }

        for (iface, node) in interfaces {
            if let Some(iface) = iface {
                output.push_str(&format!(
                    "{} {} {}\n",
                    indent,
                    "📄 Interface:".bold().bright_cyan(),
                    iface.name.bold().truecolor(180, 180, 180)
                ));
                indent.add();
            }

            if !node.functions.is_empty() {
                output.push_str(&format!(
                    "{} {}\n",
                    indent,
                    "🔧 Functions:".bold().bright_green(),
                ));
                indent.add();
            }

            for (fname, ftype) in &node.functions {
                output.push_str(&format!("{} {}\n", indent, fname.bright_magenta()));
                indent.add();
                output.push_str(&format!(
                    "{} ↳ {}: {}\n",
                    indent,
                    "Args".blue(),
                    format_type_list(&ftype.parameter_types).truecolor(180, 180, 180)
                ));
                output.push_str(&format!(
                    "{} ↳ {}: {}\n",
                    indent,
                    "Returns".blue(),
                    format_return_type(&ftype.return_type).truecolor(180, 180, 180)
                ));
                indent.remove();
            }

            for (res_name, res) in &node.resources {
                output.push_str(&format!(
                    "{} {} {}\n",
                    indent,
                    "🧩️ Resource:".bold().bright_yellow(),
                    res_name.truecolor(180, 180, 180)
                ));

                indent.add();

                if let Some(ftype) = res.constructor {
                    output.push_str(&format!(
                        "{} ↳ {}: {}\n",
                        indent,
                        "Args".blue(),
                        format_type_list(&ftype.parameter_types).truecolor(180, 180, 180)
                    ));

                    indent.add();

                    output.push_str(&format!(
                        "{} {} \n",
                        indent,
                        "🔧 Methods:".bold().bright_green(),
                    ));

                    indent.add();

                    for (mname, mtype) in &res.methods {
                        output.push_str(&format!("{} {}\n", indent, mname.bright_magenta()));
                        indent.add();

                        let parameter_types = &mtype.parameter_types;

                        let formatted = if !parameter_types.is_empty() {
                            format_type_list(&parameter_types[1..]).truecolor(180, 180, 180)
                        } else {
                            format_type_list(&[]).truecolor(180, 180, 180)
                        };

                        output.push_str(&format!(
                            "{} ↳ {}: {}\n",
                            indent,
                            "Args".blue(),
                            formatted
                        ));

                        output.push_str(&format!(
                            "{} ↳ {}: {}\n",
                            indent,
                            "Returns".blue(),
                            format_return_type(&mtype.return_type).truecolor(180, 180, 180)
                        ));

                        indent.remove();
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

fn format_return_type(typ: &Option<InferredType>) -> String {
    typ.as_ref()
        .map(|t| wasm_wave::wasm::DisplayType(&AnalysedType::try_from(t).unwrap()).to_string())
        .unwrap_or_else(|| "()".to_string())
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
            let position = &compilation_error.source_span;

            println!("{}", "[compilation error]".red().bold());
            println!("{} {}", "[position]".yellow(), position.start_column());
            println!(
                "{} {}",
                "[expression]".yellow(),
                compilation_error
                    .expr
                    .as_ref()
                    .map(|x| x.to_string())
                    .unwrap_or_default()
                    .white()
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

// Only used for displaying since Wasm Wave is yet to support resource handle types
fn display_for_resource_handle_type(type_handle: &TypeHandle) -> String {
    let resource_id = &type_handle.resource_id.0;
    let uri = &type_handle.mode;

    let mode = match uri {
        AnalysedResourceMode::Owned => "owned",
        AnalysedResourceMode::Borrowed => "borrowed",
    };

    format!("handle<resource-id:{resource_id}, mode:{mode}>")
}

pub struct Indent {
    level: usize,
}

impl Default for Indent {
    fn default() -> Self {
        Self::new()
    }
}

impl Indent {
    pub fn new() -> Self {
        Self { level: 0 }
    }

    pub fn add(&mut self) {
        self.level += 2;
    }

    pub fn remove(&mut self) {
        if self.level >= 2 {
            self.level -= 2;
        }
    }
}

impl Display for Indent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", " ".repeat(self.level))?;
        Ok(())
    }
}

// To intercept any presence of resource handle and therefore inspecting each element
// instead of value_and_type.to_string()
fn display_for_value_and_type(value_and_type: &ValueAndType) -> String {
    match &value_and_type.value {
        Value::Bool(_) => value_and_type.to_string(),
        Value::U8(_) => value_and_type.to_string(),
        Value::U16(_) => value_and_type.to_string(),
        Value::U32(_) => value_and_type.to_string(),
        Value::U64(_) => value_and_type.to_string(),
        Value::S8(_) => value_and_type.to_string(),
        Value::S16(_) => value_and_type.to_string(),
        Value::S32(_) => value_and_type.to_string(),
        Value::S64(_) => value_and_type.to_string(),
        Value::F32(_) => value_and_type.to_string(),
        Value::F64(_) => value_and_type.to_string(),
        Value::Char(_) => value_and_type.to_string(),
        Value::String(_) => value_and_type.to_string(),
        Value::List(values) => {
            let inner_type = match &value_and_type.typ {
                AnalysedType::List(inner_type) => inner_type.inner.as_ref(),
                _ => panic!("Expected a list type"),
            };

            let mut string = "[".to_string();
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    string.push_str(", ");
                }
                let inner_value_and_type = ValueAndType::new(value.clone(), inner_type.clone());
                let inner_value_and_type = display_for_value_and_type(&inner_value_and_type);
                string.push_str(&inner_value_and_type.to_string());
            }

            string.push(']');

            string
        }
        Value::Tuple(tuple) => {
            let inner_types = match &value_and_type.typ {
                AnalysedType::Tuple(inner_types) => inner_types.items.clone(),
                _ => panic!("Expected a tuple type"),
            };

            let mut string = "(".to_string();
            for (i, value) in tuple.iter().enumerate() {
                if i > 0 {
                    string.push_str(", ");
                }
                let inner_value_and_type = ValueAndType::new(value.clone(), inner_types[i].clone());
                let inner_value_and_type = display_for_value_and_type(&inner_value_and_type);
                string.push_str(&inner_value_and_type);
            }
            string.push(')');

            string
        }
        Value::Record(values) => {
            let inner_types = match &value_and_type.typ {
                AnalysedType::Record(inner_types) => inner_types.fields.clone(),
                _ => panic!("Expected a record type"),
            };

            let mut string = "{".to_string();
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    string.push_str(", ");
                }
                let inner_value_and_type =
                    ValueAndType::new(value.clone(), inner_types[i].typ.clone());
                let inner_value_and_type = display_for_value_and_type(&inner_value_and_type);
                string.push_str(&format!(
                    "{}: {}",
                    inner_types[i].name, inner_value_and_type
                ));
            }
            string.push('}');

            string
        }
        Value::Variant {
            case_idx,
            case_value,
        } => {
            let variant_type = match &value_and_type.typ {
                AnalysedType::Variant(variant_type) => variant_type,
                _ => panic!("Expected a variant type"),
            };

            let case_name = variant_type
                .cases
                .get(*case_idx as usize)
                .map_or("unknown", |c| &c.name);

            match case_value {
                Some(value) => {
                    let inner_value_and_type = ValueAndType::new(
                        value.as_ref().clone(),
                        variant_type.cases[*case_idx as usize].clone().typ.unwrap(),
                    );

                    let inner_value_and_type = display_for_value_and_type(&inner_value_and_type);
                    format!("{case_name}({inner_value_and_type})")
                }

                None => {
                    // If the case has no value, just return the case name
                    case_name.to_string()
                }
            }
        }
        Value::Enum(case_index) => {
            let enum_type = match &value_and_type.typ {
                AnalysedType::Enum(enum_type) => enum_type,
                _ => panic!("Expected an enum type"),
            };

            let case_name = enum_type
                .cases
                .get(*case_index as usize)
                .unwrap_or_else(|| {
                    panic!("Enum case index out of bounds: {case_index}");
                });
            case_name.to_string()
        }
        Value::Flags(bool_list) => {
            let flags_type = match &value_and_type.typ {
                AnalysedType::Flags(flags_type) => flags_type,
                _ => panic!("Expected a flags type"),
            };

            let mut string = "{".to_string();
            for (i, value) in bool_list.iter().enumerate() {
                if i > 0 {
                    string.push_str(", ");
                }
                let flag_name = flags_type.names.get(i).unwrap_or_else(|| {
                    panic!("Flags index out of bounds: {i}");
                });

                if *value {
                    string.push_str(flag_name);
                }
            }
            string.push('}');

            string
        }
        Value::Option(option) => {
            let inner_type = match &value_and_type.typ {
                AnalysedType::Option(inner_type) => inner_type.inner.as_ref(),
                _ => panic!("Expected an option type"),
            };

            match option {
                Some(value) => {
                    let inner_value_and_type =
                        ValueAndType::new(value.as_ref().clone(), inner_type.clone());
                    let inner_value_and_type = display_for_value_and_type(&inner_value_and_type);
                    format!("some({inner_value_and_type})")
                }
                None => "none".to_string(),
            }
        }
        Value::Result(result) => {
            let x: &Result<Option<Box<Value>>, Option<Box<Value>>> = result;

            let ok_inner_type: Option<&Box<AnalysedType>> = match &value_and_type.typ {
                AnalysedType::Result(inner_type) => inner_type.ok.as_ref(),
                _ => panic!("Expected a result type"),
            };

            let err_inner_type: Option<&Box<AnalysedType>> = match &value_and_type.typ {
                AnalysedType::Result(inner_type) => inner_type.err.as_ref(),
                _ => panic!("Expected a result type"),
            };

            match x {
                Ok(Some(value)) => {
                    let inner_value_and_type = ValueAndType::new(
                        value.as_ref().clone(),
                        ok_inner_type.unwrap().as_ref().clone(),
                    );
                    let inner_value_and_type = display_for_value_and_type(&inner_value_and_type);
                    format!("ok({inner_value_and_type})")
                }
                Ok(None) => "ok".to_string(),
                Err(Some(value)) => {
                    let inner_value_and_type = ValueAndType::new(
                        value.as_ref().clone(),
                        err_inner_type.unwrap().as_ref().clone(),
                    );
                    let inner_value_and_type = display_for_value_and_type(&inner_value_and_type);
                    format!("err({inner_value_and_type})")
                }
                Err(None) => "err".to_string(),
            }
        }
        Value::Handle { uri, resource_id } => display_for_resource_handle(uri, resource_id),
    }
}

fn display_for_resource_handle(uri: &str, resource_id: &u64) -> String {
    let resource = Value::Record(vec![
        Value::String(uri.to_string()),
        Value::U64(*resource_id),
    ]);

    let analysed_type = record(vec![
        NameTypePair {
            name: "uri".to_string(),
            typ: str(),
        },
        NameTypePair {
            name: "resource-id".to_string(),
            typ: u64(),
        },
    ]);

    let result = ValueAndType::new(resource, analysed_type);

    result.to_string()
}

fn try_formatting(input: &str, _indent: usize) -> String {
    let mut result = String::new();
    let mut depth = 0;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '{' | '[' => {
                // Check for empty object or array
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                    result.push(chars[i]);
                    result.push(chars[j]);
                    i = j + 1;
                    continue;
                }

                depth += 1;
                result.push(chars[i]);
                result.push('\n');
                result.push_str(&"  ".repeat(depth));
                i += 1;
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                result.push('\n');
                result.push_str(&"  ".repeat(depth));
                result.push(chars[i]);
                i += 1;
            }
            ',' => {
                result.push(chars[i]);
                result.push('\n');
                result.push_str(&"  ".repeat(depth));
                i += 1;
                // Skip whitespace after comma
                while i < chars.len() && chars[i].is_whitespace() && chars[i] != '\n' {
                    i += 1;
                }
            }
            _ => {
                result.push(chars[i]);
                i += 1;
            }
        }
    }

    result
}
