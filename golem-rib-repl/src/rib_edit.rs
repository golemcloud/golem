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

use crate::compiler::{CompilerOutput, InstanceVariables};
use crate::value_generator::generate_value;
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
    pub compiler_output: Option<CompilerOutput>,
    pub key_words: Vec<&'static str>,
    pub std_function_names: Vec<&'static str>,
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

    pub fn init() -> RibEdit {
        RibEdit {
            compiler_output: None,
            key_words: vec![
                "let", "if", "else", "match", "for", "in", "true", "false", "yield", "some",
                "none", "ok", "err",
            ],
            std_function_names: vec!["instance"],
        }
    }
    pub fn update_progression(&mut self, compiler_output: &CompilerOutput) {
        self.compiler_output = Some(compiler_output.clone());
    }

    fn backtrack_and_get_start_pos(line: &str, end_pos: usize) -> usize {
        let mut start = end_pos;

        while start > 0
            && line[start - 1..start].chars().all(|c| {
                c.is_alphanumeric() || c == '_' || c == '.' || c == '-' || c == '(' || c == ')'
            })
        {
            start -= 1;
        }

        start
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

                    return Ok(Some((end_pos, completions))); // Only one possible completion, return early
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
    ) -> rustyline::Result<Option<(usize, Vec<String>)>> {
        let Some(variants) = self.variants() else {
            return Ok(None);
        };

        let mut completions = Vec::new();

        for variant in variants.iter() {
            for case in variant.cases.iter() {
                if case.name.starts_with(word) {
                    completions.push(case.name.clone());
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
            instance_variables.clone().map(|x| x.variable_names());

        let mut completions = Vec::new();

        let start = Self::backtrack_and_get_start_pos(line, end_pos);

        let word = &line[start..end_pos];

        // Check if the word is a method call
        if let Some((new_start, new_completions)) =
            Self::complete_method_calls(word, instance_variables, start, end_pos)?
        {
            completions.extend(new_completions);
            return Ok((new_start, completions));
        }

        if let Some((new_start, new_completions)) = self.complete_variants(word, start)? {
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
            instance_variables.clone().map(|x| x.variable_names());

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
                // return only remaining part of the variable name
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
        let expr = Expr::from_text(input.strip_suffix(";").unwrap_or(input));

        match expr {
            Ok(_) => Ok(ValidationResult::Valid(None)),
            Err(e) => Ok(ValidationResult::Invalid(Some(e))),
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
        let mut chars = line.chars().peekable();

        while let Some(c) = chars.next() {
            if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                word.push(c);
            } else {
                if !word.is_empty() {
                    highlighted.push_str(&highlight_word(&word, self, identifiers, instance_vars));
                    word.clear();
                }
                highlighted.push(c);
            }
        }

        if !word.is_empty() {
            highlighted.push_str(&highlight_word(&word, self, identifiers, instance_vars));
        }

        Cow::Owned(highlighted)
    }
}

fn highlight_word(
    word: &str,
    context: &RibEdit,
    identifiers: Option<&Vec<VariableId>>,
    instance_vars: Option<&InstanceVariables>,
) -> String {
    // Keyword
    if context.key_words.contains(&word) {
        return word.cyan().to_string();
    }

    // Method call (e.g., obj.method)
    if let Some((obj, method)) = word.split_once('.') {
        let is_instance = instance_vars.map_or(false, |vars| {
            vars.instance_keys().contains(&obj.to_string())
        });

        let is_method = instance_vars.map_or(false, |vars| {
            vars.method_names().contains(&method.to_string())
        });

        if is_instance && is_method {
            return format!("{}.{}", obj.blue(), method.green());
        } else {
            return word.to_string();
        }
    }

    // Identifier
    let is_identifier = identifiers.map_or(false, |vars| vars.iter().any(|var| var.name() == word));

    if is_identifier {
        return word.blue().to_string();
    }

    // Instance variable
    let is_instance_var = instance_vars.map_or(false, |vars| {
        vars.instance_keys().contains(&word.to_string())
    });

    if is_instance_var {
        return word.cyan().to_string();
    }

    // Standard function name
    if context.std_function_names.contains(&word) {
        return word.green().to_string();
    }

    // Numeric literals
    if word.chars().all(|ch| ch.is_numeric()) {
        return word.green().to_string();
    }

    // Fallback: no highlighting
    word.to_string()
}
