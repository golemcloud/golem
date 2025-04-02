use crate::compiler::{CompilerOutput, InstanceVariables};
use crate::value_generator::generate_value;
use colored::Colorize;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::ValueAndType;
use rib::{Expr, InferredExpr, VariableId};
use rustyline::completion::Completer;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationResult, Validator};
use rustyline::{Context, Helper};
use std::borrow::Cow;

#[derive(Default)]
pub struct RibEdit {
    pub progressed_inferred_expr: Option<InferredExpr>,
    pub instance_variables: Option<InstanceVariables>,
    pub identifiers: Vec<VariableId>,
    pub key_words: Vec<&'static str>,
    pub std_function_names: Vec<&'static str>,
}

impl RibEdit {
    pub fn init() -> RibEdit {
        RibEdit {
            progressed_inferred_expr: None,
            instance_variables: None,
            key_words: vec![
                "let", "if", "else", "match", "for", "in", "true", "false", "yield", "some",
                "none", "ok", "err",
            ],
            std_function_names: vec!["instance"],
            identifiers: vec![],
        }
    }
    pub fn update_progression(&mut self, compiler_output: &CompilerOutput) {
        self.progressed_inferred_expr = Some(compiler_output.inferred_expr.clone());
        self.instance_variables = Some(compiler_output.instance_variables.clone());
        self.identifiers = compiler_output.identifiers.clone();
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

        let Some(func_dict) = instance_vars.instance_variables.get(instance_var_name) else {
            return Ok(None);
        };

        let mut completions = Vec::new();

        for (function, tpe) in &func_dict.map {
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

                return Ok(Some((end_pos, completions))); // Only one possible completion, return early
            }

            // Otherwise, suggest method names
            if function.name().starts_with(method_prefix) {
                completions.push(function.name());
            }
        }

        Ok(Some((start + dot_pos + 1, completions)))
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
        let instance_variables: Option<InstanceVariables> = self.instance_variables.clone();
        let instance_variable_names: Option<Vec<String>> =
            instance_variables.clone().map(|x| x.variable_names());

        let mut completions = Vec::new();

        let start = Self::backtrack_and_get_start_pos(line, end_pos);

        let word = &line[start..end_pos];

        // Check if the word is a method call
        if let Some((new_start, new_completions)) =
            Self::complete_method_calls(word, instance_variables.as_ref(), start, end_pos)?
        {
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

            for var in self.identifiers.iter() {
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
        let instance_variables: Option<InstanceVariables> = self.instance_variables.clone();
        let instance_variable_names: Option<Vec<String>> =
            instance_variables.clone().map(|x| x.variable_names());

        let start = Self::backtrack_and_get_start_pos(line, pos);
        let word = &line[start..pos];

        if word.is_empty() {
            return None;
        }

        if let Some(variables) = instance_variable_names {
            for var in variables.iter() {
                if var.starts_with(word) {
                    // return only remaining part of the variable name
                    let hint = &var[word.len()..];
                    return Some(hint.to_string());
                }
            }
        }

        for var in self.identifiers.iter() {
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
        let expr = Expr::from_text(context.input());

        match expr {
            Ok(_) => Ok(ValidationResult::Valid(None)),
            Err(e) => Ok(ValidationResult::Invalid(Some(e))),
        }
    }
}

impl Highlighter for RibEdit {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        let mut highlighted = String::new();
        let mut word = String::new();
        let chars = line.chars().peekable();

        for c in chars {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
            } else {
                if !word.is_empty() {
                    if self.key_words.contains(&word.as_str()) {
                        highlighted.push_str(&format!("{}", word.blue()));
                    } else if self.std_function_names.contains(&word.as_str()) {
                        highlighted.push_str(&format!("{}", word.cyan()));
                    } else if word.chars().all(|ch| ch.is_numeric()) {
                        highlighted.push_str(&format!("{}", word.yellow()));
                    } else {
                        highlighted.push_str(&word);
                    }
                    word.clear();
                }
                highlighted.push(c);
            }
        }

        if !word.is_empty() {
            if self.key_words.contains(&word.as_str()) {
                highlighted.push_str(&format!("{}", word.blue()));
            } else {
                highlighted.push_str(&word);
            }
        }

        Cow::Owned(highlighted)
    }
}
