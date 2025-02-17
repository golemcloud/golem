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

use crate::call_type::CallType;
use crate::generic_type_parameter::GenericTypeParameter;
use crate::parser::block::block;
use crate::parser::type_name::TypeName;
use crate::type_registry::FunctionTypeRegistry;
use crate::{
    from_string, text, type_checker, type_inference, DynamicParsedFunctionName,
    GlobalVariableTypeSpec, InferredType, ParsedFunctionName, VariableId,
};
use bigdecimal::{BigDecimal, FromPrimitive, ToPrimitive};
use combine::parser::char::spaces;
use combine::stream::position;
use combine::Parser;
use combine::{eof, EasyParser};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{IntoValueAndType, ValueAndType};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::collections::VecDeque;
use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

#[derive(Debug, Hash, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Expr {
    Let(VariableId, Option<TypeName>, Box<Expr>, InferredType),
    SelectField(Box<Expr>, String, Option<TypeName>, InferredType),
    SelectIndex(Box<Expr>, usize, Option<TypeName>, InferredType),
    Sequence(Vec<Expr>, Option<TypeName>, InferredType),
    Record(Vec<(String, Box<Expr>)>, InferredType),
    Tuple(Vec<Expr>, InferredType),
    Literal(String, InferredType),
    Number(Number, Option<TypeName>, InferredType),
    Flags(Vec<String>, InferredType),
    Identifier(VariableId, Option<TypeName>, InferredType),
    Boolean(bool, InferredType),
    Concat(Vec<Expr>, InferredType),
    ExprBlock(Vec<Expr>, InferredType),
    Not(Box<Expr>, InferredType),
    GreaterThan(Box<Expr>, Box<Expr>, InferredType),
    And(Box<Expr>, Box<Expr>, InferredType),
    Or(Box<Expr>, Box<Expr>, InferredType),
    GreaterThanOrEqualTo(Box<Expr>, Box<Expr>, InferredType),
    LessThanOrEqualTo(Box<Expr>, Box<Expr>, InferredType),
    Plus(Box<Expr>, Box<Expr>, InferredType),
    Multiply(Box<Expr>, Box<Expr>, InferredType),
    Minus(Box<Expr>, Box<Expr>, InferredType),
    Divide(Box<Expr>, Box<Expr>, InferredType),
    EqualTo(Box<Expr>, Box<Expr>, InferredType),
    LessThan(Box<Expr>, Box<Expr>, InferredType),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>, InferredType),
    PatternMatch(Box<Expr>, Vec<MatchArm>, InferredType),
    Option(Option<Box<Expr>>, Option<TypeName>, InferredType),
    Result(Result<Box<Expr>, Box<Expr>>, Option<TypeName>, InferredType),
    // instance("my-worker") will be simply be parsed Expr::Call("instance", vec!["my-worker"])
    // or as we go, instance[ns:pkg]("my-worker") will be parsed as Expr::Call("instance", vec!["my-worker"])
    // During function call inference phase, the type of this `Expr::Call` will be `Expr::Call(InstanceCreation,..)
    // with inferred-type as `InstanceType`. This way any variables attached to the instance creation
    // will be having the `InstanceType`.
    Call(
        CallType,
        Option<GenericTypeParameter>,
        Vec<Expr>,
        InferredType,
    ),
    // Any calls such as `my-worker-variable-expr.function_name()` will be parsed as Expr::Invoke
    // such that `my-worker-variable-expr` will be of the type `InferredType::InstanceType`
    // As part of a separate type inference phase this will be converted back to `Expr::Call` with fully
    // qualified function names (the complex version) which further takes part in all other type inference phases.
    InvokeLazy {
        lhs: Box<Expr>,
        function_name: String,
        generic_type_parameter: Option<GenericTypeParameter>,
        args: Vec<Expr>,
        inferred_type: InferredType, // This will be the return type of the function similar to Call
    },
    Unwrap(Box<Expr>, InferredType),
    Throw(String, InferredType),
    GetTag(Box<Expr>, InferredType),
    ListComprehension {
        iterated_variable: VariableId,
        iterable_expr: Box<Expr>,
        yield_expr: Box<Expr>,
        inferred_type: InferredType,
    },
    ListReduce {
        reduce_variable: VariableId,
        iterated_variable: VariableId,
        iterable_expr: Box<Expr>,
        yield_expr: Box<Expr>,
        init_value_expr: Box<Expr>,
        inferred_type: InferredType,
    },
}

impl Expr {
    pub fn as_record(&self) -> Option<Vec<(String, Expr)>> {
        match self {
            Expr::Record(fields, _) => Some(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), v.deref().clone()))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        }
    }
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
        spaces()
            .with(block().skip(eof()))
            .easy_parse(position::Stream::new(input))
            .map(|t| t.0)
            .map_err(|err| format!("{}", err))
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
        matches!(self, Expr::Result(_, _, _))
    }

    pub fn is_option(&self) -> bool {
        matches!(self, Expr::Option(_, _, _))
    }

    pub fn is_tuple(&self) -> bool {
        matches!(self, Expr::Tuple(_, _))
    }

    pub fn is_list(&self) -> bool {
        matches!(self, Expr::Sequence(_, _, _))
    }

    pub fn is_flags(&self) -> bool {
        matches!(self, Expr::Flags(_, _))
    }

    pub fn is_identifier(&self) -> bool {
        matches!(self, Expr::Identifier(_, _, _))
    }

    pub fn is_select_field(&self) -> bool {
        matches!(self, Expr::SelectField(_, _, _, _))
    }

    pub fn is_if_else(&self) -> bool {
        matches!(self, Expr::Cond(_, _, _, _))
    }

    pub fn is_function_call(&self) -> bool {
        matches!(self, Expr::Call(_, _, _, _))
    }

    pub fn is_match_expr(&self) -> bool {
        matches!(self, Expr::PatternMatch(_, _, _))
    }

    pub fn is_select_index(&self) -> bool {
        matches!(self, Expr::SelectIndex(_, _, _, _))
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
        matches!(self, Expr::ExprBlock(_, _))
    }

    pub fn inbuilt_variant(&self) -> Option<(String, Option<Expr>)> {
        match self {
            Expr::Option(Some(expr), _, _) => {
                Some(("some".to_string(), Some(expr.deref().clone())))
            }
            Expr::Option(None, _, _) => Some(("some".to_string(), None)),
            Expr::Result(Ok(expr), _, _) => Some(("ok".to_string(), Some(expr.deref().clone()))),
            Expr::Result(Err(expr), _, _) => Some(("err".to_string(), Some(expr.deref().clone()))),
            _ => None,
        }
    }
    pub fn unwrap(&self) -> Self {
        Expr::Unwrap(Box::new(self.clone()), InferredType::Unknown)
    }

    pub fn boolean(value: bool) -> Self {
        Expr::Boolean(value, InferredType::Bool)
    }

    pub fn and(left: Expr, right: Expr) -> Self {
        Expr::And(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn plus(left: Expr, right: Expr) -> Self {
        Expr::Plus(Box::new(left), Box::new(right), InferredType::number())
    }

    pub fn minus(left: Expr, right: Expr) -> Self {
        Expr::Minus(Box::new(left), Box::new(right), InferredType::number())
    }

    pub fn divide(left: Expr, right: Expr) -> Self {
        Expr::Divide(Box::new(left), Box::new(right), InferredType::number())
    }

    pub fn multiply(left: Expr, right: Expr) -> Self {
        Expr::Multiply(Box::new(left), Box::new(right), InferredType::number())
    }

    pub fn and_combine(conditions: Vec<Expr>) -> Option<Expr> {
        let mut cond: Option<Expr> = None;

        for i in conditions {
            let left = Box::new(cond.clone().unwrap_or(Expr::boolean(true)));
            cond = Some(Expr::And(left, Box::new(i), InferredType::Bool));
        }

        cond
    }

    pub fn call(
        dynamic_parsed_fn_name: DynamicParsedFunctionName,
        generic_type_parameter: Option<GenericTypeParameter>,
        worker_name: Option<Expr>,
        args: Vec<Expr>,
    ) -> Self {
        Expr::Call(
            CallType::Function{function_name: dynamic_parsed_fn_name, worker: worker_name.map(Box::new) },
            generic_type_parameter,
            args,
            InferredType::Unknown,
        )
    }

    pub fn invoke_worker_function(
        lhs: Expr,
        function_name: String,
        generic_type_parameter: Option<GenericTypeParameter>,
        args: Vec<Expr>,
    ) -> Self {
        Expr::InvokeLazy {
            lhs: Box::new(lhs),
            function_name,
            generic_type_parameter,
            args,
            inferred_type: InferredType::Unknown,
        }
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

    pub fn err(expr: Expr, type_annotation: Option<TypeName>) -> Self {
        let inferred_type = expr.inferred_type();
        Expr::Result(
            Err(Box::new(expr)),
            type_annotation,
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
    pub fn identifier(name: impl AsRef<str>, type_annotation: Option<TypeName>) -> Self {
        Expr::Identifier(
            VariableId::global(name.as_ref().to_string()),
            type_annotation,
            InferredType::Unknown,
        )
    }

    pub fn less_than(left: Expr, right: Expr) -> Self {
        Expr::LessThan(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn less_than_or_equal_to(left: Expr, right: Expr) -> Self {
        Expr::LessThanOrEqualTo(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn let_binding(
        name: impl AsRef<str>,
        expr: Expr,
        type_annotation: Option<TypeName>,
    ) -> Self {
        Expr::Let(
            VariableId::global(name.as_ref().to_string()),
            type_annotation,
            Box::new(expr),
            InferredType::Unknown,
        )
    }

    pub fn typed_list_reduce(
        reduce_variable: VariableId,
        iterated_variable: VariableId,
        iterable_expr: Expr,
        init_value_expr: Expr,
        yield_expr: Expr,
        inferred_type: InferredType,
    ) -> Self {
        Expr::ListReduce {
            reduce_variable,
            iterated_variable,
            iterable_expr: Box::new(iterable_expr),
            yield_expr: Box::new(yield_expr),
            init_value_expr: Box::new(init_value_expr),
            inferred_type,
        }
    }

    pub fn list_reduce(
        reduce_variable: VariableId,
        iterated_variable: VariableId,
        iterable_expr: Expr,
        init_value_expr: Expr,
        yield_expr: Expr,
    ) -> Self {
        Expr::typed_list_reduce(
            reduce_variable,
            iterated_variable,
            iterable_expr,
            init_value_expr,
            yield_expr,
            InferredType::Unknown,
        )
    }

    pub fn typed_list_comprehension(
        iterated_variable: VariableId,
        iterable_expr: Expr,
        yield_expr: Expr,
        inferred_type: InferredType,
    ) -> Self {
        Expr::ListComprehension {
            iterated_variable,
            iterable_expr: Box::new(iterable_expr),
            yield_expr: Box::new(yield_expr),
            inferred_type,
        }
    }

    pub fn list_comprehension(
        variable_id: VariableId,
        iterable_expr: Expr,
        yield_expr: Expr,
    ) -> Self {
        Expr::typed_list_comprehension(
            variable_id,
            iterable_expr,
            yield_expr,
            InferredType::List(Box::new(InferredType::Unknown)),
        )
    }

    pub fn bind_global_variable_types(
        &self,
        type_spec: &Vec<GlobalVariableTypeSpec>,
    ) -> Result<Self, String> {
        let result_expr = type_inference::bind_global_variable_types(self, type_spec)?;
        Ok(result_expr)
    }

    pub fn bind_instance_types(&mut self) {
        type_inference::bind_instance_types(self)
    }

    pub fn literal(value: impl AsRef<str>) -> Self {
        Expr::Literal(value.as_ref().to_string(), InferredType::Str)
    }

    pub fn empty_expr() -> Self {
        Expr::literal("")
    }

    pub fn expr_block(expressions: Vec<Expr>) -> Self {
        let inferred_type = expressions
            .last()
            .map_or(InferredType::Unknown, |e| e.inferred_type());

        Expr::ExprBlock(expressions, inferred_type)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn not(expr: Expr) -> Self {
        Expr::Not(Box::new(expr), InferredType::Bool)
    }

    pub fn ok(expr: Expr, type_annotation: Option<TypeName>) -> Self {
        let inferred_type = expr.inferred_type();

        Expr::Result(
            Ok(Box::new(expr)),
            type_annotation,
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
            None,
            InferredType::Option(Box::new(inferred_type)),
        )
    }

    pub fn option_with_type_annotation(expr: Option<Expr>, type_annotation: TypeName) -> Self {
        let inferred_type = match &expr {
            Some(expr) => expr.inferred_type(),
            None => InferredType::Unknown,
        };

        Expr::Option(
            expr.map(Box::new),
            Some(type_annotation),
            InferredType::Option(Box::new(inferred_type)),
        )
    }

    pub fn or(left: Expr, right: Expr) -> Self {
        Expr::Or(Box::new(left), Box::new(right), InferredType::Bool)
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

    pub fn select_field(
        expr: Expr,
        field: impl AsRef<str>,
        type_annotation: Option<TypeName>,
    ) -> Self {
        Expr::SelectField(
            Box::new(expr),
            field.as_ref().to_string(),
            type_annotation,
            InferredType::Unknown,
        )
    }

    pub fn select_field_with_type_annotation(
        expr: Expr,
        field: impl AsRef<str>,
        type_annotation: TypeName,
    ) -> Self {
        Expr::SelectField(
            Box::new(expr),
            field.as_ref().to_string(),
            Some(type_annotation),
            InferredType::Unknown,
        )
    }

    pub fn select_index(expr: Expr, index: usize) -> Self {
        Expr::SelectIndex(Box::new(expr), index, None, InferredType::Unknown)
    }

    pub fn select_index_with_type_annotation(
        expr: Expr,
        index: usize,
        type_annotation: TypeName,
    ) -> Self {
        Expr::SelectIndex(
            Box::new(expr),
            index,
            Some(type_annotation),
            InferredType::Unknown,
        )
    }

    pub fn get_tag(expr: Expr) -> Self {
        Expr::GetTag(Box::new(expr), InferredType::Unknown)
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

    pub fn sequence(expressions: Vec<Expr>, type_annotation: Option<TypeName>) -> Self {
        let inferred_type = InferredType::List(Box::new(
            expressions
                .first()
                .map_or(InferredType::Unknown, |x| x.inferred_type()),
        ));

        Expr::Sequence(expressions, type_annotation, inferred_type)
    }

    pub fn inferred_type(&self) -> InferredType {
        match self {
            Expr::Let(_, _, _, inferred_type)
            | Expr::SelectField(_, _, _, inferred_type)
            | Expr::SelectIndex(_, _, _, inferred_type)
            | Expr::Sequence(_, _, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, _, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Identifier(_, _, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::ExprBlock(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::Plus(_, _, inferred_type)
            | Expr::Minus(_, _, inferred_type)
            | Expr::Divide(_, _, inferred_type)
            | Expr::Multiply(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, _, inferred_type)
            | Expr::Result(_, _, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::GetTag(_, inferred_type)
            | Expr::And(_, _, inferred_type)
            | Expr::Or(_, _, inferred_type)
            | Expr::ListComprehension { inferred_type, .. }
            | Expr::ListReduce { inferred_type, .. }
            | Expr::Call(_, _, _, inferred_type)
            | Expr::InvokeLazy { inferred_type, .. } => inferred_type.clone(),
        }
    }

    pub fn infer_types(
        &mut self,
        function_type_registry: &FunctionTypeRegistry,
        type_spec: &Vec<GlobalVariableTypeSpec>,
    ) -> Result<(), Vec<String>> {
        self.infer_types_initial_phase(function_type_registry, type_spec)?;
        self.bind_instance_types();

        // Identifying the first fix point with method calls to infer all
        // worker function invocations as this forms the foundation for the rest of the
        // compilation. This is compiler doing its best to infer all the calls such
        // as worker invokes or instance calls etc.
        type_inference::type_inference_fix_point(Self::resolve_method_calls, self)
            .map_err(|x| vec![x])?;

        self.infer_worker_function_invokes().map_err(|x| vec![x])?;

        self.bind_instance_types();
        self.infer_worker_function_invokes().map_err(|x| vec![x])?;
        self.infer_function_call_types(function_type_registry)
            .map_err(|x| vec![x])?;

        type_inference::type_inference_fix_point(Self::inference_scan, self)
            .map_err(|x| vec![x])?;

        self.check_types(function_type_registry)
            .map_err(|x| vec![x])?;
        self.unify_types()?;
        Ok(())
    }

    pub fn infer_types_initial_phase(
        &mut self,
        function_type_registry: &FunctionTypeRegistry,
        type_spec: &Vec<GlobalVariableTypeSpec>,
    ) -> Result<(), Vec<String>> {
        self.identify_instance_creation(function_type_registry)
            .map_err(|x| vec![x])?;
        *self = self
            .bind_global_variable_types(type_spec)
            .map_err(|x| vec![x])?;
        self.bind_type_annotations();
        self.bind_variables_of_list_comprehension();
        self.bind_variables_of_list_reduce();
        self.bind_variables_of_pattern_match();
        self.bind_variables_of_let_assignment();
        self.infer_variants(function_type_registry);
        self.infer_enums(function_type_registry);

        Ok(())
    }

    pub fn resolve_method_calls(&mut self) -> Result<(), String> {
        self.bind_instance_types();
        self.infer_worker_function_invokes()
    }

    // An inference is a single cycle of to-and-fro scanning of Rib expression, that it takes part in fix point of inference.
    // Not all phases of compilation will be part of this scan.
    // Example: function call argument inference based on the worker function hardly needs to be part of the scan.
    pub fn inference_scan(&mut self) -> Result<(), String> {
        self.infer_all_identifiers()?;
        self.push_types_down()?;
        self.infer_all_identifiers()?;
        let expr = self.pull_types_up()?;
        *self = expr;
        self.infer_global_inputs();
        Ok(())
    }

    pub fn infer_worker_function_invokes(&mut self) -> Result<(), String> {
        type_inference::infer_worker_function_invokes(self)
    }

    // Make sure the bindings in the arm pattern of a pattern match are given variable-ids.
    // The same variable-ids will be tagged to the corresponding identifiers in the arm resolution
    // to avoid conflicts.
    pub fn bind_variables_of_pattern_match(&mut self) {
        type_inference::bind_variables_of_pattern_match(self);
    }

    // Make sure the variable assignment (let binding) are given variable ids,
    // which will be tagged to the corresponding identifiers to avoid conflicts.
    // This is done only for local variables and not global variables
    pub fn bind_variables_of_let_assignment(&mut self) {
        type_inference::bind_variables_of_let_assignment(self);
    }

    pub fn bind_variables_of_list_comprehension(&mut self) {
        type_inference::bind_variables_of_list_comprehension(self);
    }

    pub fn bind_variables_of_list_reduce(&mut self) {
        type_inference::bind_variables_of_list_reduce(self);
    }

    pub fn identify_instance_creation(
        &mut self,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<(), String> {
        type_inference::identify_instance_creation(self, function_type_registry)
    }

    pub fn infer_function_call_types(
        &mut self,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<(), String> {
        type_inference::infer_function_call_types(self, function_type_registry)
    }

    pub fn push_types_down(&mut self) -> Result<(), String> {
        type_inference::push_types_down(self)
    }

    pub fn infer_all_identifiers(&mut self) -> Result<(), String> {
        type_inference::infer_all_identifiers(self)
    }

    pub fn pull_types_up(&self) -> Result<Expr, String> {
        type_inference::type_pull_up(self)
    }

    pub fn infer_global_inputs(&mut self) {
        type_inference::infer_global_inputs(self);
    }

    pub fn bind_type_annotations(&mut self) {
        type_inference::bind_type_annotations(self);
    }

    pub fn check_types(
        &mut self,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<(), String> {
        type_checker::type_check(self, function_type_registry)
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
            Expr::Identifier(_, _, inferred_type)
            | Expr::Let(_, _, _, inferred_type)
            | Expr::SelectField(_, _, _, inferred_type)
            | Expr::SelectIndex(_, _, _, inferred_type)
            | Expr::Sequence(_, _, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, _, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::ExprBlock(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::Plus(_, _, inferred_type)
            | Expr::Minus(_, _, inferred_type)
            | Expr::Divide(_, _, inferred_type)
            | Expr::Multiply(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, _, inferred_type)
            | Expr::Result(_, _, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::GetTag(_, inferred_type)
            | Expr::And(_, _, inferred_type)
            | Expr::Or(_, _, inferred_type)
            | Expr::ListComprehension { inferred_type, .. }
            | Expr::ListReduce { inferred_type, .. }
            | Expr::InvokeLazy { inferred_type, .. }
            | Expr::Call(_, _, _, inferred_type) => {
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
            Expr::Identifier(_, _, inferred_type)
            | Expr::Let(_, _, _, inferred_type)
            | Expr::SelectField(_, _, _, inferred_type)
            | Expr::SelectIndex(_, _, _, inferred_type)
            | Expr::Sequence(_, _, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, _, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::ExprBlock(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Plus(_, _, inferred_type)
            | Expr::Minus(_, _, inferred_type)
            | Expr::Divide(_, _, inferred_type)
            | Expr::Multiply(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, _, inferred_type)
            | Expr::Result(_, _, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::And(_, _, inferred_type)
            | Expr::Or(_, _, inferred_type)
            | Expr::GetTag(_, inferred_type)
            | Expr::ListComprehension { inferred_type, .. }
            | Expr::ListReduce { inferred_type, .. }
            | Expr::InvokeLazy { inferred_type, .. }
            | Expr::Call(_, _, _, inferred_type) => {
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

    pub fn number(
        big_decimal: BigDecimal,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
    ) -> Expr {
        Expr::Number(
            Number { value: big_decimal },
            type_annotation,
            inferred_type,
        )
    }

    pub fn untyped_number(big_decimal: BigDecimal) -> Expr {
        Expr::number(big_decimal, None, InferredType::number())
    }

    // TODO; introduced to minimise the number of changes in tests.
    pub fn untyped_number_with_type_name(big_decimal: BigDecimal, type_name: TypeName) -> Expr {
        Expr::number(big_decimal, Some(type_name), InferredType::number())
    }
}

#[derive(Debug, Hash, Clone, PartialEq, Ord, PartialOrd)]
pub struct Number {
    pub value: BigDecimal,
}

impl Eq for Number {}

impl Number {
    pub fn to_val(&self, analysed_type: &AnalysedType) -> Option<ValueAndType> {
        match analysed_type {
            AnalysedType::F64(_) => self.value.to_f64().map(|v| v.into_value_and_type()),
            AnalysedType::U64(_) => self.value.to_u64().map(|v| v.into_value_and_type()),
            AnalysedType::F32(_) => self.value.to_f32().map(|v| v.into_value_and_type()),
            AnalysedType::U32(_) => self.value.to_u32().map(|v| v.into_value_and_type()),
            AnalysedType::S32(_) => self.value.to_i32().map(|v| v.into_value_and_type()),
            AnalysedType::S64(_) => self.value.to_i64().map(|v| v.into_value_and_type()),
            AnalysedType::U8(_) => self.value.to_u8().map(|v| v.into_value_and_type()),
            AnalysedType::S8(_) => self.value.to_i8().map(|v| v.into_value_and_type()),
            AnalysedType::U16(_) => self.value.to_u16().map(|v| v.into_value_and_type()),
            AnalysedType::S16(_) => self.value.to_i16().map(|v| v.into_value_and_type()),
            _ => None,
        }
    }
}

impl Display for Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

#[derive(Debug, Hash, Clone, PartialEq, Eq, Ord, PartialOrd)]
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
#[derive(Debug, Hash, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub enum ArmPattern {
    WildCard,
    As(String, Box<ArmPattern>),
    Constructor(String, Vec<ArmPattern>),
    TupleConstructor(Vec<ArmPattern>),
    RecordConstructor(Vec<(String, ArmPattern)>),
    ListConstructor(Vec<ArmPattern>),
    Literal(Box<Expr>),
}

impl ArmPattern {
    pub fn is_wildcard(&self) -> bool {
        matches!(self, ArmPattern::WildCard)
    }

    pub fn is_literal_identifier(&self) -> bool {
        matches!(self, ArmPattern::Literal(expr) if expr.is_identifier())
    }

    pub fn constructor(name: &str, patterns: Vec<ArmPattern>) -> ArmPattern {
        ArmPattern::Constructor(name.to_string(), patterns)
    }

    pub fn literal(expr: Expr) -> ArmPattern {
        ArmPattern::Literal(Box::new(expr))
    }

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
            ArmPattern::RecordConstructor(patterns) => {
                let mut result = vec![];
                for (_, pattern) in patterns {
                    result.extend(pattern.get_expr_literals_mut());
                }
                result
            }
            ArmPattern::ListConstructor(patterns) => {
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
            ArmPattern::RecordConstructor(patterns) => {
                let mut result = vec![];
                for (_, pattern) in patterns {
                    result.extend(pattern.get_expr_literals());
                }
                result
            }
            ArmPattern::ListConstructor(patterns) => {
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
                None,
                InferredType::Unknown,
            ))),
            None,
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
                None,
                InferredType::Unknown,
            ))),
            None,
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
                None,
                InferredType::Unknown,
            ))),
            None,
            InferredType::Unknown,
        )))
    }

    pub fn none() -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option(None, None, InferredType::Unknown)))
    }

    pub fn identifier(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Identifier(
            VariableId::global(binding_variable.to_string()),
            None,
            InferredType::Unknown,
        )))
    }
    pub fn custom_constructor(name: &str, args: Vec<ArmPattern>) -> ArmPattern {
        ArmPattern::Constructor(name.to_string(), args)
    }
}

#[cfg(feature = "protobuf")]
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
                Expr::let_binding(name, expr, type_name)
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

            golem_api_grpc::proto::golem::rib::expr::Expr::Add(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::plus((*left).try_into()?, (*right).try_into()?)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Subtract(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::plus((*left).try_into()?, (*right).try_into()?)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Divide(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::plus((*left).try_into()?, (*right).try_into()?)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Multiply(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::plus((*left).try_into()?, (*right).try_into()?)
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
                Expr::expr_block(exprs)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
                golem_api_grpc::proto::golem::rib::SequenceExpr { exprs, type_name },
            ) => {
                let type_name = type_name.map(TypeName::try_from).transpose()?;

                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::sequence(exprs, type_name)
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
                golem_api_grpc::proto::golem::rib::IdentifierExpr { name, type_name },
            ) => {
                let type_name = type_name.map(TypeName::try_from).transpose()?;

                Expr::identifier(name.as_str(), type_name)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
                golem_api_grpc::proto::golem::rib::BooleanExpr { value },
            ) => Expr::boolean(value),

            golem_api_grpc::proto::golem::rib::expr::Expr::Throw(
                golem_api_grpc::proto::golem::rib::ThrowExpr { message },
            ) => Expr::Throw(message, InferredType::Unknown),

            golem_api_grpc::proto::golem::rib::expr::Expr::And(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::and((*left).try_into()?, (*right).try_into()?)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Or(expr) => {
                let left = expr.left.ok_or("Missing left expr")?;
                let right = expr.right.ok_or("Missing right expr")?;
                Expr::or((*left).try_into()?, (*right).try_into()?)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Tag(expr) => {
                let expr = expr.expr.ok_or("Missing expr in tag")?;
                Expr::get_tag((*expr).try_into()?)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Unwrap(expr) => {
                let expr = expr.expr.ok_or("Missing expr")?;
                let expr: Expr = (*expr).try_into()?;
                expr.unwrap()
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Number(number) => {
                // Backward compatibility
                let type_name = number.type_name.map(TypeName::try_from).transpose()?;
                let big_decimal = if let Some(number) = number.number {
                    BigDecimal::from_str(&number).map_err(|e| e.to_string())?
                } else if let Some(float) = number.float {
                    BigDecimal::from_f64(float).ok_or("Invalid float")?
                } else {
                    return Err("Missing number".to_string());
                };

                if let Some(type_name) = type_name {
                    Expr::untyped_number_with_type_name(big_decimal, type_name.clone())
                } else {
                    Expr::untyped_number(big_decimal)
                }
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(expr) => {
                let expr = *expr;
                let field = expr.field;
                let type_name = expr.type_name.map(TypeName::try_from).transpose()?;
                let expr = *expr.expr.ok_or(
                    "Mi\
                ssing expr",
                )?;

                Expr::select_field(expr.try_into()?, field.as_str(), type_name)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndex(expr) => {
                let expr = *expr;
                let type_name = expr.type_name.map(TypeName::try_from).transpose()?;
                let index = expr.index as usize;
                let expr = *expr.expr.ok_or("Missing expr")?;

                if let Some(type_name) = type_name {
                    Expr::select_index_with_type_annotation(expr.try_into()?, index, type_name)
                } else {
                    Expr::select_index(expr.try_into()?, index)
                }
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Option(expr) => {
                let type_name = expr.type_name;
                let type_name = type_name.map(TypeName::try_from).transpose()?;

                match type_name {
                    Some(type_name) => match expr.expr {
                        Some(expr) => {
                            Expr::option_with_type_annotation(Some((*expr).try_into()?), type_name)
                        }
                        None => Expr::option_with_type_annotation(None, type_name),
                    },

                    None => match expr.expr {
                        Some(expr) => Expr::option(Some((*expr).try_into()?)),
                        None => Expr::option(None),
                    },
                }
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Result(expr) => {
                let type_name = expr.type_name;
                let type_name = type_name.map(TypeName::try_from).transpose()?;
                let result = expr.result.ok_or("Missing result")?;
                match result {
                    golem_api_grpc::proto::golem::rib::result_expr::Result::Ok(expr) => {
                        Expr::ok((*expr).try_into()?, type_name)
                    }
                    golem_api_grpc::proto::golem::rib::result_expr::Result::Err(expr) => {
                        Expr::err((*expr).try_into()?, type_name)
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
            golem_api_grpc::proto::golem::rib::expr::Expr::ListComprehension(
                list_comprehension,
            ) => {
                let iterable_expr = list_comprehension.iterable_expr.ok_or("Missing expr")?;
                let iterable_expr = (*iterable_expr).try_into()?;
                let yield_expr = list_comprehension.yield_expr.ok_or("Missing list")?;
                let yield_expr = (*yield_expr).try_into()?;
                let variable_id =
                    VariableId::list_comprehension_identifier(list_comprehension.iterated_variable);
                Expr::list_comprehension(variable_id, iterable_expr, yield_expr)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::ListReduce(list_reduce) => {
                let init_value_expr = list_reduce.init_value_expr.ok_or("Missing initial expr")?;
                let init_value_expr = (*init_value_expr).try_into()?;
                let iterable_expr = list_reduce.iterable_expr.ok_or("Missing expr")?;
                let iterable_expr = (*iterable_expr).try_into()?;
                let yield_expr = list_reduce.yield_expr.ok_or("Missing list")?;
                let yield_expr = (*yield_expr).try_into()?;
                let iterated_variable_id =
                    VariableId::list_comprehension_identifier(list_reduce.iterated_variable);
                let reduce_variable_id =
                    VariableId::list_reduce_identifier(list_reduce.reduce_variable);
                Expr::list_reduce(
                    reduce_variable_id,
                    iterated_variable_id,
                    iterable_expr,
                    init_value_expr,
                    yield_expr,
                )
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Call(expr) => {
                let params: Vec<Expr> = expr
                    .params
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                // This is not required and kept for backward compatibility
                let legacy_invocation_name = expr.name;
                let call_type = expr.call_type;

                match (legacy_invocation_name, call_type) {
                    (Some(legacy), None) => {
                        let name = legacy.name.ok_or("Missing function call name")?;
                        match name {
                            golem_api_grpc::proto::golem::rib::invocation_name::Name::Parsed(name) => {
                                // Reading the previous parsed-function-name in persistent store as a dynamic-parsed-function-name
                                Expr::call(DynamicParsedFunctionName::parse(
                                    ParsedFunctionName::try_from(name)?.to_string()
                                )?, None, None, params)
                            }
                            golem_api_grpc::proto::golem::rib::invocation_name::Name::VariantConstructor(
                                name,
                            ) => Expr::call(DynamicParsedFunctionName::parse(name)?, None, None, params),
                            golem_api_grpc::proto::golem::rib::invocation_name::Name::EnumConstructor(
                                name,
                            ) => Expr::call(DynamicParsedFunctionName::parse(name)?, None, None, params),
                        }
                    }
                    (_, Some(call_type)) => {
                        let name = call_type.name.ok_or("Missing function call name")?;
                        match name {
                            golem_api_grpc::proto::golem::rib::call_type::Name::Parsed(name) => {
                                Expr::call(name.try_into()?, None, None, params)
                            }
                            golem_api_grpc::proto::golem::rib::call_type::Name::VariantConstructor(
                                name,
                            ) => Expr::call(DynamicParsedFunctionName::parse(name)?, None, None, params),
                            golem_api_grpc::proto::golem::rib::call_type::Name::EnumConstructor(
                                name,
                            ) => Expr::call(DynamicParsedFunctionName::parse(name)?, None, None, params),
                        }
                    }
                    (_, _) => Err("Missing both call type (and legacy invocation type)")?,
                }
            }
        };
        Ok(expr)
    }
}

impl Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", text::to_string(self).unwrap())
    }
}

impl Display for ArmPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", text::to_string_arm_pattern(self).unwrap())
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            Value::String(expr_string) => match from_string(expr_string.as_str()) {
                Ok(expr) => Ok(expr),
                Err(message) => Err(serde::de::Error::custom(message.to_string())),
            },

            e => Err(serde::de::Error::custom(format!(
                "Failed to deserialize expression {}",
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

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::{ArmPattern, Expr, MatchArm};

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
                Expr::SelectField(expr, field, type_name, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(
                        Box::new(golem_api_grpc::proto::golem::rib::SelectFieldExpr {
                            expr: Some(Box::new((*expr).into())),
                            field,
                            type_name: type_name.map(|t| t.into()),
                        }),
                    ))
                }
                Expr::SelectIndex(expr, index, type_name, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndex(
                        Box::new(golem_api_grpc::proto::golem::rib::SelectIndexExpr {
                            expr: Some(Box::new((*expr).into())),
                            index: index as u64,
                            type_name: type_name.map(|t| t.into()),
                        }),
                    ))
                }
                Expr::Sequence(exprs, type_name, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
                        golem_api_grpc::proto::golem::rib::SequenceExpr {
                            exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                            type_name: type_name.map(|t| t.into()),
                        },
                    ))
                }
                Expr::Record(fields, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Record(
                        golem_api_grpc::proto::golem::rib::RecordExpr {
                            fields: fields
                                .into_iter()
                                .map(|(name, expr)| {
                                    golem_api_grpc::proto::golem::rib::RecordFieldExpr {
                                        name,
                                        expr: Some((*expr).into()),
                                    }
                                })
                                .collect(),
                        },
                    ))
                }
                Expr::Tuple(exprs, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Tuple(
                        golem_api_grpc::proto::golem::rib::TupleExpr {
                            exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                        },
                    ))
                }
                Expr::Literal(value, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Literal(
                        golem_api_grpc::proto::golem::rib::LiteralExpr { value },
                    ))
                }
                Expr::Number(number, type_name, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Number(
                        golem_api_grpc::proto::golem::rib::NumberExpr {
                            number: Some(number.value.to_string()),
                            float: None,
                            type_name: type_name.map(|t| t.into()),
                        },
                    ))
                }
                Expr::Flags(values, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Flags(
                        golem_api_grpc::proto::golem::rib::FlagsExpr { values },
                    ))
                }
                Expr::Identifier(variable_id, type_name, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Identifier(
                        golem_api_grpc::proto::golem::rib::IdentifierExpr {
                            name: variable_id.name(),
                            type_name: type_name.map(|t| t.into()),
                        },
                    ))
                }
                Expr::Boolean(value, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
                        golem_api_grpc::proto::golem::rib::BooleanExpr { value },
                    ))
                }
                Expr::Concat(exprs, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Concat(
                        golem_api_grpc::proto::golem::rib::ConcatExpr {
                            exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
                        },
                    ))
                }
                Expr::ExprBlock(exprs, _) => {
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
                Expr::Plus(left, right, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Add(
                        Box::new(golem_api_grpc::proto::golem::rib::AddExpr {
                            left: Some(Box::new((*left).into())),
                            right: Some(Box::new((*right).into())),
                        }),
                    ))
                }
                Expr::Minus(left, right, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Subtract(
                        Box::new(golem_api_grpc::proto::golem::rib::SubtractExpr {
                            left: Some(Box::new((*left).into())),
                            right: Some(Box::new((*right).into())),
                        }),
                    ))
                }
                Expr::Divide(left, right, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Divide(
                        Box::new(golem_api_grpc::proto::golem::rib::DivideExpr {
                            left: Some(Box::new((*left).into())),
                            right: Some(Box::new((*right).into())),
                        }),
                    ))
                }
                Expr::Multiply(left, right, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Multiply(
                        Box::new(golem_api_grpc::proto::golem::rib::MultiplyExpr {
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
                Expr::Option(expr, optional_type_name, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Option(
                        Box::new(golem_api_grpc::proto::golem::rib::OptionExpr {
                            expr: expr.map(|expr| Box::new((*expr).into())),
                            type_name: optional_type_name.map(|t| t.into()),
                        }),
                    ))
                }
                Expr::Result(expr, type_name, _) => {
                    let type_name = type_name.map(|t| t.into());

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
                            type_name,
                        }),
                    ))
                }
                Expr::Call(function_name, _, args, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Call(
                        golem_api_grpc::proto::golem::rib::CallExpr {
                            name: None,
                            params: args.into_iter().map(|expr| expr.into()).collect(),
                            call_type: Some(function_name.into()),
                        },
                    ))
                }
                Expr::Unwrap(expr, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Unwrap(
                        Box::new(golem_api_grpc::proto::golem::rib::UnwrapExpr {
                            expr: Some(Box::new((*expr).into())),
                        }),
                    ))
                }
                Expr::Throw(message, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Throw(
                        golem_api_grpc::proto::golem::rib::ThrowExpr { message },
                    ))
                }
                Expr::GetTag(expr, _) => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Tag(
                    Box::new(golem_api_grpc::proto::golem::rib::GetTagExpr {
                        expr: Some(Box::new((*expr).into())),
                    }),
                )),
                Expr::And(left, right, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::And(
                        Box::new(golem_api_grpc::proto::golem::rib::AndExpr {
                            left: Some(Box::new((*left).into())),
                            right: Some(Box::new((*right).into())),
                        }),
                    ))
                }

                Expr::Or(left, right, _) => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Or(Box::new(
                        golem_api_grpc::proto::golem::rib::OrExpr {
                            left: Some(Box::new((*left).into())),
                            right: Some(Box::new((*right).into())),
                        },
                    )))
                }
                Expr::ListComprehension {
                    iterated_variable,
                    iterable_expr,
                    yield_expr,
                    ..
                } => Some(
                    golem_api_grpc::proto::golem::rib::expr::Expr::ListComprehension(Box::new(
                        golem_api_grpc::proto::golem::rib::ListComprehensionExpr {
                            iterated_variable: iterated_variable.name(),
                            iterable_expr: Some(Box::new((*iterable_expr).into())),
                            yield_expr: Some(Box::new((*yield_expr).into())),
                        },
                    )),
                ),

                Expr::ListReduce {
                    reduce_variable,
                    iterated_variable,
                    iterable_expr,
                    yield_expr,
                    init_value_expr,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::ListReduce(
                    Box::new(golem_api_grpc::proto::golem::rib::ListReduceExpr {
                        reduce_variable: reduce_variable.name(),
                        iterated_variable: iterated_variable.name(),
                        iterable_expr: Some(Box::new((*iterable_expr).into())),
                        init_value_expr: Some(Box::new((*init_value_expr).into())),
                        yield_expr: Some(Box::new((*yield_expr).into())),
                    }),
                )),
                Expr::InvokeLazy { .. } => {
                    todo!("Invoke is not supported in protobuf serialization")
                }
            };

            golem_api_grpc::proto::golem::rib::Expr { expr }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::rib::ArmPattern> for ArmPattern {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::rib::ArmPattern,
        ) -> Result<Self, Self::Error> {
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
                golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::TupleConstructor(
                    golem_api_grpc::proto::golem::rib::TupleConstructorArmPattern { patterns },
                ) => {
                    let patterns = patterns
                        .into_iter()
                        .map(ArmPattern::try_from)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(ArmPattern::TupleConstructor(patterns))
                }
                golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::Literal(
                    golem_api_grpc::proto::golem::rib::LiteralArmPattern { expr },
                ) => {
                    let inner = expr.ok_or("Missing expr")?;
                    Ok(ArmPattern::Literal(Box::new(inner.try_into()?)))
                }
                golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::RecordConstructor(
                    golem_api_grpc::proto::golem::rib::RecordConstructorArmPattern { fields },
                ) => {
                    let fields = fields
                        .into_iter()
                        .map(|field| {
                            let name = field.name;
                            let proto_pattern = field.pattern.ok_or("Missing pattern")?;
                            let arm_pattern = ArmPattern::try_from(proto_pattern)?;
                            Ok((name, arm_pattern))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    Ok(ArmPattern::RecordConstructor(fields))
                }
                golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::ListConstructor(
                    golem_api_grpc::proto::golem::rib::ListConstructorArmPattern { patterns },
                ) => {
                    let patterns = patterns
                        .into_iter()
                        .map(ArmPattern::try_from)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(ArmPattern::ListConstructor(patterns))
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

                ArmPattern::TupleConstructor(patterns) => {
                    golem_api_grpc::proto::golem::rib::ArmPattern {
                        pattern: Some(
                            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::TupleConstructor(
                                golem_api_grpc::proto::golem::rib::TupleConstructorArmPattern {
                                    patterns: patterns
                                        .into_iter()
                                        .map(golem_api_grpc::proto::golem::rib::ArmPattern::from)
                                        .collect(),
                                },
                            ),
                        ),
                    }
                }

                ArmPattern::RecordConstructor(fields) => {
                    golem_api_grpc::proto::golem::rib::ArmPattern {
                        pattern: Some(
                            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::RecordConstructor(
                                golem_api_grpc::proto::golem::rib::RecordConstructorArmPattern {
                                    fields: fields
                                        .into_iter()
                                        .map(|(name, pattern)| {
                                            golem_api_grpc::proto::golem::rib::RecordFieldArmPattern {
                                                name,
                                                pattern: Some(pattern.into()),
                                            }
                                        })
                                        .collect(),
                                },
                            ),
                        ),
                    }
                }

                ArmPattern::ListConstructor(patterns) => {
                    golem_api_grpc::proto::golem::rib::ArmPattern {
                        pattern: Some(
                            golem_api_grpc::proto::golem::rib::arm_pattern::Pattern::ListConstructor(
                                golem_api_grpc::proto::golem::rib::ListConstructorArmPattern {
                                    patterns: patterns
                                        .into_iter()
                                        .map(golem_api_grpc::proto::golem::rib::ArmPattern::from)
                                        .collect(),
                                },
                            ),
                        ),
                    }
                }
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::rib::MatchArm> for MatchArm {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::rib::MatchArm,
        ) -> Result<Self, Self::Error> {
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
}

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::ParsedFunctionSite::PackagedInterface;
    use crate::{
        ArmPattern, DynamicParsedFunctionName, DynamicParsedFunctionReference, Expr, MatchArm,
    };

    #[test]
    fn test_single_expr_in_interpolation_wrapped_in_quotes() {
        let input = r#""${foo}""#;
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::concat(vec![Expr::identifier("foo", None)]))
        );

        let input = r#""${{foo}}""#;
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::concat(vec![Expr::flags(vec!["foo".to_string()])]))
        );

        let input = r#""${{foo: "bar"}}""#;
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::concat(vec![Expr::record(vec![(
                "foo".to_string(),
                Expr::literal("bar")
            )])]))
        );
    }

    fn expected() -> Expr {
        Expr::expr_block(vec![
            Expr::let_binding("x", Expr::untyped_number(BigDecimal::from(1)), None),
            Expr::let_binding("y", Expr::untyped_number(BigDecimal::from(2)), None),
            Expr::let_binding(
                "result",
                Expr::greater_than(Expr::identifier("x", None), Expr::identifier("y", None)),
                None,
            ),
            Expr::let_binding(
                "foo",
                Expr::option(Some(Expr::identifier("result", None))),
                None,
            ),
            Expr::let_binding(
                "bar",
                Expr::ok(Expr::identifier("result", None), None),
                None,
            ),
            Expr::let_binding(
                "baz",
                Expr::pattern_match(
                    Expr::identifier("foo", None),
                    vec![
                        MatchArm::new(
                            ArmPattern::constructor(
                                "some",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("x", None)))],
                            ),
                            Expr::identifier("x", None),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            Expr::boolean(false),
                        ),
                    ],
                ),
                None,
            ),
            Expr::let_binding(
                "qux",
                Expr::pattern_match(
                    Expr::identifier("bar", None),
                    vec![
                        MatchArm::new(
                            ArmPattern::constructor(
                                "ok",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("x", None)))],
                            ),
                            Expr::identifier("x", None),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "err",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("msg", None)))],
                            ),
                            Expr::boolean(false),
                        ),
                    ],
                ),
                None,
            ),
            Expr::let_binding(
                "result",
                Expr::call(
                    DynamicParsedFunctionName {
                        site: PackagedInterface {
                            namespace: "ns".to_string(),
                            package: "name".to_string(),
                            interface: "interface".to_string(),
                            version: None,
                        },
                        function: DynamicParsedFunctionReference::RawResourceStaticMethod {
                            resource: "resource1".to_string(),
                            method: "do-something-static".to_string(),
                        },
                    },
                    None,
                    None,
                    vec![Expr::identifier("baz", None), Expr::identifier("qux", None)],
                ),
                None,
            ),
            Expr::identifier("result", None),
        ])
    }

    #[test]
    fn test_rib() {
        let sample_rib = r#"
         let x = 1;
         let y = 2;
         let result = x > y;
         let foo = some(result);
         let bar = ok(result);

         let baz = match foo {
           some(x) => x,
           none => false
         };

         let qux = match bar {
           ok(x) => x,
           err(msg) => false
         };

         let result = ns:name/interface.{[static]resource1.do-something-static}(baz, qux);

         result
       "#;

        let result = Expr::from_text(sample_rib);
        assert_eq!(result, Ok(expected()));
    }
}
