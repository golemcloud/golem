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

use crate::compiler::{InstanceVariables, ReplCompilerOutput};
use crate::value_generator::generate_value;
use crate::CommandRegistry;
use colored::Colorize;
use golem_wasm_ast::analysis::{AnalysedType, TypeEnum, TypeVariant};
use golem_wasm_rpc::ValueAndType;
use rib::{Expr, VariableId};
use rustyline::completion::Completer;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationResult, Validator};
use rustyline::{Context, Helper};
use std::borrow::Cow;

#[derive(Default)]
pub struct RibEdit {
    pub compiler_output: Option<ReplCompilerOutput>,
    pub key_words: Vec<&'static str>,
    pub std_function_names: Vec<&'static str>,
    pub repl_commands: Vec<String>,
}

impl RibEdit {
    pub fn instance_variables(&self) -> Option<&InstanceVariables> {
        self.compiler_output
            .as_ref()
            .map(|output| &output.instance_variables)
    }

    pub fn identifiers(&self) -> Option<&Vec<VariableId>> {
        self.compiler_output
            .as_ref()
            .map(|output| &output.identifiers)
    }

    pub fn variants(&self) -> Option<&Vec<TypeVariant>> {
        self.compiler_output.as_ref().map(|output| &output.variants)
    }

    pub fn enums(&self) -> Option<&Vec<TypeEnum>> {
        self.compiler_output.as_ref().map(|output| &output.enums)
    }

    pub fn init(command_registry: &CommandRegistry) -> RibEdit {
        RibEdit {
            compiler_output: None,
            key_words: vec![
                "let", "if", "else", "match", "for", "in", "true", "false", "yield", "some",
                "none", "ok", "err",
            ],
            std_function_names: vec!["instance"],
            repl_commands: command_registry.get_commands(),
        }
    }
    pub fn update_progression(&mut self, compiler_output: &ReplCompilerOutput) {
        self.compiler_output = Some(compiler_output.clone());
    }

    fn backtrack_and_get_start_pos(line: &str, end_pos: usize) -> usize {
        line[0..end_pos]
            .char_indices()
            .rev()
            .find_map(|(pos, c)| {
                let is_token_char =
                    c.is_alphanumeric() || c == '_' || c == '.' || c == '-' || c == '(' || c == ')';
                (!is_token_char).then(|| pos + c.len_utf8())
            })
            .unwrap_or(0)
    }

    fn complete_commands(&self, word: &str, start: usize) -> Option<(usize, Vec<String>)> {
        let commands = self.repl_commands.clone();

        let completions: Vec<String> = commands
            .into_iter()
            .filter(|cmd| cmd.starts_with(word))
            .collect();

        if completions.is_empty() {
            None
        } else {
            Some((start, completions))
        }
    }

    fn complete_method_calls(
        word: &str,
        instance_variables: Option<&InstanceVariables>,
        start: usize,
        end_pos: usize,
    ) -> rustyline::Result<Option<(usize, Vec<String>)>> {
        if !word.contains('.') {
            return Ok(None);
        }

        let dot_pos = word.rfind('.').unwrap();
        let instance_var_name = &word[..dot_pos];
        let method_prefix = &word[dot_pos + 1..];

        let Some(instance_vars) = instance_variables else {
            return Ok(None);
        };

        let mut completions = Vec::new();

        if let Some(worker_instance_func_dict) =
            instance_vars.get_worker_instance_method_dict(instance_var_name)
        {
            for (function, tpe) in &worker_instance_func_dict.name_and_types {
                let name_with_paren = format!("{}(", function.name());

                // If user has typed in `(`, complete the method call with arguments
                if name_with_paren == method_prefix {
                    let args = tpe
                        .parameter_types()
                        .iter()
                        .filter_map(|arg| AnalysedType::try_from(arg).ok())
                        .map(|analysed_type| {
                            ValueAndType::new(generate_value(&analysed_type), analysed_type)
                        })
                        .collect::<Vec<_>>();

                    let args_str = args
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    completions.push(format!("{})", args_str));

                    return Ok(Some((end_pos, completions)));
                }

                if function.name().starts_with(method_prefix) {
                    completions.push(function.name());
                }
            }
        }

        if let Some(resource_instance_func_dict) =
            instance_vars.get_resource_instance_method_dict(instance_var_name)
        {
            for (resource_method_name, tpe) in &resource_instance_func_dict.name_and_types {
                let resource_method_with_paren = format!("{}(", resource_method_name.name());

                // If user has typed in `(`, complete the method call with arguments
                if resource_method_with_paren == method_prefix {
                    let args = tpe
                        .parameter_types()
                        .iter()
                        .skip(1) // Skip the first argument, which is the instance itself
                        .filter_map(|arg| AnalysedType::try_from(arg).ok())
                        .map(|analysed_type| {
                            ValueAndType::new(generate_value(&analysed_type), analysed_type)
                        })
                        .collect::<Vec<_>>();

                    let args_str = args
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    completions.push(format!("{})", args_str));

                    return Ok(Some((end_pos, completions)));
                }

                if resource_method_name.name().starts_with(method_prefix) {
                    completions.push(resource_method_name.name());
                }
            }
        }

        if completions.is_empty() {
            Ok(None)
        } else {
            Ok(Some((start + dot_pos + 1, completions)))
        }
    }

    pub fn complete_variants(
        &self,
        word: &str,
        start: usize,
        end_pos: usize,
    ) -> rustyline::Result<Option<(usize, Vec<String>)>> {
        let Some(variants) = self.variants() else {
            return Ok(None);
        };

        let mut completions = Vec::new();

        for variant in variants.iter() {
            for case in variant.cases.iter() {
                let variant_name = &case.name;

                let name_with_paren = format!("{}(", variant_name);

                if word == name_with_paren {
                    if let Some(variant_arg_type) = &case.typ {
                        let generated_value = generate_value(variant_arg_type);
                        let value_and_type =
                            ValueAndType::new(generated_value, variant_arg_type.clone());
                        let arg_str = value_and_type.to_string();
                        completions.push(format!("{})", arg_str));
                        return Ok(Some((end_pos, completions)));
                    }
                }

                if case.name.starts_with(word) {
                    completions.push(variant_name.clone());
                }
            }
        }

        if completions.is_empty() {
            Ok(None)
        } else {
            Ok(Some((start, completions)))
        }
    }

    pub fn complete_enums(
        &self,
        word: &str,
        start: usize,
    ) -> rustyline::Result<Option<(usize, Vec<String>)>> {
        let Some(enums) = self.enums() else {
            return Ok(None);
        };

        let mut completions = Vec::new();

        for enum_ in enums.iter() {
            for case in enum_.cases.iter() {
                if case.starts_with(word) {
                    completions.push(case.clone());
                }
            }
        }

        if completions.is_empty() {
            Ok(None)
        } else {
            Ok(Some((start, completions)))
        }
    }
}

impl Helper for RibEdit {}

impl Completer for RibEdit {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        end_pos: usize,
        _ctx: &Context<'_>, // a context has access to only the current line
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let instance_variables: Option<&InstanceVariables> = self.instance_variables();
        let instance_variable_names: Option<Vec<String>> =
            instance_variables.map(|x| x.variable_names());

        let mut completions = Vec::new();

        let start = Self::backtrack_and_get_start_pos(line, end_pos);

        let word = &line[start..end_pos];

        if line.starts_with(":") {
            if let Some((new_start, new_completions)) = self.complete_commands(word, start) {
                completions.extend(new_completions);
                return Ok((new_start, completions));
            }
        }

        if let Some((new_start, new_completions)) =
            Self::complete_method_calls(word, instance_variables, start, end_pos)?
        {
            completions.extend(new_completions);
            return Ok((new_start, completions));
        }

        if let Some((new_start, new_completions)) = self.complete_variants(word, start, end_pos)? {
            completions.extend(new_completions);
            return Ok((new_start, completions));
        }

        if let Some((new_start, new_completions)) = self.complete_enums(word, start)? {
            completions.extend(new_completions);
            return Ok((new_start, completions));
        }

        if !word.is_empty() {
            if let Some(variables) = instance_variable_names {
                for var in variables.iter() {
                    if var.starts_with(word) {
                        completions.push(var.clone());
                    }
                }
            }

            for var in self.identifiers().unwrap_or(&vec![]).iter() {
                if var.name().starts_with(word) {
                    completions.push(var.name());
                }
            }

            completions.extend(
                self.key_words
                    .iter()
                    .filter(|&&kw| kw.starts_with(word))
                    .map(|&kw| kw.to_string()),
            );

            completions.extend(
                self.std_function_names
                    .iter()
                    .filter(|&&fn_name| fn_name.starts_with(word))
                    .map(|&fn_name| fn_name.to_string()),
            )
        }

        Ok((start, completions))
    }
}

impl Hinter for RibEdit {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        let instance_variables: Option<&InstanceVariables> = self.instance_variables();
        let instance_variable_names: Option<Vec<String>> =
            instance_variables.map(|x| x.variable_names());

        let start = Self::backtrack_and_get_start_pos(line, pos);
        let word = &line[start..pos];

        if word.is_empty() {
            return None;
        }

        if let Some(variables) = instance_variable_names {
            for var in variables.iter() {
                if let Some(hint) = var.strip_prefix(word) {
                    // return only remaining part of the variable name
                    return Some(hint.to_string());
                }
            }
        }

        for var in self.identifiers().unwrap_or(&vec![]).iter() {
            if var.name().starts_with(word) {
                let hint = &var.name()[word.len()..];
                return Some(hint.to_string());
            }
        }

        None
    }
}

impl Validator for RibEdit {
    fn validate(
        &self,
        context: &mut rustyline::validate::ValidationContext,
    ) -> rustyline::Result<ValidationResult> {
        let input = context.input();

        if input.starts_with(":") {
            return Ok(ValidationResult::Valid(None));
        }

        // Note that this is not compiling or parsing the entire Rib program, but syntax checking
        // the single statement
        let expr = Expr::from_text(input.strip_suffix(";").unwrap_or(input));

        match expr {
            Ok(_) => Ok(ValidationResult::Valid(None)),
            Err(err) => Ok(ValidationResult::Invalid(Some(format!("\n{}\n", err)))),
        }
    }
}

impl Highlighter for RibEdit {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        let identifiers = self
            .compiler_output
            .as_ref()
            .map(|output| &output.identifiers);

        let instance_vars = self
            .compiler_output
            .as_ref()
            .map(|output| &output.instance_variables);

        let mut highlighted = String::new();

        let mut word = String::new();

        let chars = line.chars().peekable();

        // if line.starts_with(":") {
        //     // If the line starts with ":", treat it as a command
        //     highlighted.push_str(&line[..2]);
        // }

        for c in chars {
            // accumulate code characters
            if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                word.push(c);
            } else {
                if !word.is_empty() {
                    if word.starts_with(":") {
                        highlighted.push_str(&highlight_command(&word, self));
                    } else {
                        // Highlight code identifiers, instance variables, and keywords
                        highlighted.push_str(&highlight_code(
                            &word,
                            self,
                            identifiers,
                            instance_vars,
                        ));
                    }
                    word.clear();
                }
                highlighted.push(c);
            }
        }

        if !word.is_empty() {
            highlighted.push_str(&highlight_code(&word, self, identifiers, instance_vars));
        }

        Cow::Owned(highlighted)
    }
}

fn highlight_command(word: &str, context: &RibEdit) -> String {
    if context.repl_commands.contains(&word.to_string()) {
        return word.yellow().to_string();
    }
    word.to_string()
}

fn highlight_code(
    word: &str,
    context: &RibEdit,
    identifiers: Option<&Vec<VariableId>>,
    instance_vars: Option<&InstanceVariables>,
) -> String {
    if context.key_words.contains(&word) {
        return word.cyan().to_string();
    }

    if let Some((obj, method)) = word.split_once('.') {
        let is_instance =
            instance_vars.is_some_and(|vars| vars.instance_keys().contains(&obj.to_string()));

        let is_method =
            instance_vars.is_some_and(|vars| vars.method_names().contains(&method.to_string()));

        if is_instance && is_method {
            return format!("{}.{}", obj.cyan(), method.green());
        } else {
            return word.to_string();
        }
    }

    let is_identifier = identifiers.is_some_and(|vars| vars.iter().any(|var| var.name() == word));

    if is_identifier {
        return word.cyan().to_string();
    }

    let is_instance_var =
        instance_vars.is_some_and(|vars| vars.instance_keys().contains(&word.to_string()));

    if is_instance_var {
        return word.cyan().to_string();
    }

    if context.std_function_names.contains(&word) {
        return word.green().to_string();
    }

    if word.chars().all(|ch| ch.is_numeric()) {
        return word.green().to_string();
    }

    if word.starts_with("\"") && word.ends_with("\"") {
        return word.truecolor(152, 195, 121).to_string();
    }

    word.to_string()
}
