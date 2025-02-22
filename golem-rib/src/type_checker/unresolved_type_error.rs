use std::collections::VecDeque;
use crate::type_checker::{Path, PathElem, PathType};
use crate::{Expr, InferredType, TypeName};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt;
use std::fmt::Display;

#[derive(Clone, Debug)]
pub struct UnResolvedTypesError {
    pub unresolved_expr: Expr,
    pub parent_expr: Option<Expr>,
    pub additional_messages: Vec<String>,
    pub help_messages: Vec<String>,
}

impl UnResolvedTypesError {
    pub fn from(expr: &Expr, parent_expr: Option<Expr>) -> Self {
        let unresolved_types = UnResolvedTypesError {
            unresolved_expr: expr.clone(),
            additional_messages: Vec::new(),
            parent_expr: parent_expr.clone(),
            help_messages: Vec::new(),
        };

        unresolved_types.with_default_help_messages()
    }

    pub fn with_default_help_messages(&self) -> Self {
        self.with_help_message(
            "consider specifying the type explicitly. Examples: `1: u64`, `person.age: u8`"
        ).with_help_message(
            "or specify the type in let binding. Example: let numbers: list<u8> = [1, 2, 3]"
        )
    }

    pub fn with_parent_expr(&self, expr: &Expr) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error.parent_expr = Some(expr.clone());
        unresolved_error
    }

    pub fn with_additional_error_detail(&self, message: impl AsRef<str>) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error.additional_messages.push(message.as_ref().to_string());
        unresolved_error
    }

    pub fn with_help_message(&self, message: impl AsRef<str>) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error.help_messages.push(message.as_ref().to_string());

        unresolved_error
    }

    pub fn at_field(&self, field_name: String) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error.with_additional_error_detail(format!("unrecognized type at field: `{}`", field_name))
    }

    pub fn at_index(&self, index: usize) -> UnResolvedTypesError {
        let mut unresolved_error: UnResolvedTypesError = self.clone();
        unresolved_error.with_additional_error_detail(format!("unrecognized type at sequence/tuple index: {}", index));
        unresolved_error
    }
}

impl Display for UnResolvedTypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let span = self.unresolved_expr.source_span();

        writeln!(
            f,
            "cannot determine the type of the following rib expression found at line {}, column {}",
            span.start_line(), span.start_column()
        )?;

        writeln!(f, "`{}`", self.unresolved_expr)?;

        if let Some(parent) = &self.parent_expr {
            writeln!(f, "found within:")?;
            writeln!(f, "`{}`", parent)?;
        }

        if !self.additional_messages.is_empty() {
            for message in &self.additional_messages {
                writeln!(f, "{}", message)?;
            }
        }

        Ok(())
    }
}
