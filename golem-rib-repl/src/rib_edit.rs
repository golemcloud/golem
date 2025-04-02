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
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::ValueAndType;
use rib::RibResult::Val;
use crate::value_generator::generate_value;

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

    fn backtrack_and_get_start_pos(line: &str, end_pos: usize) -> usize {
        let mut start = end_pos;

        while start > 0
            && line[start - 1..start]
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' ||  c == '.' || c == '-' || c == '(' || c == ')')
        {
            start -= 1;
        }

        start
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
        let instance_variable_names: Option<Vec<String>> = instance_variables.clone().map(|x| x.variable_names());

        let mut completions = Vec::new();

        let start= Self::backtrack_and_get_start_pos(line, end_pos);

        let word = &line[start..end_pos];

        if word.contains('.') {
            let dot_pos = word.rfind('.').unwrap();
            let possible_instance_variable_name = &word[..dot_pos];
            let possible_method_name = &word[dot_pos + 1..];

            if let Some(instance_vars) = &instance_variables {
                if let Some(func_dict) = instance_vars.instance_variables.get(possible_instance_variable_name) {
                    for (function, tpe) in func_dict.map.iter() {
                        // Allow completion only if user has typed in `(`
                        let name_with_paren = format!("{}(", function.name());

                        if name_with_paren == possible_method_name {
                            let args = tpe.parameter_types();
                            let mut arg_values = vec![];

                            for arg in args {
                                let analysed_type = AnalysedType::try_from(arg).ok();

                                if let Some(analysed_type) = analysed_type {
                                    let value = generate_value(&analysed_type);
                                    arg_values.push(ValueAndType::new(value, analysed_type));
                                }
                            }

                            let args =
                                arg_values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");

                            let with_paren = format!("{})",  args);
                            completions.push(with_paren);
                            // we break here because there is only one choice of args
                            return Ok((end_pos, completions));

                        } else if function.name().starts_with(possible_method_name) {
                            completions.push(function.name());
                        }
                    }

                    return Ok((start + dot_pos + 1, completions)); // Return function completions
                }
            }
        }

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
                    .map(|&fn_name| fn_name.to_string()),
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
