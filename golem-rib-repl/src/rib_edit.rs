use crate::compiler::{CompilerOutput, InstanceVariables};
use colored::Colorize;
use rib::{Expr, InferredExpr};
use rustyline::completion::Completer;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::History;
use rustyline::validate::{ValidationResult, Validator};
use rustyline::{Context, Helper};
use std::borrow::Cow;

#[derive(Default)]
pub struct RibEdit {
    pub progressed_inferred_expr: Option<InferredExpr>,
    pub instance_variables: Option<InstanceVariables>,
    pub key_words: Vec<&'static str>,
    pub function_names: Vec<&'static str>,
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
            function_names: vec!["instance"],
        }
    }
    pub fn update_progression(&mut self, compiler_output: &CompilerOutput) {
        self.progressed_inferred_expr = Some(compiler_output.inferred_expr.clone());
        self.instance_variables = Some(compiler_output.instance_variables.clone());
    }
}

impl Helper for RibEdit {}

impl Completer for RibEdit {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>, // a context has access to only the current line
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let instance_variables: Option<InstanceVariables> = self.instance_variables.clone();
        let instance_variable_names: Option<Vec<String>> = instance_variables.clone().map(|x| x.variable_names());

        let mut completions = Vec::new();


        if let Some(dot_pos) = line[..pos].rfind('.') {
            let instance_var_name = &line[..dot_pos];

            if let Some(instance_vars) = &instance_variables {
                if let Some(func_dict) = instance_vars.instance_variables.get(instance_var_name) {
                    let prefix = &line[dot_pos + 1..pos];

                    for (name, tpe) in func_dict.map {
                        dbg!(tpe.clone());

                        if name.name().starts_with(prefix) {
                            completions.push(name.name());
                        }

                    }

                    return Ok((dot_pos + 1, completions)); // Return function completions
                }
            }
        }


        let mut start = pos;

        while start > 0
            && line[start - 1..start]
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_')
        {
            start -= 1;
        }

        let word = &line[start..pos];

        if !word.is_empty() {
            if let Some(variables) = instance_variable_names {
                for var in variables.iter() {
                    if var.starts_with(word) {
                        completions.push(var.clone());
                    }
                }
            }

            completions.extend(
                self.key_words
                    .iter()
                    .filter(|&&kw| kw.starts_with(word))
                    .map(|&kw| kw.to_string()),
            );

            completions.extend(
                self.function_names
                    .iter()
                    .filter(|&&fn_name| fn_name.starts_with(word))
                    .map(|&fn_name| format!("{}(", fn_name)),
            )
        }

        Ok((start, completions))
    }
}

impl Hinter for RibEdit {
    type Hint = String;
}

impl Validator for RibEdit {
    fn validate(
        &self,
        context: &mut rustyline::validate::ValidationContext,
    ) -> rustyline::Result<ValidationResult> {
        // Implement validation logic here (e.g., check for balanced parentheses)

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
        let mut chars = line.chars().peekable();

        while let Some(c) = chars.next() {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
            } else {
                if !word.is_empty() {
                    if self.key_words.contains(&word.as_str()) {
                        highlighted.push_str(&format!("{}", word.blue()));
                    } else if self.function_names.contains(&word.as_str()) {
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
