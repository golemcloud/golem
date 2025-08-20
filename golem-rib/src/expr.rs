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

use crate::call_type::{CallType, InstanceCreationType};
use crate::generic_type_parameter::GenericTypeParameter;
use crate::inferred_type::{DefaultType, TypeOrigin};
use crate::parser::block::block;
use crate::parser::type_name::TypeName;
use crate::rib_source_span::SourceSpan;
use crate::rib_type_error::RibTypeErrorInternal;
use crate::{
    from_string, text, type_checker, type_inference, ComponentDependencies, ComponentDependencyKey,
    DynamicParsedFunctionName, ExprVisitor, GlobalVariableTypeSpec, InferredType,
    InstanceIdentifier, ParsedFunctionName, VariableId,
};
use bigdecimal::{BigDecimal, FromPrimitive, ToPrimitive};
use combine::parser::char::spaces;
use combine::stream::position;
use combine::Parser;
use combine::{eof, EasyParser};
use golem_api_grpc::proto::golem::rib::range_expr::RangeExpr;
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
    Let {
        variable_id: VariableId,
        type_annotation: Option<TypeName>,
        expr: Box<Expr>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    SelectField {
        expr: Box<Expr>,
        field: String,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    SelectIndex {
        expr: Box<Expr>,
        index: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Sequence {
        exprs: Vec<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Range {
        range: Range,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Record {
        exprs: Vec<(String, Box<Expr>)>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Tuple {
        exprs: Vec<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Literal {
        value: String,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Number {
        number: Number,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Flags {
        flags: Vec<String>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Identifier {
        variable_id: VariableId,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Boolean {
        value: bool,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Concat {
        exprs: Vec<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    ExprBlock {
        exprs: Vec<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Not {
        expr: Box<Expr>,
        inferred_type: InferredType,
        type_annotation: Option<TypeName>,
        source_span: SourceSpan,
    },
    GreaterThan {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    And {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Or {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    GreaterThanOrEqualTo {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    LessThanOrEqualTo {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Plus {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Multiply {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Minus {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Divide {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    EqualTo {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    LessThan {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Cond {
        cond: Box<Expr>,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    PatternMatch {
        predicate: Box<Expr>,
        match_arms: Vec<MatchArm>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Option {
        expr: Option<Box<Expr>>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Result {
        expr: Result<Box<Expr>, Box<Expr>>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    // instance[t]("my-worker") will be parsed sd Expr::Call { "instance", Some(t }, vec!["my-worker"] }
    // will be parsed as Expr::Call { "instance", vec!["my-worker"] }.
    // During function call inference phase, the type of this `Expr::Call` will be `Expr::Call { InstanceCreation,.. }
    // with inferred-type as `InstanceType`. This way any variables attached to the instance creation
    // will be having the `InstanceType`.
    Call {
        call_type: CallType,
        generic_type_parameter: Option<GenericTypeParameter>,
        args: Vec<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    // Any calls such as `my-worker-variable-expr.function_name()` will be parsed as Expr::Invoke
    // such that `my-worker-variable-expr` (lhs) will be of the type `InferredType::InstanceType`. `lhs` will
    // be `Expr::Call { InstanceCreation }` with type `InferredType::InstanceType`.
    // As part of a separate type inference phase this will be converted back to `Expr::Call` with fully
    // qualified function names (the complex version) which further takes part in all other type inference phases.
    InvokeMethodLazy {
        lhs: Box<Expr>,
        method: String,
        generic_type_parameter: Option<GenericTypeParameter>,
        args: Vec<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Unwrap {
        expr: Box<Expr>,
        inferred_type: InferredType,
        type_annotation: Option<TypeName>,
        source_span: SourceSpan,
    },
    Throw {
        message: String,
        inferred_type: InferredType,
        type_annotation: Option<TypeName>,
        source_span: SourceSpan,
    },
    GetTag {
        expr: Box<Expr>,
        inferred_type: InferredType,
        type_annotation: Option<TypeName>,
        source_span: SourceSpan,
    },
    ListComprehension {
        iterated_variable: VariableId,
        iterable_expr: Box<Expr>,
        yield_expr: Box<Expr>,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    ListReduce {
        reduce_variable: VariableId,
        iterated_variable: VariableId,
        iterable_expr: Box<Expr>,
        type_annotation: Option<TypeName>,
        yield_expr: Box<Expr>,
        init_value_expr: Box<Expr>,
        inferred_type: InferredType,
        source_span: SourceSpan,
    },
    Length {
        expr: Box<Expr>,
        inferred_type: InferredType,
        type_annotation: Option<TypeName>,
        source_span: SourceSpan,
    },

    GenerateWorkerName {
        inferred_type: InferredType,
        type_annotation: Option<TypeName>,
        source_span: SourceSpan,
        variable_id: Option<VariableId>,
    },
}

impl Expr {
    pub fn as_record(&self) -> Option<Vec<(String, Expr)>> {
        match self {
            Expr::Record { exprs: fields, .. } => Some(
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
    ///   let shopping-cart-worker = instance("my-worker");
    ///   let result = shopping-cart-worker.add-to-cart({product-name: "apple", quantity: 2});
    ///
    ///   match result {
    ///     ok(id) => "product-id-${id}",
    ///     err(error_msg) => "Error: ${error_msg}"
    ///   }
    /// ```
    ///
    /// Rib supports conditional calls, function calls, pattern-matching,
    /// string interpolation (see error_message above) etc.
    ///
    pub fn from_text(input: &str) -> Result<Expr, String> {
        if input.trim().ends_with(';') {
            return Err("unexpected `;` at the end of rib expression. \nnote: `;` is used to separate expressions, but it should not appear after the last expression (which is the return value)".to_string());
        }

        spaces()
            .with(block().skip(eof()))
            .easy_parse(position::Stream::new(input))
            .map(|t| t.0)
            .map_err(|err| format!("{err}"))
    }

    pub fn lookup(&self, source_span: &SourceSpan) -> Option<Expr> {
        let mut expr = self.clone();
        find_expr(&mut expr, source_span)
    }

    pub fn is_literal(&self) -> bool {
        matches!(self, Expr::Literal { .. })
    }

    pub fn is_block(&self) -> bool {
        matches!(self, Expr::ExprBlock { .. })
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Expr::Number { .. })
    }

    pub fn is_record(&self) -> bool {
        matches!(self, Expr::Record { .. })
    }

    pub fn is_result(&self) -> bool {
        matches!(self, Expr::Result { .. })
    }

    pub fn is_option(&self) -> bool {
        matches!(self, Expr::Option { .. })
    }

    pub fn is_tuple(&self) -> bool {
        matches!(self, Expr::Tuple { .. })
    }

    pub fn is_list(&self) -> bool {
        matches!(self, Expr::Sequence { .. })
    }

    pub fn is_flags(&self) -> bool {
        matches!(self, Expr::Flags { .. })
    }

    pub fn is_identifier(&self) -> bool {
        matches!(self, Expr::Identifier { .. })
    }

    pub fn is_select_field(&self) -> bool {
        matches!(self, Expr::SelectField { .. })
    }

    pub fn is_if_else(&self) -> bool {
        matches!(self, Expr::Cond { .. })
    }

    pub fn is_function_call(&self) -> bool {
        matches!(self, Expr::Call { .. })
    }

    pub fn is_match_expr(&self) -> bool {
        matches!(self, Expr::PatternMatch { .. })
    }

    pub fn is_boolean(&self) -> bool {
        matches!(self, Expr::Boolean { .. })
    }

    pub fn is_comparison(&self) -> bool {
        matches!(
            self,
            Expr::GreaterThan { .. }
                | Expr::GreaterThanOrEqualTo { .. }
                | Expr::LessThanOrEqualTo { .. }
                | Expr::EqualTo { .. }
                | Expr::LessThan { .. }
        )
    }

    pub fn is_concat(&self) -> bool {
        matches!(self, Expr::Concat { .. })
    }

    pub fn is_multiple(&self) -> bool {
        matches!(self, Expr::ExprBlock { .. })
    }

    pub fn inbuilt_variant(&self) -> Option<(String, Option<Expr>)> {
        match self {
            Expr::Option {
                expr: Some(expr), ..
            } => Some(("some".to_string(), Some(expr.deref().clone()))),
            Expr::Option { expr: None, .. } => Some(("some".to_string(), None)),
            Expr::Result { expr: Ok(expr), .. } => {
                Some(("ok".to_string(), Some(expr.deref().clone())))
            }
            Expr::Result {
                expr: Err(expr), ..
            } => Some(("err".to_string(), Some(expr.deref().clone()))),
            _ => None,
        }
    }
    pub fn unwrap(&self) -> Self {
        Expr::Unwrap {
            expr: Box::new(self.clone()),
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn length(expr: Expr) -> Self {
        Expr::Length {
            expr: Box::new(expr),
            inferred_type: InferredType::u64(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn boolean(value: bool) -> Self {
        Expr::Boolean {
            value,
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn and(left: Expr, right: Expr) -> Self {
        Expr::And {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn throw(message: impl AsRef<str>) -> Self {
        Expr::Throw {
            message: message.as_ref().to_string(),
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn generate_worker_name(variable_id: Option<VariableId>) -> Self {
        Expr::GenerateWorkerName {
            inferred_type: InferredType::string(),
            type_annotation: None,
            source_span: SourceSpan::default(),
            variable_id,
        }
    }

    pub fn plus(left: Expr, right: Expr) -> Self {
        Expr::Plus {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn minus(left: Expr, right: Expr) -> Self {
        Expr::Minus {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn divide(left: Expr, right: Expr) -> Self {
        Expr::Divide {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn multiply(left: Expr, right: Expr) -> Self {
        Expr::Multiply {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn and_combine(conditions: Vec<Expr>) -> Option<Expr> {
        let mut cond: Option<Expr> = None;

        for i in conditions {
            let left = Box::new(cond.clone().unwrap_or(Expr::boolean(true)));
            cond = Some(Expr::And {
                lhs: left,
                rhs: Box::new(i),
                inferred_type: InferredType::bool(),
                source_span: SourceSpan::default(),
                type_annotation: None,
            });
        }

        cond
    }

    pub fn call_worker_function(
        dynamic_parsed_fn_name: DynamicParsedFunctionName,
        generic_type_parameter: Option<GenericTypeParameter>,
        module_identifier: Option<InstanceIdentifier>,
        args: Vec<Expr>,
        component_info: Option<ComponentDependencyKey>,
    ) -> Self {
        Expr::Call {
            call_type: CallType::Function {
                function_name: dynamic_parsed_fn_name,
                instance_identifier: module_identifier.map(Box::new),
                component_info,
            },
            generic_type_parameter,
            args,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn call(
        call_type: CallType,
        generic_type_parameter: Option<GenericTypeParameter>,
        args: Vec<Expr>,
    ) -> Self {
        Expr::Call {
            call_type,
            generic_type_parameter,
            args,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn invoke_worker_function(
        lhs: Expr,
        function_name: String,
        generic_type_parameter: Option<GenericTypeParameter>,
        args: Vec<Expr>,
    ) -> Self {
        Expr::InvokeMethodLazy {
            lhs: Box::new(lhs),
            method: function_name,
            generic_type_parameter,
            args,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn concat(expressions: Vec<Expr>) -> Self {
        Expr::Concat {
            exprs: expressions,
            inferred_type: InferredType::string(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn cond(cond: Expr, lhs: Expr, rhs: Expr) -> Self {
        Expr::Cond {
            cond: Box::new(cond),
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn equal_to(left: Expr, right: Expr) -> Self {
        Expr::EqualTo {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn err(expr: Expr, type_annotation: Option<TypeName>) -> Self {
        let inferred_type = expr.inferred_type();
        Expr::Result {
            expr: Err(Box::new(expr)),
            type_annotation,
            inferred_type: InferredType::result(Some(InferredType::unknown()), Some(inferred_type)),
            source_span: SourceSpan::default(),
        }
    }

    pub fn flags(flags: Vec<String>) -> Self {
        Expr::Flags {
            flags: flags.clone(),
            inferred_type: InferredType::flags(flags),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn greater_than(left: Expr, right: Expr) -> Self {
        Expr::GreaterThan {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn greater_than_or_equal_to(left: Expr, right: Expr) -> Self {
        Expr::GreaterThanOrEqualTo {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    // An identifier by default is global until name-binding phase is run
    pub fn identifier_global(name: impl AsRef<str>, type_annotation: Option<TypeName>) -> Self {
        Expr::Identifier {
            variable_id: VariableId::global(name.as_ref().to_string()),
            type_annotation,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
        }
    }

    pub fn identifier_local(
        name: impl AsRef<str>,
        id: u32,
        type_annotation: Option<TypeName>,
    ) -> Self {
        Expr::Identifier {
            variable_id: VariableId::local(name.as_ref(), id),
            type_annotation,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
        }
    }

    pub fn identifier_with_variable_id(
        variable_id: VariableId,
        type_annotation: Option<TypeName>,
    ) -> Self {
        Expr::Identifier {
            variable_id,
            type_annotation,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
        }
    }

    pub fn less_than(left: Expr, right: Expr) -> Self {
        Expr::LessThan {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn less_than_or_equal_to(left: Expr, right: Expr) -> Self {
        Expr::LessThanOrEqualTo {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn range(from: Expr, to: Expr) -> Self {
        Expr::Range {
            range: Range::Range {
                from: Box::new(from.clone()),
                to: Box::new(to.clone()),
            },
            inferred_type: InferredType::range(from.inferred_type(), Some(to.inferred_type())),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn range_from(from: Expr) -> Self {
        Expr::Range {
            range: Range::RangeFrom {
                from: Box::new(from.clone()),
            },
            inferred_type: InferredType::range(from.inferred_type(), None),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn range_inclusive(from: Expr, to: Expr) -> Self {
        Expr::Range {
            range: Range::RangeInclusive {
                from: Box::new(from.clone()),
                to: Box::new(to.clone()),
            },
            inferred_type: InferredType::range(from.inferred_type(), Some(to.inferred_type())),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn let_binding(
        name: impl AsRef<str>,
        expr: Expr,
        type_annotation: Option<TypeName>,
    ) -> Self {
        Expr::Let {
            variable_id: VariableId::global(name.as_ref().to_string()),
            type_annotation,
            expr: Box::new(expr),
            source_span: SourceSpan::default(),
            inferred_type: InferredType::tuple(vec![]),
        }
    }

    pub fn let_binding_with_variable_id(
        variable_id: VariableId,
        expr: Expr,
        type_annotation: Option<TypeName>,
    ) -> Self {
        Expr::Let {
            variable_id,
            type_annotation,
            expr: Box::new(expr),
            source_span: SourceSpan::default(),
            inferred_type: InferredType::tuple(vec![]),
        }
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
            source_span: SourceSpan::default(),
            type_annotation: None,
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
            InferredType::unknown(),
        )
    }

    pub fn list_comprehension_typed(
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
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn list_comprehension(
        variable_id: VariableId,
        iterable_expr: Expr,
        yield_expr: Expr,
    ) -> Self {
        Expr::list_comprehension_typed(
            variable_id,
            iterable_expr,
            yield_expr,
            InferredType::list(InferredType::unknown()),
        )
    }

    pub fn bind_global_variable_types(&mut self, type_spec: &Vec<GlobalVariableTypeSpec>) {
        type_inference::bind_global_variable_types(self, type_spec)
    }

    pub fn bind_instance_types(&mut self) {
        type_inference::bind_instance_types(self)
    }

    pub fn literal(value: impl AsRef<str>) -> Self {
        let default_type = DefaultType::String;

        Expr::Literal {
            value: value.as_ref().to_string(),
            inferred_type: InferredType::from(&default_type),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn empty_expr() -> Self {
        Expr::literal("")
    }

    pub fn expr_block(expressions: Vec<Expr>) -> Self {
        let inferred_type = expressions
            .last()
            .map_or(InferredType::unknown(), |e| e.inferred_type());

        Expr::ExprBlock {
            exprs: expressions,
            inferred_type,
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn not(expr: Expr) -> Self {
        Expr::Not {
            expr: Box::new(expr),
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn ok(expr: Expr, type_annotation: Option<TypeName>) -> Self {
        let inferred_type = expr.inferred_type();

        Expr::Result {
            expr: Ok(Box::new(expr)),
            type_annotation,
            inferred_type: InferredType::result(Some(inferred_type), Some(InferredType::unknown())),
            source_span: SourceSpan::default(),
        }
    }

    pub fn option(expr: Option<Expr>) -> Self {
        let inferred_type = match &expr {
            Some(expr) => expr.inferred_type(),
            None => InferredType::unknown(),
        };

        Expr::Option {
            expr: expr.map(Box::new),
            type_annotation: None,
            inferred_type: InferredType::option(inferred_type),
            source_span: SourceSpan::default(),
        }
    }

    pub fn or(left: Expr, right: Expr) -> Self {
        Expr::Or {
            lhs: Box::new(left),
            rhs: Box::new(right),
            inferred_type: InferredType::bool(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn pattern_match(expr: Expr, match_arms: Vec<MatchArm>) -> Self {
        Expr::PatternMatch {
            predicate: Box::new(expr),
            match_arms,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn record(expressions: Vec<(String, Expr)>) -> Self {
        let inferred_type = InferredType::record(
            expressions
                .iter()
                .map(|(field_name, expr)| (field_name.to_string(), expr.inferred_type()))
                .collect(),
        );

        Expr::Record {
            exprs: expressions
                .into_iter()
                .map(|(field_name, expr)| (field_name, Box::new(expr)))
                .collect(),
            inferred_type,
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn select_field(
        expr: Expr,
        field: impl AsRef<str>,
        type_annotation: Option<TypeName>,
    ) -> Self {
        Expr::SelectField {
            expr: Box::new(expr),
            field: field.as_ref().to_string(),
            type_annotation,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
        }
    }

    pub fn select_index(expr: Expr, index: Expr) -> Self {
        Expr::SelectIndex {
            expr: Box::new(expr),
            index: Box::new(index),
            type_annotation: None,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
        }
    }

    pub fn get_tag(expr: Expr) -> Self {
        Expr::GetTag {
            expr: Box::new(expr),
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn tuple(expressions: Vec<Expr>) -> Self {
        let inferred_type = InferredType::tuple(
            expressions
                .iter()
                .map(|expr| expr.inferred_type())
                .collect(),
        );

        Expr::Tuple {
            exprs: expressions,
            inferred_type,
            source_span: SourceSpan::default(),
            type_annotation: None,
        }
    }

    pub fn sequence(expressions: Vec<Expr>, type_annotation: Option<TypeName>) -> Self {
        let inferred_type = InferredType::list(
            expressions
                .first()
                .map_or(InferredType::unknown(), |x| x.inferred_type()),
        );

        Expr::Sequence {
            exprs: expressions,
            type_annotation,
            inferred_type,
            source_span: SourceSpan::default(),
        }
    }

    pub fn inferred_type_mut(&mut self) -> &mut InferredType {
        match self {
            Expr::Let { inferred_type, .. }
            | Expr::SelectField { inferred_type, .. }
            | Expr::SelectIndex { inferred_type, .. }
            | Expr::Sequence { inferred_type, .. }
            | Expr::Record { inferred_type, .. }
            | Expr::Tuple { inferred_type, .. }
            | Expr::Literal { inferred_type, .. }
            | Expr::Number { inferred_type, .. }
            | Expr::Flags { inferred_type, .. }
            | Expr::Identifier { inferred_type, .. }
            | Expr::Boolean { inferred_type, .. }
            | Expr::Concat { inferred_type, .. }
            | Expr::ExprBlock { inferred_type, .. }
            | Expr::Not { inferred_type, .. }
            | Expr::GreaterThan { inferred_type, .. }
            | Expr::GreaterThanOrEqualTo { inferred_type, .. }
            | Expr::LessThanOrEqualTo { inferred_type, .. }
            | Expr::EqualTo { inferred_type, .. }
            | Expr::Plus { inferred_type, .. }
            | Expr::Minus { inferred_type, .. }
            | Expr::Divide { inferred_type, .. }
            | Expr::Multiply { inferred_type, .. }
            | Expr::LessThan { inferred_type, .. }
            | Expr::Cond { inferred_type, .. }
            | Expr::PatternMatch { inferred_type, .. }
            | Expr::Option { inferred_type, .. }
            | Expr::Result { inferred_type, .. }
            | Expr::Unwrap { inferred_type, .. }
            | Expr::Throw { inferred_type, .. }
            | Expr::GetTag { inferred_type, .. }
            | Expr::And { inferred_type, .. }
            | Expr::Or { inferred_type, .. }
            | Expr::ListComprehension { inferred_type, .. }
            | Expr::ListReduce { inferred_type, .. }
            | Expr::Call { inferred_type, .. }
            | Expr::Range { inferred_type, .. }
            | Expr::InvokeMethodLazy { inferred_type, .. }
            | Expr::Length { inferred_type, .. }
            | Expr::GenerateWorkerName { inferred_type, .. } => &mut *inferred_type,
        }
    }

    pub fn inferred_type(&self) -> InferredType {
        match self {
            Expr::Let { inferred_type, .. }
            | Expr::SelectField { inferred_type, .. }
            | Expr::SelectIndex { inferred_type, .. }
            | Expr::Sequence { inferred_type, .. }
            | Expr::Record { inferred_type, .. }
            | Expr::Tuple { inferred_type, .. }
            | Expr::Literal { inferred_type, .. }
            | Expr::Number { inferred_type, .. }
            | Expr::Flags { inferred_type, .. }
            | Expr::Identifier { inferred_type, .. }
            | Expr::Boolean { inferred_type, .. }
            | Expr::Concat { inferred_type, .. }
            | Expr::ExprBlock { inferred_type, .. }
            | Expr::Not { inferred_type, .. }
            | Expr::GreaterThan { inferred_type, .. }
            | Expr::GreaterThanOrEqualTo { inferred_type, .. }
            | Expr::LessThanOrEqualTo { inferred_type, .. }
            | Expr::EqualTo { inferred_type, .. }
            | Expr::Plus { inferred_type, .. }
            | Expr::Minus { inferred_type, .. }
            | Expr::Divide { inferred_type, .. }
            | Expr::Multiply { inferred_type, .. }
            | Expr::LessThan { inferred_type, .. }
            | Expr::Cond { inferred_type, .. }
            | Expr::PatternMatch { inferred_type, .. }
            | Expr::Option { inferred_type, .. }
            | Expr::Result { inferred_type, .. }
            | Expr::Unwrap { inferred_type, .. }
            | Expr::Throw { inferred_type, .. }
            | Expr::GetTag { inferred_type, .. }
            | Expr::And { inferred_type, .. }
            | Expr::Or { inferred_type, .. }
            | Expr::ListComprehension { inferred_type, .. }
            | Expr::ListReduce { inferred_type, .. }
            | Expr::Call { inferred_type, .. }
            | Expr::Range { inferred_type, .. }
            | Expr::InvokeMethodLazy { inferred_type, .. }
            | Expr::Length { inferred_type, .. }
            | Expr::GenerateWorkerName { inferred_type, .. } => inferred_type.clone(),
        }
    }

    pub fn infer_types(
        &mut self,
        component_dependency: &ComponentDependencies,
        type_spec: &Vec<GlobalVariableTypeSpec>,
    ) -> Result<(), RibTypeErrorInternal> {
        self.infer_types_initial_phase(component_dependency, type_spec)?;
        self.bind_instance_types();
        // Identifying the first fix point with method calls to infer all
        // worker function invocations as this forms the foundation for the rest of the
        // compilation. This is compiler doing its best to infer all the calls such
        // as worker invokes or instance calls etc.
        type_inference::type_inference_fix_point(Self::resolve_method_calls, self)?;
        self.infer_function_call_types(component_dependency)?;
        type_inference::type_inference_fix_point(
            |x| Self::inference_scan(x, component_dependency),
            self,
        )?;
        self.check_types(component_dependency)?;
        self.unify_types()?;
        Ok(())
    }

    pub fn infer_types_initial_phase(
        &mut self,
        component_dependency: &ComponentDependencies,
        type_spec: &Vec<GlobalVariableTypeSpec>,
    ) -> Result<(), RibTypeErrorInternal> {
        self.set_origin();
        self.bind_global_variable_types(type_spec);
        self.bind_type_annotations();
        self.bind_variables_of_list_comprehension();
        self.bind_variables_of_list_reduce();
        self.bind_variables_of_pattern_match();
        self.bind_variables_of_let_assignment();
        self.identify_instance_creation(component_dependency)?;
        self.ensure_stateful_instance();
        self.infer_variants(component_dependency);
        self.infer_enums(component_dependency);
        Ok(())
    }

    pub fn resolve_method_calls(&mut self) -> Result<(), RibTypeErrorInternal> {
        self.bind_instance_types();
        self.infer_worker_function_invokes()?;
        Ok(())
    }

    pub fn set_origin(&mut self) {
        let mut visitor = ExprVisitor::bottom_up(self);

        while let Some(expr) = visitor.pop_front() {
            let source_location = expr.source_span();
            let origin = TypeOrigin::OriginatedAt(source_location.clone());
            let inferred_type = expr.inferred_type();
            let origin = inferred_type.add_origin(origin);
            expr.with_inferred_type_mut(origin);
        }
    }

    // An inference is a single cycle of to-and-fro scanning of Rib expression, that it takes part in fix point of inference.
    // Not all phases of compilation will be part of this scan.
    // Example: function call argument inference based on the worker function hardly needs to be part of the scan.
    pub fn inference_scan(
        &mut self,
        component_dependencies: &ComponentDependencies,
    ) -> Result<(), RibTypeErrorInternal> {
        self.infer_all_identifiers();
        self.push_types_down()?;
        self.infer_all_identifiers();
        self.pull_types_up(component_dependencies)?;
        self.infer_global_inputs();
        self.infer_function_call_types(component_dependencies)?;
        Ok(())
    }

    pub fn infer_worker_function_invokes(&mut self) -> Result<(), RibTypeErrorInternal> {
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
        component_dependency: &ComponentDependencies,
    ) -> Result<(), RibTypeErrorInternal> {
        type_inference::identify_instance_creation(self, component_dependency)
    }

    pub fn ensure_stateful_instance(&mut self) {
        type_inference::ensure_stateful_instance(self)
    }

    pub fn infer_function_call_types(
        &mut self,
        component_dependency: &ComponentDependencies,
    ) -> Result<(), RibTypeErrorInternal> {
        type_inference::infer_function_call_types(self, component_dependency)?;
        Ok(())
    }

    pub fn push_types_down(&mut self) -> Result<(), RibTypeErrorInternal> {
        type_inference::push_types_down(self)
    }

    pub fn infer_all_identifiers(&mut self) {
        type_inference::infer_all_identifiers(self)
    }

    pub fn pull_types_up(
        &mut self,
        component_dependencies: &ComponentDependencies,
    ) -> Result<(), RibTypeErrorInternal> {
        type_inference::type_pull_up(self, component_dependencies)
    }

    pub fn infer_global_inputs(&mut self) {
        type_inference::infer_global_inputs(self);
    }

    pub fn bind_type_annotations(&mut self) {
        type_inference::bind_type_annotations(self);
    }

    pub fn check_types(
        &mut self,
        component_dependency: &ComponentDependencies,
    ) -> Result<(), RibTypeErrorInternal> {
        type_checker::type_check(self, component_dependency)
    }

    pub fn unify_types(&mut self) -> Result<(), RibTypeErrorInternal> {
        type_inference::unify_types(self)?;
        Ok(())
    }

    pub fn merge_inferred_type(&self, new_inferred_type: InferredType) -> Expr {
        let mut expr_copied = self.clone();
        expr_copied.add_infer_type_mut(new_inferred_type);
        expr_copied
    }

    pub fn add_infer_type_mut(&mut self, new_inferred_type: InferredType) {
        match self {
            Expr::Identifier { inferred_type, .. }
            | Expr::Let { inferred_type, .. }
            | Expr::SelectField { inferred_type, .. }
            | Expr::SelectIndex { inferred_type, .. }
            | Expr::Sequence { inferred_type, .. }
            | Expr::Record { inferred_type, .. }
            | Expr::Tuple { inferred_type, .. }
            | Expr::Literal { inferred_type, .. }
            | Expr::Number { inferred_type, .. }
            | Expr::Flags { inferred_type, .. }
            | Expr::Boolean { inferred_type, .. }
            | Expr::Concat { inferred_type, .. }
            | Expr::ExprBlock { inferred_type, .. }
            | Expr::Not { inferred_type, .. }
            | Expr::GreaterThan { inferred_type, .. }
            | Expr::GreaterThanOrEqualTo { inferred_type, .. }
            | Expr::LessThanOrEqualTo { inferred_type, .. }
            | Expr::EqualTo { inferred_type, .. }
            | Expr::Plus { inferred_type, .. }
            | Expr::Minus { inferred_type, .. }
            | Expr::Divide { inferred_type, .. }
            | Expr::Multiply { inferred_type, .. }
            | Expr::LessThan { inferred_type, .. }
            | Expr::Cond { inferred_type, .. }
            | Expr::PatternMatch { inferred_type, .. }
            | Expr::Option { inferred_type, .. }
            | Expr::Result { inferred_type, .. }
            | Expr::Unwrap { inferred_type, .. }
            | Expr::Throw { inferred_type, .. }
            | Expr::GetTag { inferred_type, .. }
            | Expr::And { inferred_type, .. }
            | Expr::Or { inferred_type, .. }
            | Expr::ListComprehension { inferred_type, .. }
            | Expr::ListReduce { inferred_type, .. }
            | Expr::InvokeMethodLazy { inferred_type, .. }
            | Expr::Range { inferred_type, .. }
            | Expr::Length { inferred_type, .. }
            | Expr::GenerateWorkerName { inferred_type, .. }
            | Expr::Call { inferred_type, .. } => {
                if !new_inferred_type.is_unknown() {
                    *inferred_type = inferred_type.merge(new_inferred_type);
                }
            }
        }
    }

    pub fn reset_type(&mut self) {
        type_inference::reset_type_info(self);
    }

    pub fn source_span(&self) -> SourceSpan {
        match self {
            Expr::Identifier { source_span, .. }
            | Expr::Let { source_span, .. }
            | Expr::SelectField { source_span, .. }
            | Expr::SelectIndex { source_span, .. }
            | Expr::Sequence { source_span, .. }
            | Expr::Record { source_span, .. }
            | Expr::Tuple { source_span, .. }
            | Expr::Literal { source_span, .. }
            | Expr::Number { source_span, .. }
            | Expr::Flags { source_span, .. }
            | Expr::Boolean { source_span, .. }
            | Expr::Concat { source_span, .. }
            | Expr::ExprBlock { source_span, .. }
            | Expr::Not { source_span, .. }
            | Expr::GreaterThan { source_span, .. }
            | Expr::GreaterThanOrEqualTo { source_span, .. }
            | Expr::LessThanOrEqualTo { source_span, .. }
            | Expr::EqualTo { source_span, .. }
            | Expr::LessThan { source_span, .. }
            | Expr::Plus { source_span, .. }
            | Expr::Minus { source_span, .. }
            | Expr::Divide { source_span, .. }
            | Expr::Multiply { source_span, .. }
            | Expr::Cond { source_span, .. }
            | Expr::PatternMatch { source_span, .. }
            | Expr::Option { source_span, .. }
            | Expr::Result { source_span, .. }
            | Expr::Unwrap { source_span, .. }
            | Expr::Throw { source_span, .. }
            | Expr::And { source_span, .. }
            | Expr::Or { source_span, .. }
            | Expr::GetTag { source_span, .. }
            | Expr::ListComprehension { source_span, .. }
            | Expr::ListReduce { source_span, .. }
            | Expr::InvokeMethodLazy { source_span, .. }
            | Expr::Range { source_span, .. }
            | Expr::Length { source_span, .. }
            | Expr::Call { source_span, .. }
            | Expr::GenerateWorkerName { source_span, .. } => source_span.clone(),
        }
    }

    pub fn type_annotation(&self) -> &Option<TypeName> {
        match self {
            Expr::Identifier {
                type_annotation, ..
            }
            | Expr::Let {
                type_annotation, ..
            }
            | Expr::SelectField {
                type_annotation, ..
            }
            | Expr::SelectIndex {
                type_annotation, ..
            }
            | Expr::Sequence {
                type_annotation, ..
            }
            | Expr::Record {
                type_annotation, ..
            }
            | Expr::Tuple {
                type_annotation, ..
            }
            | Expr::Literal {
                type_annotation, ..
            }
            | Expr::Number {
                type_annotation, ..
            }
            | Expr::Flags {
                type_annotation, ..
            }
            | Expr::Boolean {
                type_annotation, ..
            }
            | Expr::Concat {
                type_annotation, ..
            }
            | Expr::ExprBlock {
                type_annotation, ..
            }
            | Expr::Not {
                type_annotation, ..
            }
            | Expr::GreaterThan {
                type_annotation, ..
            }
            | Expr::GreaterThanOrEqualTo {
                type_annotation, ..
            }
            | Expr::LessThanOrEqualTo {
                type_annotation, ..
            }
            | Expr::EqualTo {
                type_annotation, ..
            }
            | Expr::LessThan {
                type_annotation, ..
            }
            | Expr::Plus {
                type_annotation, ..
            }
            | Expr::Minus {
                type_annotation, ..
            }
            | Expr::Divide {
                type_annotation, ..
            }
            | Expr::Multiply {
                type_annotation, ..
            }
            | Expr::Cond {
                type_annotation, ..
            }
            | Expr::PatternMatch {
                type_annotation, ..
            }
            | Expr::Option {
                type_annotation, ..
            }
            | Expr::Result {
                type_annotation, ..
            }
            | Expr::Unwrap {
                type_annotation, ..
            }
            | Expr::Throw {
                type_annotation, ..
            }
            | Expr::And {
                type_annotation, ..
            }
            | Expr::Or {
                type_annotation, ..
            }
            | Expr::GetTag {
                type_annotation, ..
            }
            | Expr::ListComprehension {
                type_annotation, ..
            }
            | Expr::ListReduce {
                type_annotation, ..
            }
            | Expr::InvokeMethodLazy {
                type_annotation, ..
            }
            | Expr::Range {
                type_annotation, ..
            }
            | Expr::Length {
                type_annotation, ..
            }
            | Expr::GenerateWorkerName {
                type_annotation, ..
            }
            | Expr::Call {
                type_annotation, ..
            } => type_annotation,
        }
    }

    pub fn with_type_annotation_opt(&self, type_annotation: Option<TypeName>) -> Expr {
        if let Some(type_annotation) = type_annotation {
            self.with_type_annotation(type_annotation)
        } else {
            self.clone()
        }
    }

    pub fn with_type_annotation(&self, type_annotation: TypeName) -> Expr {
        let mut expr_copied = self.clone();
        expr_copied.with_type_annotation_mut(type_annotation);
        expr_copied
    }

    pub fn with_type_annotation_mut(&mut self, type_annotation: TypeName) {
        let new_type_annotation = type_annotation;

        match self {
            Expr::Identifier {
                type_annotation, ..
            }
            | Expr::Let {
                type_annotation, ..
            }
            | Expr::SelectField {
                type_annotation, ..
            }
            | Expr::SelectIndex {
                type_annotation, ..
            }
            | Expr::Sequence {
                type_annotation, ..
            }
            | Expr::Record {
                type_annotation, ..
            }
            | Expr::Tuple {
                type_annotation, ..
            }
            | Expr::Literal {
                type_annotation, ..
            }
            | Expr::Number {
                type_annotation, ..
            }
            | Expr::Flags {
                type_annotation, ..
            }
            | Expr::Boolean {
                type_annotation, ..
            }
            | Expr::Concat {
                type_annotation, ..
            }
            | Expr::ExprBlock {
                type_annotation, ..
            }
            | Expr::Not {
                type_annotation, ..
            }
            | Expr::GreaterThan {
                type_annotation, ..
            }
            | Expr::GreaterThanOrEqualTo {
                type_annotation, ..
            }
            | Expr::LessThanOrEqualTo {
                type_annotation, ..
            }
            | Expr::EqualTo {
                type_annotation, ..
            }
            | Expr::LessThan {
                type_annotation, ..
            }
            | Expr::Plus {
                type_annotation, ..
            }
            | Expr::Minus {
                type_annotation, ..
            }
            | Expr::Divide {
                type_annotation, ..
            }
            | Expr::Multiply {
                type_annotation, ..
            }
            | Expr::Cond {
                type_annotation, ..
            }
            | Expr::PatternMatch {
                type_annotation, ..
            }
            | Expr::Option {
                type_annotation, ..
            }
            | Expr::Result {
                type_annotation, ..
            }
            | Expr::Unwrap {
                type_annotation, ..
            }
            | Expr::Throw {
                type_annotation, ..
            }
            | Expr::And {
                type_annotation, ..
            }
            | Expr::Or {
                type_annotation, ..
            }
            | Expr::GetTag {
                type_annotation, ..
            }
            | Expr::Range {
                type_annotation, ..
            }
            | Expr::ListComprehension {
                type_annotation, ..
            }
            | Expr::ListReduce {
                type_annotation, ..
            }
            | Expr::InvokeMethodLazy {
                type_annotation, ..
            }
            | Expr::Length {
                type_annotation, ..
            }
            | Expr::GenerateWorkerName {
                type_annotation, ..
            }
            | Expr::Call {
                type_annotation, ..
            } => {
                *type_annotation = Some(new_type_annotation);
            }
        }
    }

    pub fn with_source_span(&self, new_source_span: SourceSpan) -> Expr {
        let mut expr_copied = self.clone();
        expr_copied.with_source_span_mut(new_source_span);
        expr_copied
    }

    pub fn with_source_span_mut(&mut self, new_source_span: SourceSpan) {
        match self {
            Expr::Identifier { source_span, .. }
            | Expr::Let { source_span, .. }
            | Expr::SelectField { source_span, .. }
            | Expr::SelectIndex { source_span, .. }
            | Expr::Sequence { source_span, .. }
            | Expr::Number { source_span, .. }
            | Expr::Record { source_span, .. }
            | Expr::Tuple { source_span, .. }
            | Expr::Literal { source_span, .. }
            | Expr::Flags { source_span, .. }
            | Expr::Boolean { source_span, .. }
            | Expr::Concat { source_span, .. }
            | Expr::ExprBlock { source_span, .. }
            | Expr::Not { source_span, .. }
            | Expr::GreaterThan { source_span, .. }
            | Expr::GreaterThanOrEqualTo { source_span, .. }
            | Expr::LessThanOrEqualTo { source_span, .. }
            | Expr::EqualTo { source_span, .. }
            | Expr::LessThan { source_span, .. }
            | Expr::Plus { source_span, .. }
            | Expr::Minus { source_span, .. }
            | Expr::Divide { source_span, .. }
            | Expr::Multiply { source_span, .. }
            | Expr::Cond { source_span, .. }
            | Expr::PatternMatch { source_span, .. }
            | Expr::Option { source_span, .. }
            | Expr::Result { source_span, .. }
            | Expr::Unwrap { source_span, .. }
            | Expr::Throw { source_span, .. }
            | Expr::And { source_span, .. }
            | Expr::Or { source_span, .. }
            | Expr::GetTag { source_span, .. }
            | Expr::Range { source_span, .. }
            | Expr::ListComprehension { source_span, .. }
            | Expr::ListReduce { source_span, .. }
            | Expr::InvokeMethodLazy { source_span, .. }
            | Expr::Length { source_span, .. }
            | Expr::GenerateWorkerName { source_span, .. }
            | Expr::Call { source_span, .. } => {
                *source_span = new_source_span;
            }
        }
    }

    pub fn with_inferred_type(&self, new_inferred_type: InferredType) -> Expr {
        let mut expr_copied = self.clone();
        expr_copied.with_inferred_type_mut(new_inferred_type);
        expr_copied
    }

    // `with_inferred_type` overrides the existing inferred_type and returns a new expr
    // This is different to `merge_inferred_type` where it tries to combine the new inferred type with the existing one.
    pub fn with_inferred_type_mut(&mut self, new_inferred_type: InferredType) {
        match self {
            Expr::Identifier { inferred_type, .. }
            | Expr::Let { inferred_type, .. }
            | Expr::SelectField { inferred_type, .. }
            | Expr::SelectIndex { inferred_type, .. }
            | Expr::Sequence { inferred_type, .. }
            | Expr::Record { inferred_type, .. }
            | Expr::Tuple { inferred_type, .. }
            | Expr::Literal { inferred_type, .. }
            | Expr::Number { inferred_type, .. }
            | Expr::Flags { inferred_type, .. }
            | Expr::Boolean { inferred_type, .. }
            | Expr::Concat { inferred_type, .. }
            | Expr::ExprBlock { inferred_type, .. }
            | Expr::Not { inferred_type, .. }
            | Expr::GreaterThan { inferred_type, .. }
            | Expr::GreaterThanOrEqualTo { inferred_type, .. }
            | Expr::LessThanOrEqualTo { inferred_type, .. }
            | Expr::EqualTo { inferred_type, .. }
            | Expr::LessThan { inferred_type, .. }
            | Expr::Plus { inferred_type, .. }
            | Expr::Minus { inferred_type, .. }
            | Expr::Divide { inferred_type, .. }
            | Expr::Multiply { inferred_type, .. }
            | Expr::Cond { inferred_type, .. }
            | Expr::PatternMatch { inferred_type, .. }
            | Expr::Option { inferred_type, .. }
            | Expr::Result { inferred_type, .. }
            | Expr::Unwrap { inferred_type, .. }
            | Expr::Throw { inferred_type, .. }
            | Expr::And { inferred_type, .. }
            | Expr::Or { inferred_type, .. }
            | Expr::GetTag { inferred_type, .. }
            | Expr::ListComprehension { inferred_type, .. }
            | Expr::ListReduce { inferred_type, .. }
            | Expr::InvokeMethodLazy { inferred_type, .. }
            | Expr::Range { inferred_type, .. }
            | Expr::Length { inferred_type, .. }
            | Expr::GenerateWorkerName { inferred_type, .. }
            | Expr::Call { inferred_type, .. } => {
                *inferred_type = new_inferred_type;
            }
        }
    }

    pub fn infer_enums(&mut self, component_dependency: &ComponentDependencies) {
        type_inference::infer_enums(self, component_dependency);
    }

    pub fn infer_variants(&mut self, component_dependency: &ComponentDependencies) {
        type_inference::infer_variants(self, component_dependency);
    }

    pub fn visit_expr_nodes_lazy<'a>(&'a mut self, queue: &mut VecDeque<&'a mut Expr>) {
        type_inference::visit_expr_nodes_lazy(self, queue);
    }

    pub fn number_inferred(
        big_decimal: BigDecimal,
        type_annotation: Option<TypeName>,
        inferred_type: InferredType,
    ) -> Expr {
        Expr::Number {
            number: Number { value: big_decimal },
            type_annotation,
            inferred_type,
            source_span: SourceSpan::default(),
        }
    }

    pub fn number(big_decimal: BigDecimal) -> Expr {
        let default_type = DefaultType::from(&big_decimal);
        let inferred_type = InferredType::from(&default_type);

        Expr::number_inferred(big_decimal, None, inferred_type)
    }
}

#[derive(Debug, Hash, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Range {
    Range { from: Box<Expr>, to: Box<Expr> },
    RangeInclusive { from: Box<Expr>, to: Box<Expr> },
    RangeFrom { from: Box<Expr> },
}

impl Range {
    pub fn from(&self) -> Option<&Expr> {
        match self {
            Range::Range { from, .. } => Some(from),
            Range::RangeInclusive { from, .. } => Some(from),
            Range::RangeFrom { from } => Some(from),
        }
    }

    pub fn to(&self) -> Option<&Expr> {
        match self {
            Range::Range { to, .. } => Some(to),
            Range::RangeInclusive { to, .. } => Some(to),
            Range::RangeFrom { .. } => None,
        }
    }

    pub fn inclusive(&self) -> bool {
        matches!(self, Range::RangeInclusive { .. })
    }

    pub fn get_exprs_mut(&mut self) -> Vec<&mut Box<Expr>> {
        match self {
            Range::Range { from, to } => vec![from, to],
            Range::RangeInclusive { from, to } => vec![from, to],
            Range::RangeFrom { from } => vec![from],
        }
    }

    pub fn get_exprs(&self) -> Vec<&Expr> {
        match self {
            Range::Range { from, to } => vec![from.as_ref(), to.as_ref()],
            Range::RangeInclusive { from, to } => vec![from.as_ref(), to.as_ref()],
            Range::RangeFrom { from } => vec![from.as_ref()],
        }
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
        ArmPattern::Literal(Box::new(Expr::Result {
            expr: Ok(Box::new(Expr::Identifier {
                variable_id: VariableId::global(binding_variable.to_string()),
                type_annotation: None,
                inferred_type: InferredType::unknown(),
                source_span: SourceSpan::default(),
            })),
            type_annotation: None,
            inferred_type: InferredType::result(
                Some(InferredType::unknown()),
                Some(InferredType::unknown()),
            ),
            source_span: SourceSpan::default(),
        }))
    }

    // Helper to construct err(v). Cannot be used if there is nested constructors such as err(some(v)))
    pub fn err(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Result {
            expr: Err(Box::new(Expr::Identifier {
                variable_id: VariableId::global(binding_variable.to_string()),
                type_annotation: None,
                inferred_type: InferredType::unknown(),
                source_span: SourceSpan::default(),
            })),
            type_annotation: None,
            inferred_type: InferredType::result(
                Some(InferredType::unknown()),
                Some(InferredType::unknown()),
            ),
            source_span: SourceSpan::default(),
        }))
    }

    // Helper to construct some(v). Cannot be used if there is nested constructors such as some(ok(v)))
    pub fn some(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option {
            expr: Some(Box::new(Expr::Identifier {
                variable_id: VariableId::local_with_no_id(binding_variable),
                type_annotation: None,
                inferred_type: InferredType::unknown(),
                source_span: SourceSpan::default(),
            })),
            type_annotation: None,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
        }))
    }

    pub fn none() -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option {
            expr: None,
            type_annotation: None,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
        }))
    }

    pub fn identifier(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Identifier {
            variable_id: VariableId::global(binding_variable.to_string()),
            type_annotation: None,
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
        }))
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
                let type_annotation = expr.type_name.map(TypeName::try_from).transpose()?;
                let expr_: golem_api_grpc::proto::golem::rib::Expr =
                    *expr.expr.ok_or("Missing expr")?;
                let expr: Expr = expr_.try_into()?;
                Expr::let_binding(name, expr, type_annotation)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndexV1(expr) => {
                let selection = *expr.expr.ok_or("Missing expr")?;
                let field = *expr.index.ok_or("Missing index")?;
                let type_annotation = expr.type_name.map(TypeName::try_from).transpose()?;

                Expr::select_index(selection.try_into()?, field.try_into()?)
                    .with_type_annotation_opt(type_annotation)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Length(expr) => {
                let expr = expr.expr.ok_or("Missing expr")?;
                Expr::Length {
                    expr: Box::new((*expr).try_into()?),
                    type_annotation: None,
                    inferred_type: InferredType::unknown(),
                    source_span: SourceSpan::default(),
                }
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Range(range) => {
                let range_expr = range.range_expr.ok_or("Missing range expr")?;

                match range_expr {
                    RangeExpr::RangeFrom(range_from) => {
                        let from = range_from.from.ok_or("Missing from expr")?;
                        Expr::range_from((*from).try_into()?)
                    }
                    RangeExpr::Range(range) => {
                        let from = range.from.ok_or("Missing from expr")?;
                        let to = range.to.ok_or("Missing to expr")?;
                        Expr::range((*from).try_into()?, (*to).try_into()?)
                    }
                    RangeExpr::RangeInclusive(range_inclusive) => {
                        let from = range_inclusive.from.ok_or("Missing from expr")?;
                        let to = range_inclusive.to.ok_or("Missing to expr")?;
                        Expr::range_inclusive((*from).try_into()?, (*to).try_into()?)
                    }
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
                let type_annotation = type_name.map(TypeName::try_from).transpose()?;

                let exprs: Vec<Expr> = exprs
                    .into_iter()
                    .map(|expr| expr.try_into())
                    .collect::<Result<Vec<_>, _>>()?;
                Expr::sequence(exprs, type_annotation)
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

                Expr::identifier_global(name.as_str(), type_name)
            }

            golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
                golem_api_grpc::proto::golem::rib::BooleanExpr { value },
            ) => Expr::boolean(value),

            golem_api_grpc::proto::golem::rib::expr::Expr::Throw(
                golem_api_grpc::proto::golem::rib::ThrowExpr { message },
            ) => Expr::throw(message),

            golem_api_grpc::proto::golem::rib::expr::Expr::GenerateWorkerName(
                golem_api_grpc::proto::golem::rib::GenerateWorkerNameExpr {},
            ) => Expr::generate_worker_name(None),

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

                Expr::number(big_decimal).with_type_annotation_opt(type_name)
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

                let index_expr =
                    Expr::number(BigDecimal::from_usize(index).ok_or("Invalid index")?);

                Expr::select_index(expr.try_into()?, index_expr).with_type_annotation_opt(type_name)
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::Option(expr) => {
                let type_name = expr.type_name;
                let type_name = type_name.map(TypeName::try_from).transpose()?;

                match expr.expr {
                    Some(expr) => {
                        Expr::option(Some((*expr).try_into()?)).with_type_annotation_opt(type_name)
                    }
                    None => Expr::option(None).with_type_annotation_opt(type_name),
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
                let generic_type_parameter = expr
                    .generic_type_parameter
                    .map(|tp| GenericTypeParameter { value: tp });

                match (legacy_invocation_name, call_type) {
                    (Some(legacy), None) => {
                        let name = legacy.name.ok_or("Missing function call name")?;
                        match name {
                            golem_api_grpc::proto::golem::rib::invocation_name::Name::Parsed(name) => {
                                // Reading the previous parsed-function-name in persistent store as a dynamic-parsed-function-name
                                Expr::call_worker_function(DynamicParsedFunctionName::parse(
                                    ParsedFunctionName::try_from(name)?.to_string()
                                )?, generic_type_parameter, None, params, None)
                            }
                            golem_api_grpc::proto::golem::rib::invocation_name::Name::VariantConstructor(
                                name,
                            ) => Expr::call_worker_function(DynamicParsedFunctionName::parse(name)?, generic_type_parameter, None, params, None),
                            golem_api_grpc::proto::golem::rib::invocation_name::Name::EnumConstructor(
                                name,
                            ) => Expr::call_worker_function(DynamicParsedFunctionName::parse(name)?, generic_type_parameter, None, params, None),
                        }
                    }
                    (_, Some(call_type)) => {
                        let name = call_type.name.ok_or("Missing function call name")?;
                        match name {
                            golem_api_grpc::proto::golem::rib::call_type::Name::Parsed(name) => {
                                Expr::call_worker_function(name.try_into()?, generic_type_parameter, None, params, None)
                            }
                            golem_api_grpc::proto::golem::rib::call_type::Name::VariantConstructor(
                                name,
                            ) => Expr::call_worker_function(DynamicParsedFunctionName::parse(name)?, generic_type_parameter, None, params, None),
                            golem_api_grpc::proto::golem::rib::call_type::Name::EnumConstructor(
                                name,
                            ) => Expr::call_worker_function(DynamicParsedFunctionName::parse(name)?, generic_type_parameter, None, params, None),
                            golem_api_grpc::proto::golem::rib::call_type::Name::InstanceCreation(instance_creation) => {
                                let instance_creation_type = InstanceCreationType::try_from(*instance_creation)?;
                                let call_type = CallType::InstanceCreation(instance_creation_type);
                                Expr::Call {
                                    call_type,
                                    generic_type_parameter,
                                    args: vec![],
                                    inferred_type: InferredType::unknown(),
                                    source_span: SourceSpan::default(),
                                    type_annotation: None, // TODO
                                }
                            }
                        }
                    }
                    (_, _) => Err("Missing both call type (and legacy invocation type)")?,
                }
            }
            golem_api_grpc::proto::golem::rib::expr::Expr::LazyInvokeMethod(lazy_invoke) => {
                let lhs_proto = lazy_invoke.lhs.ok_or("Missing lhs")?;
                let lhs = Box::new((*lhs_proto).try_into()?);
                let method = lazy_invoke.method;
                let generic_type_parameter = lazy_invoke.generic_type_parameter;
                let args: Vec<Expr> = lazy_invoke
                    .args
                    .into_iter()
                    .map(Expr::try_from)
                    .collect::<Result<Vec<_>, _>>()?;

                Expr::InvokeMethodLazy {
                    lhs,
                    method,
                    generic_type_parameter: generic_type_parameter
                        .map(|value| GenericTypeParameter { value }),
                    args,
                    inferred_type: InferredType::unknown(),
                    source_span: SourceSpan::default(),
                    type_annotation: None, //TODO
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
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(expr_string) => match from_string(expr_string.as_str()) {
                Ok(expr) => Ok(expr),
                Err(message) => Err(serde::de::Error::custom(message.to_string())),
            },

            e => Err(serde::de::Error::custom(format!(
                "Failed to deserialize expression {e}"
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
            Ok(value) => Value::serialize(&Value::String(value), serializer),
            Err(error) => Err(serde::ser::Error::custom(error.to_string())),
        }
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::{ArmPattern, Expr, MatchArm, Range};
    use golem_api_grpc::proto::golem::rib::range_expr::RangeExpr;

    // It is to be noted that when we change `Expr` tree solely for the purpose of
    // updating type inference, we don't need to change the proto version
    // of Expr. A proto version of Expr changes only when Rib adds/updates the grammar itself.
    // This makes it easy to keep backward compatibility at the persistence level
    // (if persistence using grpc encode)
    //
    // Reason: A proto version of Expr doesn't take into the account the type inferred
    // for each expr, or encode any behaviour that's the result of a type inference
    // Example: in a type inference, a variable-id of expr which is tagged as `global` becomes
    // `VariableId::Local(Identifier)` after a particular type inference phase, however, when encoding
    // we don't need to consider this `Identifier` and is kept as global (i.e, the raw form) in Expr.
    // This is because we never (want to) encode an Expr which is a result of `infer_types` function.
    //
    // Summary: We encode Expr only prior to compilation. After compilation, we encode only the RibByteCode.
    // If we ever want to encode Expr after type-inference phase, it implies, we need to encode `InferredExpr`
    // rather than `Expr` and this will ensure, users when retrieving back the `Expr` will never have
    // noise regarding types and variable-ids, and will always stay one to one in round trip
    impl From<Expr> for golem_api_grpc::proto::golem::rib::Expr {
        fn from(value: Expr) -> Self {
            let expr = match value {
                Expr::GenerateWorkerName { .. } => Some(
                    golem_api_grpc::proto::golem::rib::expr::Expr::GenerateWorkerName(
                        golem_api_grpc::proto::golem::rib::GenerateWorkerNameExpr {},
                    ),
                ),
                Expr::Let {
                    variable_id,
                    type_annotation,
                    expr,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Let(
                    Box::new(golem_api_grpc::proto::golem::rib::LetExpr {
                        name: variable_id.name().to_string(),
                        expr: Some(Box::new((*expr).into())),
                        type_name: type_annotation.map(|t| t.into()),
                    }),
                )),

                Expr::Length { expr, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Length(
                        Box::new(golem_api_grpc::proto::golem::rib::LengthExpr {
                            expr: Some(Box::new((*expr).into())),
                        }),
                    ))
                }

                Expr::SelectField {
                    expr,
                    field,
                    type_annotation,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(
                    Box::new(golem_api_grpc::proto::golem::rib::SelectFieldExpr {
                        expr: Some(Box::new((*expr).into())),
                        field,
                        type_name: type_annotation.map(|t| t.into()),
                    }),
                )),

                Expr::Range { range, .. } => match range {
                    Range::RangeFrom { from } => {
                        Some(golem_api_grpc::proto::golem::rib::expr::Expr::Range(
                            Box::new(golem_api_grpc::proto::golem::rib::RangeExpr {
                                range_expr: Some(RangeExpr::RangeFrom(Box::new(
                                    golem_api_grpc::proto::golem::rib::RangeFrom {
                                        from: Some(Box::new((*from).into())),
                                    },
                                ))),
                            }),
                        ))
                    }
                    Range::Range { from, to } => {
                        Some(golem_api_grpc::proto::golem::rib::expr::Expr::Range(
                            Box::new(golem_api_grpc::proto::golem::rib::RangeExpr {
                                range_expr: Some(RangeExpr::Range(Box::new(
                                    golem_api_grpc::proto::golem::rib::Range {
                                        from: Some(Box::new((*from).into())),
                                        to: Some(Box::new((*to).into())),
                                    },
                                ))),
                            }),
                        ))
                    }
                    Range::RangeInclusive { from, to } => {
                        Some(golem_api_grpc::proto::golem::rib::expr::Expr::Range(
                            Box::new(golem_api_grpc::proto::golem::rib::RangeExpr {
                                range_expr: Some(RangeExpr::RangeInclusive(Box::new(
                                    golem_api_grpc::proto::golem::rib::RangeInclusive {
                                        from: Some(Box::new((*from).into())),
                                        to: Some(Box::new((*to).into())),
                                    },
                                ))),
                            }),
                        ))
                    }
                },

                Expr::SelectIndex {
                    expr,
                    index,
                    type_annotation,
                    ..
                } => Some(
                    golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndexV1(Box::new(
                        golem_api_grpc::proto::golem::rib::SelectIndexExprV1 {
                            expr: Some(Box::new((*expr).into())),
                            index: Some(Box::new((*index).into())),
                            type_name: type_annotation.map(|t| t.into()),
                        },
                    )),
                ),

                Expr::Sequence {
                    exprs: expressions,
                    type_annotation,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
                    golem_api_grpc::proto::golem::rib::SequenceExpr {
                        exprs: expressions.into_iter().map(|expr| expr.into()).collect(),
                        type_name: type_annotation.map(|t| t.into()),
                    },
                )),
                Expr::Record { exprs: fields, .. } => {
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
                Expr::Tuple {
                    exprs: expressions, ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Tuple(
                    golem_api_grpc::proto::golem::rib::TupleExpr {
                        exprs: expressions.into_iter().map(|expr| expr.into()).collect(),
                    },
                )),
                Expr::Literal { value, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Literal(
                        golem_api_grpc::proto::golem::rib::LiteralExpr { value },
                    ))
                }
                Expr::Number {
                    number,
                    type_annotation,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Number(
                    golem_api_grpc::proto::golem::rib::NumberExpr {
                        number: Some(number.value.to_string()),
                        float: None,
                        type_name: type_annotation.map(|t| t.into()),
                    },
                )),
                Expr::Flags { flags, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Flags(
                        golem_api_grpc::proto::golem::rib::FlagsExpr { values: flags },
                    ))
                }
                Expr::Identifier {
                    variable_id,
                    type_annotation,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Identifier(
                    golem_api_grpc::proto::golem::rib::IdentifierExpr {
                        name: variable_id.name(),
                        type_name: type_annotation.map(|t| t.into()),
                    },
                )),
                Expr::Boolean { value, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
                        golem_api_grpc::proto::golem::rib::BooleanExpr { value },
                    ))
                }
                Expr::Concat {
                    exprs: expressions, ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Concat(
                    golem_api_grpc::proto::golem::rib::ConcatExpr {
                        exprs: expressions.into_iter().map(|expr| expr.into()).collect(),
                    },
                )),
                Expr::ExprBlock {
                    exprs: expressions, ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Multiple(
                    golem_api_grpc::proto::golem::rib::MultipleExpr {
                        exprs: expressions.into_iter().map(|expr| expr.into()).collect(),
                    },
                )),
                Expr::Not { expr, .. } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Not(
                    Box::new(golem_api_grpc::proto::golem::rib::NotExpr {
                        expr: Some(Box::new((*expr).into())),
                    }),
                )),
                Expr::GreaterThan {
                    lhs: left,
                    rhs: right,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThan(
                    Box::new(golem_api_grpc::proto::golem::rib::GreaterThanExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    }),
                )),
                Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => Some(
                    golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThanOrEqual(Box::new(
                        golem_api_grpc::proto::golem::rib::GreaterThanOrEqualToExpr {
                            left: Some(Box::new((*lhs).into())),
                            right: Some(Box::new((*rhs).into())),
                        },
                    )),
                ),
                Expr::LessThan {
                    lhs: left,
                    rhs: right,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::LessThan(
                    Box::new(golem_api_grpc::proto::golem::rib::LessThanExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    }),
                )),
                Expr::Plus {
                    lhs: left,
                    rhs: right,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Add(
                    Box::new(golem_api_grpc::proto::golem::rib::AddExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    }),
                )),
                Expr::Minus {
                    lhs: left,
                    rhs: right,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Subtract(
                    Box::new(golem_api_grpc::proto::golem::rib::SubtractExpr {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    }),
                )),
                Expr::Divide { lhs, rhs, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Divide(
                        Box::new(golem_api_grpc::proto::golem::rib::DivideExpr {
                            left: Some(Box::new((*lhs).into())),
                            right: Some(Box::new((*rhs).into())),
                        }),
                    ))
                }
                Expr::Multiply { lhs, rhs, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Multiply(
                        Box::new(golem_api_grpc::proto::golem::rib::MultiplyExpr {
                            left: Some(Box::new((*lhs).into())),
                            right: Some(Box::new((*rhs).into())),
                        }),
                    ))
                }
                Expr::LessThanOrEqualTo { lhs, rhs, .. } => Some(
                    golem_api_grpc::proto::golem::rib::expr::Expr::LessThanOrEqual(Box::new(
                        golem_api_grpc::proto::golem::rib::LessThanOrEqualToExpr {
                            left: Some(Box::new((*lhs).into())),
                            right: Some(Box::new((*rhs).into())),
                        },
                    )),
                ),
                Expr::EqualTo { lhs, rhs, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::EqualTo(
                        Box::new(golem_api_grpc::proto::golem::rib::EqualToExpr {
                            left: Some(Box::new((*lhs).into())),
                            right: Some(Box::new((*rhs).into())),
                        }),
                    ))
                }
                // Note: We were storing and retrieving (proto) condition expressions such that
                // `cond` was written `lhs` and vice versa.
                // This is probably difficult to fix to keep backward compatibility
                // The issue is only with the protobuf types and the roundtrip tests were/are working since
                // the read handles this (i.e, reading cond as lhs)
                Expr::Cond {
                    cond: lhs,
                    lhs: cond,
                    rhs,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Cond(
                    Box::new(golem_api_grpc::proto::golem::rib::CondExpr {
                        left: Some(Box::new((*lhs).into())),
                        cond: Some(Box::new((*cond).into())),
                        right: Some(Box::new((*rhs).into())),
                    }),
                )),
                Expr::PatternMatch {
                    predicate,
                    match_arms,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::PatternMatch(
                    Box::new(golem_api_grpc::proto::golem::rib::PatternMatchExpr {
                        expr: Some(Box::new((*predicate).into())),
                        patterns: match_arms.into_iter().map(|a| a.into()).collect(),
                    }),
                )),
                Expr::Option {
                    expr,
                    type_annotation,
                    ..
                } => Some(golem_api_grpc::proto::golem::rib::expr::Expr::Option(
                    Box::new(golem_api_grpc::proto::golem::rib::OptionExpr {
                        expr: expr.map(|expr| Box::new((*expr).into())),
                        type_name: type_annotation.map(|t| t.into()),
                    }),
                )),
                Expr::Result {
                    expr,
                    type_annotation,
                    ..
                } => {
                    let type_name = type_annotation.map(|t| t.into());

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
                Expr::Call {
                    call_type,
                    generic_type_parameter,
                    args,
                    ..
                } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Call(
                        Box::new(golem_api_grpc::proto::golem::rib::CallExpr {
                            name: None, // Kept for backward compatibility
                            params: args.into_iter().map(|expr| expr.into()).collect(),
                            generic_type_parameter: generic_type_parameter.map(|t| t.value),
                            call_type: Some(Box::new(
                                golem_api_grpc::proto::golem::rib::CallType::from(call_type),
                            )),
                        }),
                    ))
                }
                Expr::Unwrap { expr, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Unwrap(
                        Box::new(golem_api_grpc::proto::golem::rib::UnwrapExpr {
                            expr: Some(Box::new((*expr).into())),
                        }),
                    ))
                }
                Expr::Throw { message, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Throw(
                        golem_api_grpc::proto::golem::rib::ThrowExpr { message },
                    ))
                }
                Expr::GetTag { expr, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Tag(
                        Box::new(golem_api_grpc::proto::golem::rib::GetTagExpr {
                            expr: Some(Box::new((*expr).into())),
                        }),
                    ))
                }
                Expr::And { lhs, rhs, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::And(
                        Box::new(golem_api_grpc::proto::golem::rib::AndExpr {
                            left: Some(Box::new((*lhs).into())),
                            right: Some(Box::new((*rhs).into())),
                        }),
                    ))
                }

                Expr::Or { lhs, rhs, .. } => {
                    Some(golem_api_grpc::proto::golem::rib::expr::Expr::Or(Box::new(
                        golem_api_grpc::proto::golem::rib::OrExpr {
                            left: Some(Box::new((*lhs).into())),
                            right: Some(Box::new((*rhs).into())),
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
                Expr::InvokeMethodLazy {
                    lhs,
                    method,
                    generic_type_parameter,
                    args,
                    ..
                } => Some(
                    golem_api_grpc::proto::golem::rib::expr::Expr::LazyInvokeMethod(Box::new(
                        golem_api_grpc::proto::golem::rib::LazyInvokeMethodExpr {
                            lhs: Some(Box::new((*lhs).into())),
                            method,
                            generic_type_parameter: generic_type_parameter.map(|t| t.value),
                            args: args.into_iter().map(|expr| expr.into()).collect(),
                        },
                    )),
                ),
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

fn find_expr(expr: &mut Expr, source_span: &SourceSpan) -> Option<Expr> {
    let mut expr = expr.clone();

    let mut visitor = ExprVisitor::bottom_up(&mut expr);

    while let Some(current) = visitor.pop_back() {
        let span = current.source_span();

        if source_span.eq(&span) {
            return Some(current.clone());
        }
    }

    None
}
