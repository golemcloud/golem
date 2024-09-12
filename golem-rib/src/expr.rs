// Copyright 2024 Golem Cloud
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

use crate::call_type::CallType;
use crate::function_name::ParsedFunctionName;
use crate::parser::rib_expr::rib_program;
use crate::parser::type_name::TypeName;
use crate::type_registry::FunctionTypeRegistry;
use crate::{text, type_inference, InferredType, VariableId};
use bincode::{Decode, Encode};
use combine::EasyParser;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::collections::VecDeque;
use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Expr {
    Let(VariableId, Option<TypeName>, Box<Expr>, InferredType),
    SelectField(Box<Expr>, String, InferredType),
    SelectIndex(Box<Expr>, usize, InferredType),
    Sequence(Vec<Expr>, InferredType),
    Record(Vec<(String, Box<Expr>)>, InferredType),
    Tuple(Vec<Expr>, InferredType),
    Literal(String, InferredType),
    Number(Number, Option<TypeName>, InferredType),
    Flags(Vec<String>, InferredType),
    Identifier(VariableId, InferredType),
    Boolean(bool, InferredType),
    Concat(Vec<Expr>, InferredType),
    Multiple(Vec<Expr>, InferredType),
    Not(Box<Expr>, InferredType),
    GreaterThan(Box<Expr>, Box<Expr>, InferredType),
    And(Box<Expr>, Box<Expr>, InferredType),
    GreaterThanOrEqualTo(Box<Expr>, Box<Expr>, InferredType),
    LessThanOrEqualTo(Box<Expr>, Box<Expr>, InferredType),
    EqualTo(Box<Expr>, Box<Expr>, InferredType),
    LessThan(Box<Expr>, Box<Expr>, InferredType),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>, InferredType),
    PatternMatch(Box<Expr>, Vec<MatchArm>, InferredType),
    Option(Option<Box<Expr>>, InferredType),
    Result(Result<Box<Expr>, Box<Expr>>, InferredType),
    Call(CallType, Vec<Expr>, InferredType),
    Unwrap(Box<Expr>, InferredType),
    Throw(String, InferredType),
    Tag(Box<Expr>, InferredType),
    Get(Box<Expr>, usize, InferredType),
}

impl Expr {
    /// Parse a text directly as Rib expression
    /// Example of a Rib expression:
    ///
    /// ```rib
    ///   let result = worker.response;
    ///   let error_message = "invalid response from worker";
    ///
    ///   match result {
    ///     some(record) => record,
    ///     none => "Error: ${error_message}"
    ///   }
    /// ```
    ///
    /// Rib supports conditional calls, function calls, pattern-matching,
    /// string interpolation (see error_message above) etc.
    ///
    pub fn from_text(input: &str) -> Result<Expr, String> {
        rib_program()
            .easy_parse(input.as_ref())
            .map_err(|err| err.to_string())
            .and_then(|(expr, remaining)| {
                if remaining.is_empty() {
                    Ok(expr)
                } else {
                    Err(format!("Failed to parse: {}", remaining))
                }
            })
    }

    /// Parse an interpolated text as Rib expression. The input is always expected to be wrapped with `${..}`
    /// This is mainly to keep the backward compatibility where Golem Cloud console passes a Rib Expression always wrapped in `${..}`
    ///
    /// Explanation:
    /// Usually `Expr::from_text` is all that you need which takes a plain text and try to parse it as an Expr.
    /// `from_interpolated_str` can be used when you want to be strict - only if text is wrapped in `${..}`, it should
    /// be considered as a Rib expression.
    ///
    /// Example 1:
    ///
    /// ```rib
    ///   ${
    ///     let result = worker.response;
    ///     let error_message = "invalid response from worker";
    ///
    ///     match result {
    ///       some(record) => record,
    ///       none => "Error: ${error_message}"
    ///     }
    ///   }
    /// ```
    /// You can see the entire text is wrapped in `${..}` to specify that it's containing
    /// a Rib expression and anything outside is considered as a literal string.
    ///
    /// The advantage of using `from_interpolated_str` is Rib the behaviour is consistent that only those texts
    //  within `${..}` are considered as Rib expressions all the time.
    ///
    /// Example 2:
    ///
    /// ```rib
    ///  worker-id-${request.user_id}
    /// ```
    /// ```rib
    ///   ${"worker-id-${request.user_id}"}
    /// ```
    /// ```rib
    ///   ${request.user_id}
    /// ```
    /// ```rib
    ///   foo-${"worker-id-${request.user_id}"}
    /// ```
    /// etc.
    ///
    /// The first one will be parsed as `Expr::Concat(Expr::Literal("worker-id-"), Expr::SelectField(Expr::Identifier("request"), "user_id"))`.
    ///
    /// The following will work too.
    /// In the below example, the entire if condition is a Rib expression  (because it is wrapped in ${..}) and
    /// the else condition is resolved to  a literal where part of it is a Rib expression itself (user.id).
    ///
    /// ```rib
    ///   ${if foo > 1 then bar else "baz-${user.id}"}
    /// ```
    /// If you need the following to be considered as Rib program (without interpolation), use `Expr::from_text` instead.
    ///
    /// ```rib
    ///   if foo > 1 then bar else "baz-${user.id}"
    /// ```
    ///
    pub fn from_interpolated_str(input: &str) -> Result<Expr, String> {
        let input = format!("\"{}\"", input);
        Self::from_text(input.as_str())
    }

    pub fn is_literal(&self) -> bool {
        matches!(self, Expr::Literal(_, _))
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Expr::Number(_, _, _))
    }

    pub fn is_record(&self) -> bool {
        matches!(self, Expr::Record(_, _))
    }

    pub fn is_result(&self) -> bool {
        matches!(self, Expr::Result(_, _))
    }

    pub fn is_option(&self) -> bool {
        matches!(self, Expr::Option(_, _))
    }

    pub fn is_tuple(&self) -> bool {
        matches!(self, Expr::Tuple(_, _))
    }

    pub fn is_list(&self) -> bool {
        matches!(self, Expr::Sequence(_, _))
    }

    pub fn is_flags(&self) -> bool {
        matches!(self, Expr::Flags(_, _))
    }

    pub fn is_identifier(&self) -> bool {
        matches!(self, Expr::Identifier(_, _))
    }

    pub fn is_select_field(&self) -> bool {
        matches!(self, Expr::SelectField(_, _, _))
    }

    pub fn is_if_else(&self) -> bool {
        matches!(self, Expr::Cond(_, _, _, _))
    }

    pub fn is_match_expr(&self) -> bool {
        matches!(self, Expr::PatternMatch(_, _, _))
    }

    pub fn is_select_index(&self) -> bool {
        matches!(self, Expr::SelectIndex(_, _, _))
    }

    pub fn is_boolean(&self) -> bool {
        matches!(self, Expr::Boolean(_, _))
    }

    pub fn is_comparison(&self) -> bool {
        matches!(
            self,
            Expr::GreaterThan(_, _, _)
                | Expr::GreaterThanOrEqualTo(_, _, _)
                | Expr::LessThanOrEqualTo(_, _, _)
                | Expr::EqualTo(_, _, _)
                | Expr::LessThan(_, _, _)
        )
    }

    pub fn is_concat(&self) -> bool {
        matches!(self, Expr::Concat(_, _))
    }

    pub fn is_multiple(&self) -> bool {
        matches!(self, Expr::Multiple(_, _))
    }

    pub fn inbuilt_variant(&self) -> Option<(String, Option<Expr>)> {
        match self {
            Expr::Option(Some(expr), _) => Some(("some".to_string(), Some(expr.deref().clone()))),
            Expr::Option(None, _) => Some(("some".to_string(), None)),
            Expr::Result(Ok(expr), _) => Some(("ok".to_string(), Some(expr.deref().clone()))),
            Expr::Result(Err(expr), _) => Some(("err".to_string(), Some(expr.deref().clone()))),
            _ => None,
        }
    }
    pub fn unwrap(&self) -> Self {
        Expr::Unwrap(Box::new(self.clone()), InferredType::Unknown)
    }

    pub fn get(&self, index: usize) -> Self {
        Expr::Get(Box::new(self.clone()), index, InferredType::Unknown)
    }
    pub fn boolean(value: bool) -> Self {
        Expr::Boolean(value, InferredType::Bool)
    }

    pub fn call(parsed_fn_name: ParsedFunctionName, args: Vec<Expr>) -> Self {
        Expr::Call(
            CallType::Function(parsed_fn_name),
            args,
            InferredType::Unknown,
        )
    }

    pub fn concat(expressions: Vec<Expr>) -> Self {
        Expr::Concat(expressions, InferredType::Str)
    }

    pub fn cond(cond: Expr, then: Expr, else_: Expr) -> Self {
        Expr::Cond(
            Box::new(cond),
            Box::new(then),
            Box::new(else_),
            InferredType::Unknown,
        )
    }

    pub fn equal_to(left: Expr, right: Expr) -> Self {
        Expr::EqualTo(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn err(expr: Expr) -> Self {
        let inferred_type = expr.inferred_type();
        Expr::Result(
            Err(Box::new(expr)),
            InferredType::Result {
                ok: Some(Box::new(InferredType::Unknown)),
                error: Some(Box::new(inferred_type)),
            },
        )
    }

    pub fn flags(flags: Vec<String>) -> Self {
        Expr::Flags(flags.clone(), InferredType::Flags(flags))
    }

    pub fn greater_than(left: Expr, right: Expr) -> Self {
        Expr::GreaterThan(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn greater_than_or_equal_to(left: Expr, right: Expr) -> Self {
        Expr::GreaterThanOrEqualTo(Box::new(left), Box::new(right), InferredType::Bool)
    }

    // An identifier by default is global until name-binding phase is run
    pub fn identifier(name: impl AsRef<str>) -> Self {
        Expr::Identifier(
            VariableId::global(name.as_ref().to_string()),
            InferredType::Unknown,
        )
    }

    pub fn less_than(left: Expr, right: Expr) -> Self {
        Expr::LessThan(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn less_than_or_equal_to(left: Expr, right: Expr) -> Self {
        Expr::LessThanOrEqualTo(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn let_binding(name: impl AsRef<str>, expr: Expr) -> Self {
        Expr::Let(
            VariableId::global(name.as_ref().to_string()),
            None,
            Box::new(expr),
            InferredType::Unknown,
        )
    }

    pub fn let_binding_with_type(name: impl AsRef<str>, type_name: TypeName, expr: Expr) -> Self {
        Expr::Let(
            VariableId::global(name.as_ref().to_string()),
            Some(type_name),
            Box::new(expr),
            InferredType::Unknown,
        )
    }

    pub fn literal(value: impl AsRef<str>) -> Self {
        Expr::Literal(value.as_ref().to_string(), InferredType::Str)
    }

    pub fn multiple(expressions: Vec<Expr>) -> Self {
        let inferred_type = expressions
            .last()
            .map_or(InferredType::Unknown, |e| e.inferred_type());

        Expr::Multiple(expressions, inferred_type)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn not(expr: Expr) -> Self {
        Expr::Not(Box::new(expr), InferredType::Bool)
    }

    pub fn ok(expr: Expr) -> Self {
        let inferred_type = expr.inferred_type();
        Expr::Result(
            Ok(Box::new(expr)),
            InferredType::Result {
                ok: Some(Box::new(inferred_type)),
                error: Some(Box::new(InferredType::Unknown)),
            },
        )
    }

    pub fn option(expr: Option<Expr>) -> Self {
        let inferred_type = match &expr {
            Some(expr) => expr.inferred_type(),
            None => InferredType::Unknown,
        };

        Expr::Option(
            expr.map(Box::new),
            InferredType::Option(Box::new(inferred_type)),
        )
    }

    pub fn pattern_match(expr: Expr, match_arms: Vec<MatchArm>) -> Self {
        Expr::PatternMatch(Box::new(expr), match_arms, InferredType::Unknown)
    }

    pub fn record(expressions: Vec<(String, Expr)>) -> Self {
        let inferred_type = InferredType::Record(
            expressions
                .iter()
                .map(|(field_name, expr)| (field_name.to_string(), expr.inferred_type()))
                .collect(),
        );

        Expr::Record(
            expressions
                .into_iter()
                .map(|(field_name, expr)| (field_name, Box::new(expr)))
                .collect(),
            inferred_type,
        )
    }

    pub fn select_field(expr: Expr, field: impl AsRef<str>) -> Self {
        Expr::SelectField(
            Box::new(expr),
            field.as_ref().to_string(),
            InferredType::Unknown,
        )
    }

    pub fn select_index(expr: Expr, index: usize) -> Self {
        Expr::SelectIndex(Box::new(expr), index, InferredType::Unknown)
    }

    pub fn tag(expr: Expr) -> Self {
        Expr::Tag(Box::new(expr), InferredType::Unknown)
    }

    pub fn tuple(expressions: Vec<Expr>) -> Self {
        let inferred_type = InferredType::Tuple(
            expressions
                .iter()
                .map(|expr| expr.inferred_type())
                .collect(),
        );

        Expr::Tuple(expressions, inferred_type)
    }

    pub fn sequence(expressions: Vec<Expr>) -> Self {
        let inferred_type = InferredType::List(Box::new(
            expressions
                .first()
                .map_or(InferredType::Unknown, |x| x.inferred_type()),
        ));

        Expr::Sequence(expressions, inferred_type)
    }

    pub fn inferred_type(&self) -> InferredType {
        match self {
            Expr::Let(_, _, _, inferred_type)
            | Expr::SelectField(_, _, inferred_type)
            | Expr::SelectIndex(_, _, inferred_type)
            | Expr::Sequence(_, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, _, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Identifier(_, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::Multiple(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, inferred_type)
            | Expr::Result(_, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::Tag(_, inferred_type)
            | Expr::Get(_, _, inferred_type)
            | Expr::And(_, _, inferred_type)
            | Expr::Call(_, _, inferred_type) => inferred_type.clone(),
        }
    }

    pub fn infer_types(
        &mut self,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<(), Vec<String>> {
        self.bind_types();
        self.name_binding_pattern_match_variables();
        self.name_binding_local_variables();
        self.infer_function_types(function_type_registry)
            .map_err(|x| vec![x])?;
        self.infer_variants(function_type_registry);
        self.infer_enums(function_type_registry);
        type_inference::type_inference_fix_point(Self::inference_scan, self)
            .map_err(|x| vec![x])?;
        self.unify_types()?;
        Ok(())
    }

    // An inference scan is a single cycle of to-and-fro scanning of Rib expression
    // to infer the types
    pub fn inference_scan(&mut self) -> Result<(), String> {
        self.infer_all_identifiers()?;
        self.push_types_down()?;
        self.infer_all_identifiers()?;
        self.pull_types_up()?;
        self.unify_types().unwrap_or(());
        self.infer_global_inputs();

        Ok(())
    }

    pub fn name_binding_pattern_match_variables(&mut self) {
        type_inference::name_binding_pattern_matches(self);
    }
    // We make sure the let bindings name are properly
    // bound to the named identifiers.
    pub fn name_binding_local_variables(&mut self) {
        type_inference::name_binding_local_variables(self);
    }

    // At this point we simply update the types to the parameter type expressions and the call expression itself.
    pub fn infer_function_types(
        &mut self,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<(), String> {
        type_inference::infer_function_types(self, function_type_registry)
    }

    pub fn push_types_down(&mut self) -> Result<(), String> {
        type_inference::push_types_down(self)
    }

    pub fn infer_all_identifiers(&mut self) -> Result<(), String> {
        type_inference::infer_all_identifiers(self)
    }

    pub fn pull_types_up(&mut self) -> Result<(), String> {
        type_inference::pull_types_up(self)
    }

    pub fn infer_global_inputs(&mut self) {
        type_inference::infer_global_inputs(self);
    }

    pub fn bind_types(&mut self) {
        type_inference::bind_type(self);
    }

    pub fn type_check(&self) -> Result<(), Vec<String>> {
        type_inference::type_check(self)
    }

    pub fn unify_types(&mut self) -> Result<(), Vec<String>> {
        type_inference::unify_types(self)
    }

    pub fn add_infer_type(&self, new_inferred_type: InferredType) -> Expr {
        let mut expr_copied = self.clone();
        expr_copied.add_infer_type_mut(new_inferred_type);
        expr_copied
    }

    pub fn add_infer_type_mut(&mut self, new_inferred_type: InferredType) {
        match self {
            Expr::Identifier(_, inferred_type)
            | Expr::Let(_, _, _, inferred_type)
            | Expr::SelectField(_, _, inferred_type)
            | Expr::SelectIndex(_, _, inferred_type)
            | Expr::Sequence(_, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, _, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::Multiple(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, inferred_type)
            | Expr::Result(_, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::Tag(_, inferred_type)
            | Expr::Get(_, _, inferred_type)
            | Expr::And(_, _, inferred_type)
            | Expr::Call(_, _, inferred_type) => {
                if new_inferred_type != InferredType::Unknown {
                    *inferred_type = inferred_type.merge(new_inferred_type);
                }
            }
        }
    }

    pub fn reset_type(&mut self) {
        type_inference::reset_type_info(self);
    }

    pub fn override_type_type_mut(&mut self, new_inferred_type: InferredType) {
        match self {
            Expr::Identifier(_, inferred_type)
            | Expr::Let(_, _, _, inferred_type)
            | Expr::SelectField(_, _, inferred_type)
            | Expr::SelectIndex(_, _, inferred_type)
            | Expr::Sequence(_, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, _, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::Multiple(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, inferred_type)
            | Expr::Result(_, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::Get(_, _, inferred_type)
            | Expr::And(_, _, inferred_type)
            | Expr::Tag(_, inferred_type)
            | Expr::Call(_, _, inferred_type) => {
                if new_inferred_type != InferredType::Unknown {
                    *inferred_type = new_inferred_type;
                }
            }
        }
    }

    pub fn infer_enums(&mut self, function_type_registry: &FunctionTypeRegistry) {
        type_inference::infer_enums(self, function_type_registry);
    }

    pub fn infer_variants(&mut self, function_type_registry: &FunctionTypeRegistry) {
        type_inference::infer_variants(self, function_type_registry);
    }

    pub fn visit_children_bottom_up<'a>(&'a self, queue: &mut VecDeque<&'a Expr>) {
        type_inference::visit_children_bottom_up(self, queue);
    }

    pub fn visit_children_mut_top_down<'a>(&'a mut self, queue: &mut VecDeque<&'a mut Expr>) {
        type_inference::visit_children_mut_top_down(self, queue);
    }

    pub fn visit_children_mut_bottom_up<'a>(&'a mut self, queue: &mut VecDeque<&'a mut Expr>) {
        type_inference::visit_children_bottom_up_mut(self, queue);
    }

    pub fn number(f64: f64) -> Expr {
        Expr::Number(
            Number { value: f64 },
            None,
            InferredType::OneOf(vec![
                InferredType::U64,
                InferredType::U32,
                InferredType::U8,
                InferredType::U16,
                InferredType::S64,
                InferredType::S32,
                InferredType::S8,
                InferredType::S16,
                InferredType::F64,
                InferredType::F32,
            ]),
        )
    }

    // TODO; introduced to minimise the number of changes in tests.
    pub fn number_with_type_name(f64: f64, type_name: TypeName) -> Expr {
        Expr::Number(
            Number { value: f64 },
            Some(type_name),
            InferredType::OneOf(vec![
                InferredType::U64,
                InferredType::U32,
                InferredType::U8,
                InferredType::U16,
                InferredType::S64,
                InferredType::S32,
                InferredType::S8,
                InferredType::S16,
                InferredType::F64,
                InferredType::F32,
            ]),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct Number {
    pub value: f64, // Change to bigdecimal
}

impl Number {
    pub fn to_val(&self, analysed_type: &AnalysedType) -> Option<TypeAnnotatedValue> {
        match analysed_type {
            AnalysedType::F64(_) => Some(TypeAnnotatedValue::F64(self.value)),
            AnalysedType::U64(_) => Some(TypeAnnotatedValue::U64(self.value as u64)),
            AnalysedType::F32(_) => Some(TypeAnnotatedValue::F32(self.value as f32)),
            AnalysedType::U32(_) => Some(TypeAnnotatedValue::U32(self.value as u32)),
            AnalysedType::S32(_) => Some(TypeAnnotatedValue::S32(self.value as i32)),
            AnalysedType::S64(_) => Some(TypeAnnotatedValue::S64(self.value as i64)),
            AnalysedType::U8(_) => Some(TypeAnnotatedValue::U8(self.value as u32)),
            AnalysedType::S8(_) => Some(TypeAnnotatedValue::S8(self.value as i32)),
            AnalysedType::U16(_) => Some(TypeAnnotatedValue::U16(self.value as u32)),
            AnalysedType::S16(_) => Some(TypeAnnotatedValue::S16(self.value as i32)),
            _ => None,
        }
    }
}

impl Display for Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct MatchArm {
    pub arm_pattern: ArmPattern,
    pub arm_resolution_expr: Box<Expr>,
}

impl MatchArm {
    pub fn new(arm_pattern: ArmPattern, arm_resolution: Expr) -> MatchArm {
        MatchArm {
            arm_pattern,
            arm_resolution_expr: Box::new(arm_resolution),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ArmPattern {
    WildCard,
    As(String, Box<ArmPattern>),
    Constructor(String, Vec<ArmPattern>),
    TupleConstructor(Vec<ArmPattern>), // Just because tuple doesn't have a name to fall into Constructor
    Literal(Box<Expr>),
}

impl ArmPattern {
    pub fn get_expr_literals_mut(&mut self) -> Vec<&mut Box<Expr>> {
        match self {
            ArmPattern::Literal(expr) => vec![expr],
            ArmPattern::As(_, pattern) => pattern.get_expr_literals_mut(),
            ArmPattern::Constructor(_, patterns) => {
                let mut result = vec![];
                for pattern in patterns {
                    result.extend(pattern.get_expr_literals_mut());
                }
                result
            }
            ArmPattern::TupleConstructor(patterns) => {
                let mut result = vec![];
                for pattern in patterns {
                    result.extend(pattern.get_expr_literals_mut());
                }
                result
            }
            ArmPattern::WildCard => vec![],
        }
    }

    pub fn get_expr_literals(&self) -> Vec<&Expr> {
        match self {
            ArmPattern::Literal(expr) => vec![expr.as_ref()],
            ArmPattern::As(_, pattern) => pattern.get_expr_literals(),
            ArmPattern::Constructor(_, patterns) => {
                let mut result = vec![];
                for pattern in patterns {
                    result.extend(pattern.get_expr_literals());
                }
                result
            }
            ArmPattern::TupleConstructor(patterns) => {
                let mut result = vec![];
                for pattern in patterns {
                    result.extend(pattern.get_expr_literals());
                }
                result
            }
            ArmPattern::WildCard => vec![],
        }
    }
    // Helper to construct ok(v). Cannot be used if there is nested constructors such as ok(some(v)))
    pub fn ok(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Result(
            Ok(Box::new(Expr::Identifier(
                VariableId::global(binding_variable.to_string()),
                InferredType::Unknown,
            ))),
            InferredType::Result {
                ok: Some(Box::new(InferredType::Unknown)),
                error: Some(Box::new(InferredType::Unknown)),
            },
        )))
    }

    // Helper to construct err(v). Cannot be used if there is nested constructors such as err(some(v)))
    pub fn err(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Result(
            Err(Box::new(Expr::Identifier(
                VariableId::global(binding_variable.to_string()),
                InferredType::Unknown,
            ))),
            InferredType::Result {
                ok: Some(Box::new(InferredType::Unknown)),
                error: Some(Box::new(InferredType::Unknown)),
            },
        )))
    }

    // Helper to construct some(v). Cannot be used if there is nested constructors such as some(ok(v)))
    pub fn some(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option(
            Some(Box::new(Expr::Identifier(
                VariableId::local_with_no_id(binding_variable),
                InferredType::Unknown,
            ))),
            InferredType::Unknown,
        )))
    }

    pub fn none() -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option(None, InferredType::Unknown)))
    }

    pub fn identifier(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Identifier(
            VariableId::global(binding_variable.to_string()),
            InferredType::Unknown,
        )))
    }
    pub fn custom_constructor(name: &str, args: Vec<ArmPattern>) -> ArmPattern {
        ArmPattern::Constructor(name.to_string(), args)
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::Expr> for Expr {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::Expr) -> Result<Self, Self::Error> {
        let expr = value.expr.ok_or("Missing expr")?;

        let expr = match expr {
            golem_api_grpc::proto::golem::rib::expr::Expr::Let(expr) => {
                let name = expr.name;
                let type_name = expr.type_name.map(TypeName::try_from).transpose()?;
                let expr_: golem_api_grpc::proto::golem::rib::Expr =
                    *expr.expr.ok_or("Missing expr")?;
                let expr: Expr = expr_.try_into()?;
                if let Some(type_name) = type_name {
                    Expr::let_binding_with_type(name, type_name, expr)
                } else {
                    Expr::let_binding(name, expr)
                }
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Not(expr) => {
                let expr = expr.expr.ok_or("Missing expr")?;
                Expr::not((*expr).try_into()?)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThan(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::greater_than((*left).try_into()?, (*right).try_into()?)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThanOrEqual(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::greater_than_or_equal_to((*left).try_into()?, (*right).try_into()?)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::LessThan(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::less_than((*left).try_into()?, (*right).try_into()?)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::LessThanOrEqual(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::less_than_or_equal_to((*left).try_into()?, (*right).try_into()?)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::EqualTo(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::equal_to((*left).try_into()?, (*right).try_into()?)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Cond(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let cond = expr.cond.ok_or("Missing cond expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::cond(
                    (*left).try_into()?,
                    (*cond).try_into()?,
                    (*right).try_into()?,
                )
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Concat(
                golem_api_grpc::proto::golem::rib::ConcatExpr { exprs },
            ) => {
                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::concat(exprs)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Multiple(
                golem_api_grpc::proto::golem::rib::MultipleExpr { exprs },
            ) => {
                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::multiple(exprs)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
                golem_api_grpc::proto::golem::rib::SequenceExpr { exprs },
            ) => {
                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::sequence(exprs)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Tuple(
                golem_api_grpc::proto::golem::rib::TupleExpr { exprs },
            ) => {
                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::tuple(exprs)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Record(
                golem_api_grpc::proto::golem::rib::RecordExpr { fields },
            ) => {
                let mut values: Vec<(String, Expr)> = vec![];
                for record in fields.into_iter() {
                    let name = record.name;
                    let expr = record.expr.ok_or("Missing expr")?;
                    values.push((name, expr.try_into()?));
                }
                Expr::record(values)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Flags(
                golem_api_grpc::proto::golem::rib::FlagsExpr { values },
            ) => Expr::flags(values),
            golem_api_grpc::proto::golem::rib::expr::Expr::Literal(
                golem_api_grpc::proto::golem::rib::LiteralExpr { value },
            ) => Expr::literal(value),
            golem_api_grpc::proto::golem::rib::expr::Expr::Identifier(
                golem_api_grpc::proto::golem::rib::IdentifierExpr { name },
            ) => Expr::identifier(name.as_str()),
            golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
                golem_api_grpc::proto::golem::rib::BooleanExpr { value },
            ) => Expr::boolean(value),
            golem_api_grpc::proto::golem::rib::expr::Expr::Number(number) => {
                let type_name = number.type_name.map(TypeName::try_from).transpose()?;
                if let Some(type_name) = type_name {
                    Expr::number_with_type_name(number.float, type_name.clone())
                } else {
                    Expr::number(number.float)
                }
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(expr) => {
                let expr = *expr;
                let field = expr.field;
                let expr = *expr.expr.ok_or(
                    "Mi\
                ssing expr",
                )?;
                Expr::select_field(expr.try_into()?, field.as_str())
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndex(expr) => {
                let expr = *expr;
                let index = expr.index as usize;
                let expr = *expr.expr.ok_or("Missing expr")?;
                Expr::select_index(expr.try_into()?, index)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Option(expr) => match expr.expr {
                Some(expr) => Expr::option(Some((*expr).try_into()?)),
                None => Expr::option(None),
            },
            golem_api_grpc::proto::golem::rib::expr::Expr::Result(expr) => {
                let result = expr.result.ok_or("Missing result")?;
                match result {
                    golem_api_grpc::proto::golem::rib::result_expr::Result::Ok(expr) => {
                        Expr::ok((*expr).try_into()?)
                    }
                    golem_api_grpc::proto::golem::rib::result_expr::Result::Err(expr) => {
                        Expr::err((*expr).try_into()?)
                    }
                }
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::PatternMatch(expr) => {
                let patterns: Vec<MatchArm> = expr
                    .patterns
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                let expr = expr.expr.ok_or("Missing expr")?;
                Expr::pattern_match((*expr).try_into()?, patterns)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Call(expr) => {
                let params: Vec<Expr> = expr
                    .params
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                let invocation_name = expr.name.ok_or("Missing invocation name")?;
                let name = invocation_name.name.ok_or("Missing function call name")?;
                match name {
                    golem_api_grpc::proto::golem::rib::invocation_name::Name::Parsed(name) => {
                        Expr::call(name.try_into()?, params)
                    }
                    golem_api_grpc::proto::golem::rib::invocation_name::Name::VariantConstructor(
                        name,
                    ) => Expr::call(ParsedFunctionName::parse(name)?, params),
                    golem_api_grpc::proto::golem::rib::invocation_name::Name::EnumConstructor(
                        name,
                    ) => Expr::call(ParsedFunctionName::parse(name)?, params),
                }
            }
        };
        Ok(expr)
    }
}

impl From<Expr> for golem_api_grpc::proto::golem::rib::Expr {
    fn from(value: Expr) -> Self {
        let expr = match value {
            Expr::Let(variable_id, type_name, expr, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Let(
                    Box::new(golem_api_grpc::proto::golem::rib::LetExpr {
                        name: variable_id.name().to_string(),
                        expr: Some(Box::new((*expr).into())),
                        type_name: type_name.map(|t| t.into()),
                    }),
                ))
            }
            Expr::SelectField(expr, field, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(
                    Box::new(golem_api_grpc::proto::golem::rib::SelectFieldExpr {
                        expr: Some(Box::new((*expr).into())),
                        field,
                    }),
                ))
            }
            Expr::SelectIndex(expr, index, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndex(
                    Box::new(golem_api_grpc::proto::golem::rib::SelectIndexExpr {
                        expr: Some(Box::new((*expr).into())),
                        index: index as u64,
                    }),
                ))
            }
            Expr::Sequence(exprs, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
                    golem_api_grpc::proto::golem::rib::SequenceExpr {
                        exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                    },
                ))
            }
            Expr::Record(fields, _) => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Record(
                golem_api_grpc::proto::golem::rib::RecordExpr {
                    fields: fields
                        .into_iter()
                        .map(
                            |(name, expr)| golem_api_grpc::proto::golem::rib::RecordFieldExpr {
                                name,
                                expr: Some((*expr).into()),
                            },
                        )
                        .collect(),
                },
            )),
            Expr::Tuple(exprs, _) => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Tuple(
                golem_api_grpc::proto::golem::rib::TupleExpr {
                    exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                },
            )),
            Expr::Literal(value, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Literal(
                    golem_api_grpc::proto::golem::rib::LiteralExpr { value },
                ))
            }
            Expr::Number(number, type_name, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Number(
                    golem_api_grpc::proto::golem::rib::NumberExpr {
                        float: number.value,
                        type_name: type_name.map(|t| t.into()),
                    },
                ))
            }
            Expr::Flags(values, _) => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Flags(
                golem_api_grpc::proto::golem::rib::FlagsExpr { values },
            )),
            Expr::Identifier(variable_id, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Identifier(
                    golem_api_grpc::proto::golem::rib::IdentifierExpr {
                        name: variable_id.name(),
                    },
                ))
            }
            Expr::Boolean(value, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
                    golem_api_grpc::proto::golem::rib::BooleanExpr { value },
                ))
            }
            Expr::Concat(exprs, _) => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Concat(
                golem_api_grpc::proto::golem::rib::ConcatExpr {
                    exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                },
            )),
            Expr::Multiple(exprs, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Multiple(
                    golem_api_grpc::proto::golem::rib::MultipleExpr {
                        exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                    },
                ))
            }
            Expr::Not(expr, _) => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Not(
                Box::new(golem_api_grpc::proto::golem::rib::NotExpr {
                    expr: Some(Box::new((*expr).into())),
                }),
            )),
            Expr::GreaterThan(left, right, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThan(
                    Box::new(golem_api_grpc::proto::golem::rib::GreaterThanExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    }),
                ))
            }
            Expr::GreaterThanOrEqualTo(left, right, _) => Some(
                golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThanOrEqual(Box::new(
                    golem_api_grpc::proto::golem::rib::GreaterThanOrEqualToExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                )),
            ),
            Expr::LessThan(left, right, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::LessThan(
                    Box::new(golem_api_grpc::proto::golem::rib::LessThanExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    }),
                ))
            }
            Expr::LessThanOrEqualTo(left, right, _) => Some(
                golem_api_grpc::proto::golem::rib::expr::Expr::LessThanOrEqual(Box::new(
                    golem_api_grpc::proto::golem::rib::LessThanOrEqualToExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                )),
            ),
            Expr::EqualTo(left, right, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::EqualTo(
                    Box::new(golem_api_grpc::proto::golem::rib::EqualToExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    }),
                ))
            }
            Expr::Cond(left, cond, right, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Cond(
                    Box::new(golem_api_grpc::proto::golem::rib::CondExpr {
                        left: Some(Box::new((*left).into())),
                        cond: Some(Box::new((*cond).into())),
                        right: Some(Box::new((*right).into())),
                    }),
                ))
            }
            Expr::PatternMatch(expr, arms, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::PatternMatch(
                    Box::new(golem_api_grpc::proto::golem::rib::PatternMatchExpr {
                        expr: Some(Box::new((*expr).into())),
                        patterns: arms.into_iter().map(|a| a.into()).collect(),
                    }),
                ))
            }
            Expr::Option(expr, _) => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Option(
                Box::new(golem_api_grpc::proto::golem::rib::OptionExpr {
                    expr: expr.map(|expr| Box::new((*expr).into())),
                }),
            )),
            Expr::Result(expr, _) => {
                let result = match expr {
                    Ok(expr) => golem_api_grpc::proto::golem::rib::result_expr::Result::Ok(
                        Box::new((*expr).into()),
                    ),
                    Err(expr) => golem_api_grpc::proto::golem::rib::result_expr::Result::Err(
                        Box::new((*expr).into()),
                    ),
                };

                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Result(
                    Box::new(golem_api_grpc::proto::golem::rib::ResultExpr {
                        result: Some(result),
                    }),
                ))
            }
            Expr::Call(function_name, args, _) => {
                Some(golem_api_grpc::proto::golem::rib::expr::Expr::Call(
                    golem_api_grpc::proto::golem::rib::CallExpr {
                        name: Some(function_name.into()),
                        params: args.into_iter().map(|expr| expr.into()).collect(),
                    },
                ))
            }
            // Not yet supported as a syntax, so shouldn't be called
            Expr::Unwrap(expr, _) => Self::from(*expr).expr,
            // Not yet supported as a syntax, so shouldn't be called
            Expr::Throw(msg, _) => Self::from(Expr::literal(msg)).expr,
            Expr::Tag(expr, _) => Self::from(*expr).expr,
            Expr::Get(expr, _, _) => Self::from(*expr).expr,
            Expr::And(left, right, _) => Self::from(*left).expr,
        };

        golem_api_grpc::proto::golem::rib::Expr { expr }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::ArmPattern> for ArmPattern {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::ArmPattern) -> Result<Self, Self::Error> {
        let pattern = value.pattern.ok_or("Missing pattern")?;
        match pattern {
            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::WildCard(_) => {
                Ok(ArmPattern::WildCard)
            }
            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::As(asp) => {
                let name = asp.name;
                let pattern = asp.pattern.ok_or("Missing pattern")?;
                Ok(ArmPattern::As(name, Box::new((*pattern).try_into()?)))
            }
            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Constructor(
                golem_api_grpc::proto::golem::rib::ConstructorArmPattern { name, patterns },
            ) => {
                let patterns = patterns
                    .into_iter()
                    .map(ArmPattern::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ArmPattern::Constructor(name, patterns))
            }
            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Literal(
                golem_api_grpc::proto::golem::rib::LiteralArmPattern { expr },
            ) => {
                let inner = expr.ok_or("Missing expr")?;
                Ok(ArmPattern::Literal(Box::new(inner.try_into()?)))
            }
        }
    }
}

impl From<ArmPattern> for golem_api_grpc::proto::golem::rib::ArmPattern {
    fn from(value: ArmPattern) -> Self {
        match value {
            ArmPattern::WildCard => golem_api_grpc::proto::golem::rib::ArmPattern {
                pattern: Some(
                    golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::WildCard(
                        golem_api_grpc::proto::golem::rib::WildCardArmPattern {},
                    ),
                ),
            },
            ArmPattern::As(name, pattern) => golem_api_grpc::proto::golem::rib::ArmPattern {
                pattern: Some(golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::As(
                    Box::new(golem_api_grpc::proto::golem::rib::AsArmPattern {
                        name,
                        pattern: Some(Box::new((*pattern).into())),
                    }),
                )),
            },
            ArmPattern::Constructor(name, patterns) => {
                golem_api_grpc::proto::golem::rib::ArmPattern {
                    pattern: Some(
                        golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Constructor(
                            golem_api_grpc::proto::golem::rib::ConstructorArmPattern {
                                name,
                                patterns: patterns
                                    .into_iter()
                                    .map(golem_api_grpc::proto::golem::rib::ArmPattern::from)
                                    .collect(),
                            },
                        ),
                    ),
                }
            }
            ArmPattern::Literal(expr) => golem_api_grpc::proto::golem::rib::ArmPattern {
                pattern: Some(
                    golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Literal(
                        golem_api_grpc::proto::golem::rib::LiteralArmPattern {
                            expr: Some((*expr).into()),
                        },
                    ),
                ),
            },

            ArmPattern::TupleConstructor(patterns) => todo!("TupleConstructor"),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::MatchArm> for MatchArm {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::MatchArm) -> Result<Self, Self::Error> {
        let pattern = value.pattern.ok_or("Missing pattern")?;
        let expr = value.expr.ok_or("Missing expr")?;
        Ok(MatchArm::new(pattern.try_into()?, expr.try_into()?))
    }
}

impl From<MatchArm> for golem_api_grpc::proto::golem::rib::MatchArm {
    fn from(value: MatchArm) -> Self {
        let MatchArm {
            arm_pattern,
            arm_resolution_expr,
        } = value;
        golem_api_grpc::proto::golem::rib::MatchArm {
            pattern: Some(arm_pattern.into()),
            expr: Some((*arm_resolution_expr).into()),
        }
    }
}

impl Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", text::to_string(self).unwrap())
    }
}

impl FromStr for Expr {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Expr::from_interpolated_str(s).map_err(|err| err.to_string())
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            Value::String(expr_string) => match text::from_string(expr_string.as_str()) {
                Ok(expr) => Ok(expr),
                Err(message) => Err(serde::de::Error::custom(message.to_string())),
            },

            e => Err(serde::de::Error::custom(format!(
                "Failed to deserialise expression {}",
                e
            ))),
        }
    }
}

impl Serialize for Expr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match text::to_string(self) {
            Ok(value) => serde_json::Value::serialize(&Value::String(value), serializer),
            Err(error) => Err(serde::ser::Error::custom(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {}
