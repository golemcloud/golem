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

use crate::expr::internal::{IdentifierTypeState, IdentifierVariableIdState};
use crate::function_name::ParsedFunctionName;
use crate::parser::rib_expr::rib_program;
use crate::type_registry::{FunctionTypeRegistry, RegistryKey, RegistryValue};
use crate::{text, InferredType};
use bincode::{Decode, Encode};
use combine::EasyParser;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
struct VariableId(Option<u16>);

impl VariableId {
    pub fn init() -> Self {
        VariableId(None)
    }
    pub fn increment(&mut self) -> VariableId {
        let new_variable_id = self.0.map_or(Some(0), |x| Some(x + 1));
        self.0 = new_variable_id;
        VariableId(new_variable_id)
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Expr {
    // Let(Option<variable-id>)
    Let(VariableId, String, Box<Expr>, InferredType),
    SelectField(Box<Expr>, String, InferredType), // SelectField(SelectField(Expr::Identifier("request"), "body", InferredType::Record), "streetNumber", U8)
    SelectIndex(Box<Expr>, usize, InferredType),
    Sequence(Vec<Expr>, InferredType),
    Record(Vec<(String, Box<Expr>)>, InferredType),
    Tuple(Vec<Expr>, InferredType),
    Literal(String, InferredType),
    Number(Number, InferredType),
    Flags(Vec<String>, InferredType),
    Identifier(String, InferredType, VariableId),
    Boolean(bool, InferredType),
    Concat(Vec<Expr>, InferredType),
    Multiple(Vec<Expr>, InferredType),
    Not(Box<Expr>, InferredType),
    GreaterThan(Box<Expr>, Box<Expr>, InferredType),
    GreaterThanOrEqualTo(Box<Expr>, Box<Expr>, InferredType),
    LessThanOrEqualTo(Box<Expr>, Box<Expr>, InferredType),
    EqualTo(Box<Expr>, Box<Expr>, InferredType),
    LessThan(Box<Expr>, Box<Expr>, InferredType),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>, InferredType),
    PatternMatch(Box<Expr>, Vec<MatchArm>, InferredType),
    Option(Option<Box<Expr>>, InferredType),
    Result(Result<Box<Expr>, Box<Expr>>, InferredType),
    Call(ParsedFunctionName, Vec<Expr>, InferredType),
}

impl Expr {
    pub fn boolean(value: bool) -> Self {
        Expr::Boolean(value, InferredType::Bool)
    }

    pub fn call(parsed_fn_name: ParsedFunctionName, args: Vec<Expr>) -> Self {
        Expr::Call(parsed_fn_name, args, InferredType::Unknown)
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

    pub fn error(expr: Expr) -> Self {
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
        Expr::Flags(flags, InferredType::Unknown)
    }

    pub fn greater_than(left: Expr, right: Expr) -> Self {
        Expr::GreaterThan(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn greater_than_or_equal_to(left: Expr, right: Expr) -> Self {
        Expr::GreaterThanOrEqualTo(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn identifier(name: &str) -> Self {
        Expr::Identifier(name.to_string(), InferredType::Unknown, VariableId::init())
    }

    pub fn less_than(left: Expr, right: Expr) -> Self {
        Expr::LessThan(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn less_than_or_equal_to(left: Expr, right: Expr) -> Self {
        Expr::LessThanOrEqualTo(Box::new(left), Box::new(right), InferredType::Bool)
    }

    pub fn let_binding(name: &str, expr: Expr) -> Self {
        Expr::Let(
            VariableId::init(),
            name.to_string(),
            Box::new(expr),
            InferredType::Unknown,
        )
    }

    pub fn literal(value: impl AsRef<str>) -> Self {
        Expr::Literal(value.to_string(), InferredType::Str)
    }

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

    pub fn select_field(expr: Expr, field: &str) -> Self {
        Expr::SelectField(Box::new(expr), field.to_string(), InferredType::Unknown)
    }

    pub fn select_index(expr: Expr, index: usize) -> Self {
        Expr::SelectIndex(Box::new(expr), index, InferredType::Unknown)
    }

    pub fn tuple(expressions: Vec<Expr>) -> Self {
        Expr::Tuple(expressions, InferredType::Unknown)
    }

    pub fn sequence(expressions: Vec<Expr>) -> Self {
        let inferred_type = if expressions.is_empty() {
            InferredType::Unknown
        } else {
            InferredType::List(Box::new(expressions.first().unwrap().inferred_type()))
        };
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
            | Expr::Number(_, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Identifier(_, _, _)
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
            | Expr::Call(_, _, inferred_type) => inferred_type.clone(),
        }
    }
    // We make sure the let bindings name are properly
    // binded to the named identifiers
    pub fn name_binding(&mut self) {
        let mut latest_identifier_identity = IdentifierVariableIdState::new();
        let mut queue = VecDeque::new();
        queue.push_back(self);

        // Start from the end
        if let Some(expr) = queue.pop_front() {
            match expr {
                Expr::Let(variable_id, name, expr, _) => {
                    let id = variable_id.increment();
                    latest_identifier_identity.update(&name, id);
                    expr.name_binding();
                }

                Expr::Identifier(name, _, variable_id) => {
                    if let Some(latest_variable_id) = latest_identifier_identity.lookup(name) {
                        *variable_id = latest_variable_id.clone();
                    }
                }

                _ => expr.visit_children(&mut queue),
            }
        }
    }

    // At this point we simply update the types to the parameter type expressions and the call expression itself
    pub fn infer_function_types(&mut self, function_type_registry: &FunctionTypeRegistry) {
        let mut queue = VecDeque::new();
        queue.push_back(self);
        // From the end to top
        while let Some(expr) = queue.pop_back() {
            // call(x), let x = 1;
            match expr {
                Expr::Call(parsed_fn_name, args, inferred_type) => {
                    // TODO; Retrieve interface name properly from parsed_fn_name
                    let key = RegistryKey::FunctionName(parsed_fn_name.to_string());
                    if let Some(values) = function_type_registry.types.get(&key) {
                        for value in values {
                            if let RegistryValue::Function {
                                parameter_types,
                                return_types,
                            } = value
                            {
                                // Check if the argument types match
                                if parameter_types.len() == args.len() {
                                    for (arg, param_type) in args.iter_mut().zip(parameter_types) {
                                        arg.add_infer_type(param_type.into());
                                        // to handle the scenario of
                                        // let x = 1;
                                        // let y = 2;
                                        // call({foo: x, bar: y})
                                        arg.push_types_down()
                                        // x -> AnalysedType::U64, Expr::Identifier(x, InferredType::Leaf(AnalysedType::U64))
                                    }
                                    // Update inferred type with the function's return type
                                    *inferred_type = InferredType::Sequence(
                                        return_types.iter().map(|t| t.into()).collect(),
                                    );
                                    // Expr::Call(parsed_fn_name, args, InferredType::Sequence(return_types.iter().map(|t| InferredType::Leaf(t.clone())).collect()));
                                }
                            }
                        }
                    }
                }
                // Continue for nested expressions
                _ => expr.visit_children(&mut queue),
            }
        }
    }

    // selectField, and selectIndex -> specifying what each field is
    // SelectField(SelectField(Expr::Identifier("request"), "body", InferredType::Unknown), "streetNumber", AnalysedType::U8)
    // updated to
    // SelectField(SelectField(Expr::Identifier("request")..., "body", InferredType::Record-having-street-number-which-is-u8), "streetNumber", Analysed)
    pub fn push_types_down(&mut self) {
        let mut queue = VecDeque::new();
        queue.push_back(self);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::SelectField(expr, field, inferred_type) => {
                    let field_type = inferred_type.clone();
                    let record_type = vec![(field.to_string(), field_type)];
                    let inferred_record_type = InferredType::Record(record_type);

                    // the type of the expr is a record type having the specific field
                    expr.add_infer_type(inferred_record_type);
                }

                Expr::SelectIndex(expr, index, inferred_type) => {
                    // If the field is not known, we update the inferred type with the field type

                    let field_type = inferred_type.clone();
                    let inferred_record_type = InferredType::List(Box::new(field_type));

                    // the type of the expr is a record type having the specific field
                    expr.add_infer_type(inferred_record_type);
                }
                Expr::Cond(cond, then, else_, inferred_type) => {
                    // If an entire if condition is inferred to be a specific type, then both branches should be of the same type
                    // If the field is not known, we update the inferred type with the field type
                    then.add_infer_type(inferred_type.clone());
                    else_.add_infer_type(inferred_type.clone());

                    // A condition expression is always a boolean type and can be tagged as a boolean
                    cond.add_infer_type(InferredType::Bool);
                }
                Expr::Not(expr, inferred_type) => {
                    // The inferred_type should be ideally boolean type and should be pushed down as a boolean type
                    // however, at this phase, we are unsure and we propogate the inferred_type as is
                    expr.add_infer_type(inferred_type.clone());
                }
                Expr::Option(Some(expr), inferred_type) => {
                    // The inferred_type should be ideally optional type, i.e, either Unknown type. or all of multiple optional types, or one of all optional types,
                    // and otherwise we give up inferring the internal type at this phase
                    match inferred_type {
                        InferredType::Option(ref t) => {
                            expr.add_infer_type(*t.clone());
                        }
                        InferredType::AllOf(types) => {
                            let mut all_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Option(ref t) => {
                                        all_types.push(*t.clone());
                                    }
                                    _ => {}
                                }
                            }
                            expr.add_infer_type(InferredType::AllOf(all_types));
                        }
                        InferredType::OneOf(types) => {
                            let mut one_of_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Option(ref t) => {
                                        one_of_types.push(*t.clone());
                                    }
                                    _ => {}
                                }
                            }
                            expr.add_infer_type(InferredType::OneOf(one_of_types));
                        }
                        // we can't push down the types otherwise
                        _ => {}
                    }
                }

                Expr::Result(Ok(expr), inferred_type) => {
                    // The inferred_type should be ideally result type, i.e, either Unknown type. or all of multiple result types, or one of all result types,
                    // and otherwise we give up inferring the internal type at this phase
                    match inferred_type {
                        InferredType::Result { ref ok, error } => {
                            if let Some(ok) = ok {
                                expr.add_infer_type(*ok.clone());
                            }
                        }
                        InferredType::AllOf(types) => {
                            let mut all_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Result { ok, error } => {
                                        if let Some(ok) = ok {
                                            all_types.push(*ok.clone());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            expr.add_infer_type(InferredType::AllOf(all_types));
                        }
                        InferredType::OneOf(types) => {
                            let mut one_of_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Result { ref ok, error } => {
                                        if let Some(ok) = ok {
                                            one_of_types.push(*ok.clone());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            expr.add_infer_type(InferredType::OneOf(one_of_types));
                        }
                        // we can't push down the types otherwise
                        _ => {}
                    }
                }

                Expr::Result(Err(expr), inferred_type) => {
                    // The inferred_type should be ideally result type, i.e, either Unknown type. or all of multiple result types, or one of all result types,
                    // and otherwise we give up inferring the internal type at this phase
                    match inferred_type {
                        InferredType::Result { ref error, .. } => {
                            if let Some(error) = error {
                                expr.add_infer_type(*error.clone());
                            }
                        }
                        InferredType::AllOf(types) => {
                            let mut all_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Result { ref error, .. } => {
                                        if let Some(error) = error {
                                            all_types.push(*error.clone());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            expr.add_infer_type(InferredType::AllOf(all_types));
                        }
                        InferredType::OneOf(types) => {
                            let mut one_of_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Result { ref error, .. } => {
                                        if let Some(error) = error {
                                            one_of_types.push(*error.clone());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            expr.add_infer_type(InferredType::OneOf(one_of_types));
                        }
                        // we can't push down the types otherwise
                        _ => {}
                    }
                }
                Expr::PatternMatch(_, match_arms, inferred_type) => {
                    // There is nothing to be done for condition_expr and match_arm's pattern
                    for match_arm in match_arms {
                        let mut match_arm_expr = match_arm.0 .1.clone();
                        match_arm_expr.add_infer_type(inferred_type.clone());
                        let arm_pattern = match_arm.0 .0.clone();
                        *match_arm = MatchArm((arm_pattern, match_arm_expr));
                    }
                }

                Expr::Tuple(exprs, inferred_type) => {
                    // The inferred_type should be ideally tuple type, i.e, either Unknown type. or all of multiple tuple types, or one of all tuple types,
                    // and otherwise we give up inferring the internal type at this phase
                    match inferred_type {
                        InferredType::Tuple(types) => {
                            for (expr, typ) in exprs.iter_mut().zip(types) {
                                expr.add_infer_type(typ.clone());
                            }
                        }
                        InferredType::AllOf(types) => {
                            let mut all_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Tuple(types) => {
                                        all_types.extend(types);
                                    }
                                    _ => {}
                                }
                            }
                            for (expr, typ) in exprs.iter_mut().zip(all_types) {
                                expr.add_infer_type(typ.clone());
                            }
                        }
                        InferredType::OneOf(types) => {
                            let mut one_of_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Tuple(types) => {
                                        one_of_types.extend(types);
                                    }
                                    _ => {}
                                }
                            }
                            for (expr, typ) in exprs.iter_mut().zip(one_of_types) {
                                expr.add_infer_type(typ.clone());
                            }
                        }
                        // we can't push down the types otherwise
                        _ => {}
                    }
                }
                Expr::Sequence(expressions, inferred_type) => {
                    // The inferred_type should be ideally sequence type, i.e, either Unknown type. or all of multiple sequence types, or one of all sequence types,
                    // and otherwise we give up inferring the internal type at this phase
                    match inferred_type {
                        InferredType::Sequence(types) => {
                            for (expr, typ) in expressions.iter_mut().zip(types) {
                                expr.add_infer_type(typ.clone());
                            }
                        }
                        InferredType::AllOf(types) => {
                            let mut all_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Sequence(types) => {
                                        all_types.extend(types);
                                    }
                                    _ => {}
                                }
                            }
                            for (expr, typ) in expressions.iter_mut().zip(all_types) {
                                expr.add_infer_type(typ.clone());
                            }
                        }
                        InferredType::OneOf(types) => {
                            let mut one_of_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Sequence(types) => {
                                        one_of_types.extend(types);
                                    }
                                    _ => {}
                                }
                            }
                            for (expr, typ) in expressions.iter_mut().zip(one_of_types) {
                                expr.add_infer_type(typ.clone());
                            }
                        }
                        // we can't push down the types otherwise
                        _ => {}
                    }
                }

                Expr::Record(expressions, inferred_type) => {
                    // The inferred_type should be ideally record type, i.e, either Unknown type. or all of multiple record types, or one of all record types,
                    // and otherwise we give up inferring the internal type at this phase
                    match inferred_type {
                        InferredType::Record(types) => {
                            for (field_name, expr) in expressions.iter_mut() {
                                if let Some((_, typ)) =
                                    types.iter().find(|(name, _)| name == field_name)
                                {
                                    expr.add_infer_type(typ.clone());
                                }
                                queue.push_back(expr);
                            }
                        }
                        InferredType::AllOf(types) => {
                            let mut all_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Record(types) => {
                                        all_types.extend(types);
                                    }
                                    _ => {}
                                }
                            }
                            for (field_name, expr) in expressions.iter_mut() {
                                if let Some((_, typ)) =
                                    all_types.iter().find(|(name, _)| name == field_name)
                                {
                                    expr.add_infer_type(typ.clone());
                                }

                                queue.push_back(expr);
                            }
                        }
                        InferredType::OneOf(types) => {
                            let mut one_of_types = vec![];
                            for typ in types {
                                match typ {
                                    InferredType::Record(types) => {
                                        one_of_types.extend(types);
                                    }
                                    _ => {}
                                }
                            }
                            for (field_name, expr) in expressions.iter_mut() {
                                if let Some((_, typ)) =
                                    one_of_types.iter().find(|(name, _)| name == field_name)
                                {
                                    expr.add_infer_type(typ.clone());
                                }
                                queue.push_back(expr);
                            }
                        }
                        // we can't push down the types otherwise
                        _ => {}
                    }
                }
                expr @ Expr::Literal(_, _) => expr.visit_children(&mut queue),
                expr @ Expr::Number(_, _) => expr.visit_children(&mut queue),
                expr @ Expr::Flags(_, _) => expr.visit_children(&mut queue),
                expr @ Expr::Identifier(_, _, _) => expr.visit_children(&mut queue),
                expr @ Expr::Boolean(_, _) => expr.visit_children(&mut queue),
                expr @ Expr::Concat(_, _) => expr.visit_children(&mut queue),
                expr @ Expr::Multiple(_, _) => expr.visit_children(&mut queue),
                expr @ Expr::GreaterThan(_, _, _) => expr.visit_children(&mut queue),
                expr @ Expr::GreaterThanOrEqualTo(_, _, _) => expr.visit_children(&mut queue),
                expr @ Expr::LessThanOrEqualTo(_, _, _) => expr.visit_children(&mut queue),
                expr @ Expr::EqualTo(_, _, _) => expr.visit_children(&mut queue),
            }
        }
    }

    // Should updating the let binding be before or after pushing down/up the types?
    //
    // Here is an example where `before` works:
    // let x = { a: 1, b: 2 };
    // call(Box::Identifier(x))
    // We update the let binding's expression type to be a record type with a and b fields
    // Now we have type under let be identified as a record type with each fields' type specified
    // and then we push down the types of that record to a and b.
    //
    // Here is an example where `after` works:
    // let x = 1;
    // call({ foo: x, bar: y })
    // Here we identified the expr in call to be a record type with foo and bar fields, with specific types
    // However, we haven't pushed down the types yet, such that x's type is still unknown.
    // Therefore cannot update the let binding type yet.
    // There we must push down the types first and then update the let binding type
    // while pushing down the let binding type is a noop
    // and then we later update the let binding type

    // one answer here would be to pass it twice, i.e, before and after pushing down.
    // or to avoid double pass, during type inference itself, we can call a push down function.
    // and then update the let binding types, and then push down all the types altogether
    pub fn update_let_binding_type(&mut self) {
        let mut identifier_lookup = IdentifierTypeState::new();
        let mut queue = VecDeque::new();
        queue.push_back(self);

        // We start from the end and pick the identifiers type
        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(name, inferred_type, variable_id) => {
                    identifier_lookup.update(name, variable_id.clone(), inferred_type.clone());
                }
                Expr::Let(variable_id, name, expr, _) => {
                    if let Some(inferred_type) = identifier_lookup.lookup(name, variable_id.clone())
                    {
                        expr.add_infer_type(inferred_type);
                        expr.push_types_down();
                    }
                }
                _ => expr.visit_children(&mut queue),
            }
        }
    }

    // last line
    // {body : {address: "foo", street_number: streetNumber}}}
    // Start with leaf nodes and work the way up the tree.
    // Record("body" -> Record("address" -> "foo": String, "street_number" -> streetNumber: U8))
    // Record("body" -> Record("address" -> "foo", "street_number" -> streetNumber: U8), AnalysedType::Recpord("body" -> Record("address" -> "String", "street_number" -> streetNumber: U8)))
    pub fn pull_types_up(&mut self) {
        let mut queue = VecDeque::new();
        queue.push_back(self);
        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Tuple(exprs, inferred_type) => {
                    let mut types = vec![];
                    for expr in exprs {
                        types.push(expr.inferred_type());
                    }
                    let tuple_type = InferredType::Tuple(types);
                    inferred_type.update(tuple_type)
                }
                Expr::Sequence(exprs, inferred_type) => {
                    let mut types = vec![];
                    for expr in exprs {
                        types.push(expr.inferred_type());
                    }
                    let sequence_type = InferredType::Sequence(types);
                    inferred_type.update(sequence_type)
                }
                Expr::Record(exprs, inferred_type) => {
                    let mut types = vec![];
                    for (field_name, expr) in exprs {
                        types.push((field_name.clone(), expr.inferred_type()));
                    }
                    let record_type = InferredType::Record(types);
                    inferred_type.update(record_type)
                }
                Expr::Option(Some(expr), inferred_type) => {
                    let option_type = InferredType::Option(Box::new(expr.inferred_type()));
                    inferred_type.update(option_type)
                }
                Expr::Result(Ok(expr), inferred_type) => {
                    let result_type = InferredType::Result {
                        ok: Some(Box::new(expr.inferred_type())),
                        error: None,
                    };
                    inferred_type.update(result_type)
                }
                Expr::Result(Err(expr), inferred_type) => {
                    let result_type = InferredType::Result {
                        ok: None,
                        error: Some(Box::new(expr.inferred_type())),
                    };
                    inferred_type.update(result_type)
                }

                Expr::Cond(_, then_, else_, inferred_type) => {
                    let then_type = then_.inferred_type();
                    let else_type = else_.inferred_type();

                    if then_type == else_type {
                        inferred_type.update(then_type);
                    } else {
                        let cond_then_else_type = InferredType::AllOf(vec![then_type, else_type]);
                        inferred_type.update(cond_then_else_type)
                    }
                }
                Expr::PatternMatch(_, match_arms, inferred_type) => {
                    let mut possible_inference_types = vec![];
                    for match_arm in match_arms {
                        possible_inference_types.push(match_arm.arm_expr().inferred_type())
                    }

                    if !possible_inference_types.is_empty() {
                        let first_type = possible_inference_types[0].clone();
                        if possible_inference_types.iter().all(|t| t == &first_type) {
                            inferred_type.update(first_type);
                        } else {
                            inferred_type.update(InferredType::AllOf(possible_inference_types));
                        }
                    }
                }

                _ => expr.visit_children(&mut queue),
            }
        }
    }

    // Aggregate the fragment of type info of global variables
    // and infer the final type. Take two request's type (identified through pullup/down, and make sure the type consist of all fields and value types)
    // more about handling information about inputs (request)
    // let x = request;
    // let y = x.body.streetNumber;
    // call_y(y);
    // let z = x.header.user;
    // call_z(z);
    // Now, before the infer_input_type, following is the state
    // We tagged the identifier expression of z inside call_z to be a Str
    // We tagged the identifier expression of y inside call_y expression to be a u8
    // We updated the let binding types before pushing down or pushing up
    // Meaning, the expression `x.header.user` is updated to be Str
    // and the expression `x.body.streetNumber` is updated to be u8
    // During these steps itself, we made some specific push down, meaning, we made sure we tag the type of `x.body` and `x.header` expression are record,
    // and therefore the type of Identifier(x) to be a record having header in one place, and to be a record having body in another place.
    // And the queue is popped up further, the type of x is propogated to the type of request.
    // i.e, we are fully type inferred even before calling infer_input_type.
    // We have a separate push down and pull up phase in the type inference.
    // let x = { street_number: request.body.stretNumber, street_name: request.headers.streetName };
    // call_x(x.street_number);
    // call_y(x.street_name);
    // During type inference phase, the following happened:
    // Here we inferred the expressions x.street_number expression to be u8 and x.street_name to be Str
    // we separately push down the types in this process, to say Identifier(x) is a record type having street_number in one place
    // and Expr::Identifier(y) to be a street_name in another place.
    // In the next phase of updating the let binding types:
    // We pop from the back of the queue, and it encountered an idenitifer(x) having a specific record type with street_number and updated the dictionary
    // and it encountered again the identifier(x) having a specific record type with street_name, and it tries to update the dictionary resulting in AllOf(record1, record2)
    // And then it bumps into the let binding of x, and x's expression is updated to be this AllOf(record1, record2)
    // During this phase itself, it tries to push down the type AllOf to the inner expressions.
    // Ex: Expr::Record(
    //  vec![
    //    "streetNumber" -> Expr::SelectField(Expr::SelectField(Expr::Identifier("request"), "body"), "streetNumber")], InferredType::Unknown),
    //    "streetName" -> Expr::SelectField(Expr::SelectField(Expr::Identifier("request"), "body"), "streetName")], InferredType::Unknown),
    //  ]
    //  InferredType::AllOf(vec![InferredType::Record(vec![("streetNumber", U8)]), InferredType::Record(vec![("streetName", String)])])
    // is updated to
    // Expr::Record(
    //  vec![
    //    "streetNumber" -> Expr::SelectField(Expr::SelectField(Expr::Identifier("request"), "body"), "streetNumber", U8),
    //    "streetName" -> Expr::SelectField(Expr::SelectField(Expr::Identifier("request"), "headers"), "streetName", String),
    //  ]
    // )
    // Internally we push the whole expressions above into the queue and
    // in updating the type of Expr::SelectField(Expr::Identifier("request"), "body") to be a record type with field streetNumber
    // and and updating the type of Expr::SelectField(Expr::Identifier("request"), "headers") to be a record type with field streetName
    // we push this down further so that Expr:Identifier("request") is updated to be a record type with field "body" in one place
    // and field "headers" in another place. This implies same identifier (same variable-id) can have different types in different places
    // especially if they are global identifiers (that one which doesn't have a corresponding let binding). We call this input_type
    pub fn infer_input_type(&mut self) {
        // Collecting the fragmented types of input
        let mut queue = VecDeque::new();
        queue.push_back(self);

        let mut all_types_of_global_variables = HashMap::new();
        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_name, inferred_type, variable_id) => {
                    // We are only interested in global variables
                    if variable_id.is_none() {
                        all_types_of_global_variables
                            .entry(variable_name.clone())
                            .or_insert(Vec::new())
                            .push(inferred_type.clone());
                    }
                }
                _ => expr.visit_children(&mut queue),
            }
        }

        // Updating the collected types in all positions of input
        let mut queue = VecDeque::new();
        queue.push_back(self);
        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_name, inferred_type, variable_id) => {
                    // We are only interested in global variables
                    if variable_id.is_none() {
                        if let Some(types) = all_types_of_global_variables.get(variable_name) {
                            inferred_type.update(InferredType::AllOf(types.clone()));
                        }
                    }
                }
                _ => expr.visit_children(&mut queue),
            }
        }
    }

    // Doesn't need to be mutable
    pub fn type_check(&mut self) -> Result<(), Vec<String>> {
        let mut queue = VecDeque::new();

        let mut errors = vec![];

        queue.push_back(self);

        while let Some(expr) = queue.pop_back() {
            match expr {
                expr @ Expr::Record(vec, inferred_type) => {
                    queue.extend(vec.iter().map(|(_, expr)| expr));
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::Tuple(vec, inferred_type) => {
                    queue.extend(vec.iter());
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::Sequence(vec, inferred_type) => {
                    queue.extend(vec.iter());
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::Option(Some(expr), inferred_type) => {
                    queue.push_back(expr);
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::Result(Ok(expr), inferred_type) => {
                    queue.push_back(expr);
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::Result(Err(expr), inferred_type) => {
                    queue.push_back(expr);
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::Cond(cond, then, else_, inferred_type) => {
                    queue.push_back(cond);
                    queue.push_back(then);
                    queue.push_back(else_);
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::PatternMatch(expr, arms, inferred_type) => {
                    queue.push_back(expr);
                    for arm in arms {
                        let mut arm_expr = arm.arm_expr();
                        queue.push_back(&mut arm_expr);
                    }
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::Call(_, vec, inferred_type) => {
                    queue.extend(vec.iter());
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::SelectField(expr, _, inferred_type) => {
                    queue.push_back(expr);
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                Expr::SelectIndex(expr, _, inferred_type) => {
                    queue.push_back(expr);
                    if !inferred_type.type_check() {
                        errors.push(format!("{} is inferred to have incompatible types", expr));
                    }
                }
                _ => {
                    //Avoid this
                    let mut expr = expr.clone();
                    expr.visit_children(&mut queue)
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    // Once type checked we can unify the types without erroring out
    fn unify_types(&mut self) -> Result<(), Vec<String>> {
        let mut queue = VecDeque::new();
        queue.push_back(self);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Record(vec, inferred_type) => {
                    queue.extend(vec.iter().map(|(_, expr)| expr));
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::Tuple(vec, inferred_type) => {
                    queue.extend(vec.iter());
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::Sequence(vec, inferred_type) => {
                    queue.extend(vec.iter());
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::Option(Some(expr), inferred_type) => {
                    queue.push_back(expr);
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::Result(Ok(expr), inferred_type) => {
                    queue.push_back(expr);
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::Result(Err(expr), inferred_type) => {
                    queue.push_back(expr);
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::Cond(cond, then, else_, inferred_type) => {
                    queue.push_back(cond);
                    queue.push_back(then);
                    queue.push_back(else_);
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::PatternMatch(expr, arms, inferred_type) => {
                    queue.push_back(expr);
                    // TODOl this is wrong, make match arm be able to mutate
                    for arm in arms {
                        let mut arm_expr = arm.arm_expr();
                        queue.push_back(&mut arm_expr);
                    }
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::Call(_, vec, inferred_type) => {
                    queue.extend(vec.iter());
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::SelectField(expr, _, inferred_type) => {
                    queue.push_back(expr);
                    *inferred_type = inferred_type.unify_types()?;
                }
                Expr::SelectIndex(expr, _, inferred_type) => {
                    queue.push_back(expr);
                    *inferred_type = inferred_type.unify_types()?;
                }
                _ => expr.visit_children(&mut queue),
            }
        }

        Ok(())
    }

    fn compile_to_ir(&mut self) {
        // Lower Level Stack basd instruction set with full type information , plus ProtoVal
        // Encountering unknown types - we reject it
    }

    fn add_infer_type(&mut self, new_inferred_type: InferredType) {
        match self {
            Expr::Identifier(_, inferred_type, _)
            | Expr::Let(_, _, _, inferred_type)
            | Expr::SelectField(_, _, inferred_type)
            | Expr::SelectIndex(_, _, inferred_type)
            | Expr::Sequence(_, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            // Expr::Number(1, AnalysedType::U64)
            | Expr::Number(_, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Identifier(_, inferred_type, _)
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
            | Expr::Call(_, _, inferred_type) => {
                if new_inferred_type != InferredType::Unknown {
                    inferred_type.update(new_inferred_type);
                }
            }
        }
    }

    pub fn infer_no_arg_variants(&mut self, function_type_registry: &FunctionTypeRegistry) {
        let mut queue = VecDeque::new();
        queue.push_back(self); // call(x

        while let Some(expr) = queue.pop_back() {
            // call(x)
            match expr {
                Expr::Identifier(name, inferred_type, _) => {
                    // Retrieve the possible no-arg variant from the registry
                    let key = RegistryKey::FunctionName(name.clone());
                    if let Some(values) = function_type_registry.types.get(&key) {
                        for value in values {
                            if let RegistryValue::Function {
                                parameter_types,
                                return_types,
                            } = value
                            {
                                if parameter_types.is_empty() {
                                    // No-arg function found, update the inferred type
                                    *inferred_type = InferredType::Sequence(
                                        return_types.iter().map(|t| t.into()).collect(),
                                    );
                                }
                            }
                        }
                    }
                }
                // Continue for nested expressions
                _ => expr.visit_children(&mut queue),
            }
        }
    }

    fn visit_children(&mut self, queue: &mut VecDeque<&mut Expr>) {
        match self {
            Expr::Let(_, _, expr, _) => queue.push_back(expr),
            Expr::SelectField(expr, _, _) => queue.push_back(expr),
            Expr::SelectIndex(expr, _, _) => queue.push_back(expr),
            Expr::Sequence(exprs, _) => queue.extend(exprs.iter_mut()),
            Expr::Record(exprs, _) => queue.extend(exprs.iter_mut().map(|(_, expr)| expr)),
            Expr::Tuple(exprs, _) => queue.extend(exprs.iter_mut()),
            Expr::Concat(exprs, _) => queue.extend(exprs.iter_mut()),
            Expr::Multiple(exprs, _) => queue.extend(exprs.iter_mut()), // let x = 1, y = call(x);
            Expr::Not(expr, _) => queue.push_back(expr),
            Expr::GreaterThan(lhs, rhs, _) => {
                queue.push_back(lhs);
                queue.push_back(rhs);
            }
            Expr::GreaterThanOrEqualTo(lhs, rhs, _) => {
                queue.push_back(lhs);
                queue.push_back(rhs);
            }
            Expr::LessThanOrEqualTo(lhs, rhs, _) => {
                queue.push_back(lhs);
                queue.push_back(rhs);
            }
            Expr::EqualTo(lhs, rhs, _) => {
                queue.push_back(lhs);
                queue.push_back(rhs);
            }
            Expr::LessThan(lhs, rhs, _) => {
                queue.push_back(lhs);
                queue.push_back(rhs);
            }
            Expr::Cond(cond, then, else_, _) => {
                queue.push_back(cond);
                queue.push_back(then);
                queue.push_back(else_);
            }
            Expr::PatternMatch(expr, arms, _) => {
                queue.push_back(expr);
                queue.extend(arms.iter_mut().map(|arm| &mut arm.0 .1));
            }
            Expr::Option(Some(expr), _) => queue.push_back(expr),
            Expr::Result(Ok(expr), _) => queue.push_back(expr),
            Expr::Result(Err(expr), _) => queue.push_back(expr),
            Expr::Call(_, expressions, _) => queue.extend(expressions.iter_mut()),
            Expr::Literal(_, _) => {}
            Expr::Number(_, _) => {}
            Expr::Flags(_, _) => {}
            Expr::Identifier(_, _, _) => {}
            Expr::Boolean(_, _) => {}
            Expr::Option(None, _) => {}
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

    // Probably good idea to make it just Unknown
    pub fn number(f64: f64) -> Expr {
        Expr::Number(
            Number { value: f64 },
            InferredType::AllOf(vec![
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

// impl TryFrom<golem_api_grpc::proto::golem::rib::Expr> for Expr {
//     type Error = String;
//
//     fn try_from(value: golem_api_grpc::proto::golem::rib::Expr) -> Result<Self, Self::Error> {
//         let expr = value.expr.ok_or("Missing expr")?;
//
//         let expr = match expr {
//             golem_api_grpc::proto::golem::rib::expr::Expr::Let(expr) => {
//                 let name = expr.name;
//                 let expr = *expr.expr.ok_or("Missing expr")?;
//                 Expr::Let(name, Box::new(expr.try_into()?))
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Not(expr) => {
//                 let expr = expr.expr.ok_or("Missing expr")?;
//                 Expr::Not(Box::new((*expr).try_into()?))
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThan(expr) => {
//                 let left = expr.left.ok_or("Missing left expr")?;
//                 let right = expr.right.ok_or("Missing right expr")?;
//                 Expr::GreaterThan(
//                     Box::new((*left).try_into()?),
//                     Box::new((*right).try_into()?),
//                 )
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThanOrEqual(expr) => {
//                 let left = expr.left.ok_or("Missing left expr")?;
//                 let right = expr.right.ok_or("Missing right expr")?;
//                 Expr::GreaterThanOrEqualTo(
//                     Box::new((*left).try_into()?),
//                     Box::new((*right).try_into()?),
//                 )
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::LessThan(expr) => {
//                 let left = expr.left.ok_or("Missing left expr")?;
//                 let right = expr.right.ok_or("Missing right expr")?;
//                 Expr::LessThan(
//                     Box::new((*left).try_into()?),
//                     Box::new((*right).try_into()?),
//                 )
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::LessThanOrEqual(expr) => {
//                 let left = expr.left.ok_or("Missing left expr")?;
//                 let right = expr.right.ok_or("Missing right expr")?;
//                 Expr::LessThanOrEqualTo(
//                     Box::new((*left).try_into()?),
//                     Box::new((*right).try_into()?),
//                 )
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::EqualTo(expr) => {
//                 let left = expr.left.ok_or("Missing left expr")?;
//                 let right = expr.right.ok_or("Missing right expr")?;
//                 Expr::EqualTo(
//                     Box::new((*left).try_into()?),
//                     Box::new((*right).try_into()?),
//                 )
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Cond(expr) => {
//                 let left = expr.left.ok_or("Missing left expr")?;
//                 let cond = expr.cond.ok_or("Missing cond expr")?;
//                 let right = expr.right.ok_or("Missing right expr")?;
//                 Expr::Cond(
//                     Box::new((*left).try_into()?),
//                     Box::new((*cond).try_into()?),
//                     Box::new((*right).try_into()?),
//                 )
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Concat(
//                 golem_api_grpc::proto::golem::rib::ConcatExpr { exprs },
//             ) => {
//                 let exprs: Vec<Expr> = exprs
//                     .into_iter()
//                     .map(|expr| expr.try_into())
//                     .collect::<Result<Vec<_>, _>>()?;
//                 Expr::Concat(exprs)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Multiple(
//                 golem_api_grpc::proto::golem::rib::MultipleExpr { exprs },
//             ) => {
//                 let exprs: Vec<Expr> = exprs
//                     .into_iter()
//                     .map(|expr| expr.try_into())
//                     .collect::<Result<Vec<_>, _>>()?;
//                 Expr::Multiple(exprs)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
//                 golem_api_grpc::proto::golem::rib::SequenceExpr { exprs },
//             ) => {
//                 let exprs: Vec<Expr> = exprs
//                     .into_iter()
//                     .map(|expr| expr.try_into())
//                     .collect::<Result<Vec<_>, _>>()?;
//                 Expr::Sequence(exprs)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Tuple(
//                 golem_api_grpc::proto::golem::rib::TupleExpr { exprs },
//             ) => {
//                 let exprs: Vec<Expr> = exprs
//                     .into_iter()
//                     .map(|expr| expr.try_into())
//                     .collect::<Result<Vec<_>, _>>()?;
//                 Expr::Tuple(exprs)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Record(
//                 golem_api_grpc::proto::golem::rib::RecordExpr { fields },
//             ) => {
//                 let mut values: Vec<(String, Box<Expr>)> = vec![];
//                 for record in fields.into_iter() {
//                     let name = record.name;
//                     let expr = record.expr.ok_or("Missing expr")?;
//                     values.push((name, Box::new(expr.try_into()?)));
//                 }
//                 Expr::Record(values)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Flags(
//                 golem_api_grpc::proto::golem::rib::FlagsExpr { values },
//             ) => Expr::Flags(values),
//             golem_api_grpc::proto::golem::rib::expr::Expr::Literal(
//                 golem_api_grpc::proto::golem::rib::LiteralExpr { value },
//             ) => Expr::Literal(value),
//             golem_api_grpc::proto::golem::rib::expr::Expr::Identifier(
//                 golem_api_grpc::proto::golem::rib::IdentifierExpr { name },
//             ) => Expr::Identifier(name),
//             golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
//                 golem_api_grpc::proto::golem::rib::BooleanExpr { value },
//             ) => Expr::Boolean(value),
//             golem_api_grpc::proto::golem::rib::expr::Expr::Number(expr) => {
//                 Expr::Number(expr.try_into()?)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(expr) => {
//                 let expr = *expr;
//                 let field = expr.field;
//                 let expr = *expr.expr.ok_or(
//                     "Mi\
//                 ssing expr",
//                 )?;
//                 Expr::SelectField(Box::new(expr.try_into()?), field)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndex(expr) => {
//                 let expr = *expr;
//                 let index = expr.index as usize;
//                 let expr = *expr.expr.ok_or("Missing expr")?;
//                 Expr::SelectIndex(Box::new(expr.try_into()?), index)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Option(expr) => match expr.expr {
//                 Some(expr) => Expr::Option(Some(Box::new((*expr).try_into()?))),
//                 None => Expr::Option(None),
//             },
//             golem_api_grpc::proto::golem::rib::expr::Expr::Result(expr) => {
//                 let result = expr.result.ok_or("Missing result")?;
//                 match result {
//                     golem_api_grpc::proto::golem::rib::result_expr::Result::Ok(expr) => {
//                         Expr::Result(Ok(Box::new((*expr).try_into()?)))
//                     }
//                     golem_api_grpc::proto::golem::rib::result_expr::Result::Err(expr) => {
//                         Expr::Result(Err(Box::new((*expr).try_into()?)))
//                     }
//                 }
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::PatternMatch(expr) => {
//                 let patterns: Vec<MatchArm> = expr
//                     .patterns
//                     .into_iter()
//                     .map(|expr| expr.try_into())
//                     .collect::<Result<Vec<_>, _>>()?;
//                 let expr = expr.expr.ok_or("Missing expr")?;
//                 Expr::PatternMatch(Box::new((*expr).try_into()?), patterns)
//             }
//             golem_api_grpc::proto::golem::rib::expr::Expr::Call(expr) => {
//                 let params: Vec<Expr> = expr
//                     .params
//                     .into_iter()
//                     .map(|expr| expr.try_into())
//                     .collect::<Result<Vec<_>, _>>()?;
//                 let name = expr.name.ok_or("Missing name")?;
//                 Expr::Call(name.try_into()?, params)
//             }
//         };
//         Ok(expr)
//     }
// }

// impl From<Expr> for golem_api_grpc::proto::golem::rib::Expr {
//     fn from(value: Expr) -> Self {
//         let expr = match value {
//             Expr::Let(_, _, expr, _) => golem_api_grpc::proto::golem::rib::expr::Expr::Let(Box::new(
//                 golem_api_grpc::proto::golem::rib::LetExpr {
//                     name,
//                     expr: Some(Box::new((*expr).into())),
//                 },
//             )),
//             Expr::SelectField(expr, field, _) => {
//                 golem_api_grpc::proto::golem::rib::expr::Expr::SelectField(Box::new(
//                     golem_api_grpc::proto::golem::rib::SelectFieldExpr {
//                         expr: Some(Box::new((*expr).into())),
//                         field,
//                     },
//                 ))
//             }
//             Expr::SelectIndex(expr, index, _) => {
//                 golem_api_grpc::proto::golem::rib::expr::Expr::SelectIndex(Box::new(
//                     golem_api_grpc::proto::golem::rib::SelectIndexExpr {
//                         expr: Some(Box::new((*expr).into())),
//                         index: index as u64,
//                     },
//                 ))
//             }
//             Expr::Sequence(exprs, _) => golem_api_grpc::proto::golem::rib::expr::Expr::Sequence(
//                 golem_api_grpc::proto::golem::rib::SequenceExpr {
//                     exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
//                 },
//             ),
//             Expr::Record(fields, _) => golem_api_grpc::proto::golem::rib::expr::Expr::Record(
//                 golem_api_grpc::proto::golem::rib::RecordExpr {
//                     fields: fields
//                         .into_iter()
//                         .map(
//                             |(name, expr)| golem_api_grpc::proto::golem::rib::RecordFieldExpr {
//                                 name,
//                                 expr: Some((*expr).into()),
//                             },
//                         )
//                         .collect(),
//                 },
//             ),
//             Expr::Tuple(exprs, _) => golem_api_grpc::proto::golem::rib::expr::Expr::Tuple(
//                 golem_api_grpc::proto::golem::rib::TupleExpr {
//                     exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
//                 },
//             ),
//             Expr::Literal(value, _) => golem_api_grpc::proto::golem::rib::expr::Expr::Literal(
//                 golem_api_grpc::proto::golem::rib::LiteralExpr { value },
//             ),
//             Expr::Number(number, _) => {
//                 golem_api_grpc::proto::golem::rib::expr::Expr::Number(number.into())
//             }
//             Expr::Flags(values, _) => golem_api_grpc::proto::golem::rib::expr::Expr::Flags(
//                 golem_api_grpc::proto::golem::rib::FlagsExpr { values },
//             ),
//             Expr::Identifier(name) => golem_api_grpc::proto::golem::rib::expr::Expr::Identifier(
//                 golem_api_grpc::proto::golem::rib::IdentifierExpr { name },
//             ),
//             Expr::Boolean(value) => golem_api_grpc::proto::golem::rib::expr::Expr::Boolean(
//                 golem_api_grpc::proto::golem::rib::BooleanExpr { value },
//             ),
//             Expr::Concat(exprs) => golem_api_grpc::proto::golem::rib::expr::Expr::Concat(
//                 golem_api_grpc::proto::golem::rib::ConcatExpr {
//                     exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
//                 },
//             ),
//             Expr::Multiple(exprs) => golem_api_grpc::proto::golem::rib::expr::Expr::Multiple(
//                 golem_api_grpc::proto::golem::rib::MultipleExpr {
//                     exprs: exprs.into_iter().map(|expr| expr.into()).collect(),
//                 },
//             ),
//             Expr::Not(expr) => golem_api_grpc::proto::golem::rib::expr::Expr::Not(Box::new(
//                 golem_api_grpc::proto::golem::rib::NotExpr {
//                     expr: Some(Box::new((*expr).into())),
//                 },
//             )),
//             Expr::GreaterThan(left, right) => {
//                 golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThan(Box::new(
//                     golem_api_grpc::proto::golem::rib::GreaterThanExpr {
//                         left: Some(Box::new((*left).into())),
//                         right: Some(Box::new((*right).into())),
//                     },
//                 ))
//             }
//             Expr::GreaterThanOrEqualTo(left, right) => {
//                 golem_api_grpc::proto::golem::rib::expr::Expr::GreaterThanOrEqual(Box::new(
//                     golem_api_grpc::proto::golem::rib::GreaterThanOrEqualToExpr {
//                         left: Some(Box::new((*left).into())),
//                         right: Some(Box::new((*right).into())),
//                     },
//                 ))
//             }
//             Expr::LessThan(left, right) => golem_api_grpc::proto::golem::rib::expr::Expr::LessThan(
//                 Box::new(golem_api_grpc::proto::golem::rib::LessThanExpr {
//                     left: Some(Box::new((*left).into())),
//                     right: Some(Box::new((*right).into())),
//                 }),
//             ),
//             Expr::LessThanOrEqualTo(left, right) => {
//                 golem_api_grpc::proto::golem::rib::expr::Expr::LessThanOrEqual(Box::new(
//                     golem_api_grpc::proto::golem::rib::LessThanOrEqualToExpr {
//                         left: Some(Box::new((*left).into())),
//                         right: Some(Box::new((*right).into())),
//                     },
//                 ))
//             }
//             Expr::EqualTo(left, right) => golem_api_grpc::proto::golem::rib::expr::Expr::EqualTo(
//                 Box::new(golem_api_grpc::proto::golem::rib::EqualToExpr {
//                     left: Some(Box::new((*left).into())),
//                     right: Some(Box::new((*right).into())),
//                 }),
//             ),
//             Expr::Cond(left, cond, right) => golem_api_grpc::proto::golem::rib::expr::Expr::Cond(
//                 Box::new(golem_api_grpc::proto::golem::rib::CondExpr {
//                     left: Some(Box::new((*left).into())),
//                     cond: Some(Box::new((*cond).into())),
//                     right: Some(Box::new((*right).into())),
//                 }),
//             ),
//             Expr::PatternMatch(expr, arms) => {
//                 golem_api_grpc::proto::golem::rib::expr::Expr::PatternMatch(Box::new(
//                     golem_api_grpc::proto::golem::rib::PatternMatchExpr {
//                         expr: Some(Box::new((*expr).into())),
//                         patterns: arms.into_iter().map(|a| a.into()).collect(),
//                     },
//                 ))
//             }
//             Expr::Option(expr) => golem_api_grpc::proto::golem::rib::expr::Expr::Option(Box::new(
//                 golem_api_grpc::proto::golem::rib::OptionExpr {
//                     expr: expr.map(|expr| Box::new((*expr).into())),
//                 },
//             )),
//             Expr::Result(expr) => {
//                 let result = match expr {
//                     Ok(expr) => golem_api_grpc::proto::golem::rib::result_expr::Result::Ok(
//                         Box::new((*expr).into()),
//                     ),
//                     Err(expr) => golem_api_grpc::proto::golem::rib::result_expr::Result::Err(
//                         Box::new((*expr).into()),
//                     ),
//                 };
//
//                 golem_api_grpc::proto::golem::rib::expr::Expr::Result(Box::new(
//                     golem_api_grpc::proto::golem::rib::ResultExpr {
//                         result: Some(result),
//                     },
//                 ))
//             }
//             Expr::Call(function_name, args) => golem_api_grpc::proto::golem::rib::expr::Expr::Call(
//                 golem_api_grpc::proto::golem::rib::CallExpr {
//                     name: Some(function_name.into()),
//                     params: args.into_iter().map(|expr| expr.into()).collect(),
//                 },
//             ),
//         };
//
//         golem_api_grpc::proto::golem::rib::Expr { expr: Some(expr) }
//     }
// }

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct Number {
    pub value: f64, // Change to bigdecimal
}

impl Display for Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Number::Unsigned(value) => write!(f, "{}", value),
            Number::Signed(value) => write!(f, "{}", value),
            Number::Float(value) => write!(f, "{}", value),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::NumberExpr> for Number {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::NumberExpr) -> Result<Self, Self::Error> {
        let number = value.number.ok_or("Missing number")?;
        match number {
            golem_api_grpc::proto::golem::rib::number_expr::Number::Unsigned(value) => {
                Ok(Number::Unsigned(value))
            }
            golem_api_grpc::proto::golem::rib::number_expr::Number::Signed(value) => {
                Ok(Number::Signed(value))
            }
            golem_api_grpc::proto::golem::rib::number_expr::Number::Float(value) => {
                Ok(Number::Float(value))
            }
        }
    }
}

impl From<Number> for golem_api_grpc::proto::golem::rib::NumberExpr {
    fn from(value: Number) -> Self {
        golem_api_grpc::proto::golem::rib::NumberExpr {
            number: Some(match value {
                Number::Unsigned(value) => {
                    golem_api_grpc::proto::golem::rib::number_expr::Number::Unsigned(value)
                }
                Number::Signed(value) => {
                    golem_api_grpc::proto::golem::rib::number_expr::Number::Signed(value)
                }
                Number::Float(value) => {
                    golem_api_grpc::proto::golem::rib::number_expr::Number::Float(value)
                }
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct MatchArm(pub (ArmPattern, Box<Expr>));

impl MatchArm {
    pub fn arm_pattern(&self) -> ArmPattern {
        let arm_pattern = &self.0 .0;
        arm_pattern.clone()
    }

    pub fn arm_expr(&self) -> Expr {
        self.0 .1.deref().clone()
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::MatchArm> for MatchArm {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::MatchArm) -> Result<Self, Self::Error> {
        let pattern = value.pattern.ok_or("Missing pattern")?;
        let expr = value.expr.ok_or("Missing expr")?;
        Ok(MatchArm((pattern.try_into()?, Box::new(expr.try_into()?))))
    }
}

impl From<MatchArm> for golem_api_grpc::proto::golem::rib::MatchArm {
    fn from(value: MatchArm) -> Self {
        let (pattern, expr) = value.0;
        golem_api_grpc::proto::golem::rib::MatchArm {
            pattern: Some(pattern.into()),
            expr: Some((*expr).into()),
        }
    }
}

// Ex: Some(x)
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ArmPattern {
    WildCard,
    As(String, Box<ArmPattern>),
    Constructor(String, Vec<ArmPattern>),
    Literal(Box<Expr>),
}

impl ArmPattern {
    // Helper to construct ok(v). Cannot be used if there is nested constructors such as ok(some(v)))
    pub fn ok(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Result(
            Ok(Box::new(Expr::Identifier(
                binding_variable.to_string(),
                InferredType::Unknown,
                VariableId::init(),
            ))),
            InferredType::Unknown,
        )))
    }

    // Helper to construct err(v). Cannot be used if there is nested constructors such as err(some(v)))
    pub fn err(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Result(
            Err(Box::new(Expr::Identifier(
                binding_variable.to_string(),
                InferredType::Unknown,
                VariableId::init(),
            ))),
            InferredType::Unknown,
        )))
    }

    // Helper to construct some(v). Cannot be used if there is nested constructors such as some(ok(v)))
    pub fn some(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option(
            Some(Box::new(Expr::Identifier(
                binding_variable.to_string(),
                InferredType::Unknown,
                VariableId::init(),
            ))),
            InferredType::Unknown,
        )))
    }

    pub fn none() -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Option(None, InferredType::Unknown)))
    }

    pub fn identifier(binding_variable: &str) -> ArmPattern {
        ArmPattern::Literal(Box::new(Expr::Identifier(
            binding_variable.to_string(),
            InferredType::Unknown,
            VariableId::init(),
        )))
    }
    pub fn custom_constructor(name: &str, args: Vec<ArmPattern>) -> ArmPattern {
        ArmPattern::Constructor(name.to_string(), args)
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

mod internal {
    use crate::expr::{InferredType, VariableId};
    use std::collections::HashMap;

    // let x = "foo"; // variable_id incremented to 1
    // let x = "bar"; // variable_id incremented to 2
    // call(x)
    // Current variable identity when calling with x will be <x -> 2>
    // Similarly
    // let x = "foo"; // set variable_id to be 1
    // call(x); // At this point the variable_id of x will b 1
    // let x = "bar"; // By this point the variable_id of x will be permanently changed to 2
    // call(x) // The variable_identity of this x is 2.
    pub struct IdentifierVariableIdState(HashMap<String, VariableId>);

    impl IdentifierVariableIdState {
        pub fn new() -> Self {
            IdentifierVariableIdState(HashMap::new())
        }

        pub fn update(&mut self, identifier: &str, id: VariableId) {
            self.0.insert(identifier.to_string(), id);
        }

        pub fn lookup(&self, identifier: &str) -> Option<VariableId> {
            self.0.get(identifier).cloned()
        }
    }

    // A state that maps from the identifers to the types inferred
    pub struct IdentifierTypeState(HashMap<(String, VariableId), InferredType>);

    impl IdentifierTypeState {
        pub fn new() -> Self {
            IdentifierTypeState(HashMap::new())
        }

        pub fn update(&mut self, identifier: &str, id: VariableId, ty: InferredType) {
            self.0
                .entry((identifier.to_string(), id))
                .and_modify(|e| e.update(ty.clone()))
                .or_insert(ty);
        }

        pub fn lookup(&self, identifier: &str, id: VariableId) -> Option<InferredType> {
            self.0.get(&(identifier.to_string(), id)).cloned()
        }
    }
}
