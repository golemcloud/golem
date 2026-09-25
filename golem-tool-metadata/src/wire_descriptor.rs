// Copyright 2024-2026 Golem Cloud
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

use crate::extended_tool_type::*;
use crate::{ToolBuildError, ToolLiteral, ToolSchemaRepr};
use golem_schema::schema::tool::wit::wire as tool;
use golem_schema::schema::wit::direct::{WireSchema, WireSchemaBuilder, WireWriter};
use golem_schema::schema::wit::wire;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Default, Clone)]
pub struct WireToolSchema(Rc<RefCell<WireSchemaBuilder>>);

impl WireToolSchema {
    pub fn schema<T: WireSchema>(&self) -> WireTypeRef {
        let index = T::append_schema(&mut self.0.borrow_mut());
        self.reference(index)
    }

    pub fn reference(&self, index: wire::TypeNodeIndex) -> WireTypeRef {
        WireTypeRef {
            index,
            arena: self.clone(),
        }
    }

    pub fn with_builder<T>(&self, f: impl FnOnce(&mut WireSchemaBuilder) -> T) -> T {
        f(&mut self.0.borrow_mut())
    }

    pub fn error_cases(&self, cases: Vec<tool::ErrorCase>) -> Vec<ExtendedErrorCase<WireTypeRef>> {
        cases
            .into_iter()
            .map(|case| ExtendedErrorCase {
                name: case.name,
                doc: (&case.doc).into(),
                kind: (&case.kind).into(),
                exit_code: case.exit_code,
                payload: case.payload.map(|index| self.reference(index)),
            })
            .collect()
    }

    pub fn finish(self) -> wire::SchemaGraph {
        let mut builder = Rc::try_unwrap(self.0)
            .unwrap_or_else(|_| panic!("tool schema references must be lowered before finishing"))
            .into_inner();
        let root = builder.push(wire::SchemaTypeBody::RecordType(Vec::new()));
        builder.finish(root)
    }
}

#[derive(Clone)]
pub struct WireTypeRef {
    pub index: wire::TypeNodeIndex,
    arena: WireToolSchema,
}

impl std::fmt::Debug for WireTypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WireTypeRef").field(&self.index).finish()
    }
}

/// A metadata literal has no resources. Keeping its concrete type and authored
/// literal makes composition clonable without cloning affine WIT value trees.
#[derive(Clone, Debug)]
pub struct WireMetadataValue {
    type_: WireTypeRef,
    literal: ToolLiteral,
}

impl WireMetadataValue {
    pub fn into_wire(self) -> wire::SchemaValueTree {
        self.type_
            .encode_literal(&self.literal)
            .expect("metadata literal was type checked when constructed")
    }
}

impl WireTypeRef {
    pub fn literal(&self, literal: &ToolLiteral) -> Result<WireMetadataValue, ToolBuildError> {
        self.encode_literal(literal)?;
        Ok(WireMetadataValue {
            type_: self.clone(),
            literal: literal.clone(),
        })
    }

    fn encode_literal(
        &self,
        literal: &ToolLiteral,
    ) -> Result<wire::SchemaValueTree, ToolBuildError> {
        let mut writer = WireWriter::default();
        let root = literal_node(
            &self.arena.0.borrow(),
            self.index,
            literal,
            &mut writer,
            128,
        )?;
        Ok(writer.finish(root))
    }

    fn refined(
        &self,
        refine: impl FnOnce(&mut wire::SchemaTypeBody) -> Result<(), ToolBuildError>,
    ) -> Result<Self, ToolBuildError> {
        let mut arena = self.arena.0.borrow_mut();
        let node = arena.node(self.index).expect("generated schema index");
        let mut body = node.body.clone();
        let metadata = node.metadata.clone();
        refine(&mut body)?;
        let index = arena.push_with_metadata(body, metadata);
        Ok(self.arena.reference(index))
    }

    pub fn refine_text(
        &self,
        regex: Option<String>,
        min: Option<u32>,
        max: Option<u32>,
    ) -> Result<Self, ToolBuildError> {
        self.refined(|body| {
            if matches!(body, wire::SchemaTypeBody::StringType) {
                *body = wire::SchemaTypeBody::TextType(wire::TextRestrictions {
                    languages: None,
                    regex: None,
                    min_length: None,
                    max_length: None,
                });
            }
            let wire::SchemaTypeBody::TextType(restrictions) = body else {
                return Err(refinement_mismatch("text", body));
            };
            if regex.is_some() {
                restrictions.regex = regex;
            }
            if min.is_some() {
                restrictions.min_length = min;
            }
            if max.is_some() {
                restrictions.max_length = max;
            }
            Ok(())
        })
    }

    pub fn refine_path(
        &self,
        direction: Option<golem_schema::schema::PathDirection>,
        kind: Option<golem_schema::schema::PathKind>,
        mime: Option<Vec<String>>,
    ) -> Result<Self, ToolBuildError> {
        self.refined(|body| {
            let wire::SchemaTypeBody::PathType(spec) = body else {
                return Err(refinement_mismatch("path", body));
            };
            if let Some(value) = direction {
                spec.direction = match value {
                    golem_schema::schema::PathDirection::Input => wire::PathDirection::Input,
                    golem_schema::schema::PathDirection::Output => wire::PathDirection::Output,
                    golem_schema::schema::PathDirection::InOut => wire::PathDirection::InOut,
                };
            }
            if let Some(value) = kind {
                spec.kind = match value {
                    golem_schema::schema::PathKind::File => wire::PathKind::File,
                    golem_schema::schema::PathKind::Directory => wire::PathKind::Directory,
                    golem_schema::schema::PathKind::Any => wire::PathKind::Any,
                };
            }
            if mime.is_some() {
                spec.allowed_mime_types = mime;
            }
            Ok(())
        })
    }

    pub fn refine_url(&self, schemes: Option<Vec<String>>) -> Result<Self, ToolBuildError> {
        self.refined(|body| {
            let wire::SchemaTypeBody::UrlType(restrictions) = body else {
                return Err(refinement_mismatch("url", body));
            };
            if schemes.is_some() {
                restrictions.allowed_schemes = schemes;
            }
            Ok(())
        })
    }

    pub fn refine_numeric(
        &self,
        min: Option<golem_schema::schema::schema_type::NumericBound>,
        max: Option<golem_schema::schema::schema_type::NumericBound>,
        unit: Option<String>,
    ) -> Result<Self, ToolBuildError> {
        self.refined(|body| {
            use wire::SchemaTypeBody::*;
            let restrictions = match body {
                S8Type(r) | S16Type(r) | S32Type(r) | S64Type(r) | U8Type(r) | U16Type(r)
                | U32Type(r) | U64Type(r) | F32Type(r) | F64Type(r) => r,
                _ => return Err(refinement_mismatch("numeric", body)),
            };
            if min.is_none() && max.is_none() && unit.is_none() {
                return Ok(());
            }
            let restrictions = restrictions.get_or_insert(wire::NumericRestrictions {
                min: None,
                max: None,
                unit: None,
            });
            if let Some(value) = min {
                restrictions.min = Some(numeric_bound(value));
            }
            if let Some(value) = max {
                restrictions.max = Some(numeric_bound(value));
            }
            if unit.is_some() {
                restrictions.unit = unit;
            }
            Ok(())
        })
    }
}

impl ToolSchemaRepr for WireTypeRef {
    type Value = WireMetadataValue;

    fn list(&self) -> Self {
        let index = self
            .arena
            .0
            .borrow_mut()
            .push(wire::SchemaTypeBody::ListType(self.index));
        self.arena.reference(index)
    }

    fn map_value(&self) -> Option<Self> {
        let arena = self.arena.0.borrow();
        let wire::SchemaTypeBody::MapType(spec) = &arena.resolve(self.index)?.body else {
            return None;
        };
        Some(self.arena.reference(spec.value))
    }

    fn shape_matches(&self, other: &Self) -> bool {
        shapes_match(
            &self.arena.0.borrow(),
            self.index,
            &other.arena.0.borrow(),
            other.index,
            &mut HashSet::new(),
            32,
        )
    }

    fn is_sound(&self) -> bool {
        true
    }

    fn value_is_literal(
        &self,
        literal: &ToolLiteral,
        whole_or_one_peel: bool,
    ) -> Result<Self::Value, ToolBuildError> {
        let error = match self.literal(literal) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if !whole_or_one_peel {
            return Err(error);
        }
        let arena = self.arena.0.borrow();
        let mut index = self.index;
        for _ in 0..128 {
            match arena.resolve(index).map(|node| &node.body) {
                Some(wire::SchemaTypeBody::OptionType(inner)) => index = *inner,
                Some(wire::SchemaTypeBody::ListType(inner)) => {
                    return self.arena.reference(*inner).literal(literal);
                }
                Some(wire::SchemaTypeBody::FixedListType(spec)) => {
                    return self.arena.reference(spec.element).literal(literal);
                }
                Some(wire::SchemaTypeBody::MapType(spec)) => {
                    return self.arena.reference(spec.value).literal(literal);
                }
                _ => break,
            }
        }
        Err(error)
    }
}

fn refinement_mismatch(refinement: &'static str, body: &wire::SchemaTypeBody) -> ToolBuildError {
    use wire::SchemaTypeBody::*;
    let actual = match body {
        RefType(_) => "ref",
        BoolType => "bool",
        CharType => "char",
        StringType => "string",
        TextType(_) => "text",
        PathType(_) => "path",
        UrlType(_) => "url",
        RecordType(_) => "record",
        ListType(_) => "list",
        OptionType(_) => "option",
        _ => "non-matching wire type",
    };
    ToolBuildError::RefinementTypeMismatch { refinement, actual }
}

fn literal_node(
    arena: &WireSchemaBuilder,
    index: i32,
    literal: &ToolLiteral,
    writer: &mut WireWriter,
    depth: usize,
) -> Result<i32, ToolBuildError> {
    use ToolLiteral as L;
    use wire::SchemaTypeBody as T;
    use wire::SchemaValueNode as V;
    let mismatch = || {
        ToolBuildError::DefaultTypeMismatch(format!(
            "literal {literal:?} does not match type node {index}"
        ))
    };
    if depth == 0 {
        return Err(mismatch());
    }
    let body = &arena.resolve(index).ok_or_else(mismatch)?.body;
    macro_rules! integer {
        ($value:expr, $ty:ty, $variant:ident) => {
            V::$variant(<$ty>::try_from(*$value).map_err(|_| mismatch())?)
        };
    }
    let node = match (body, literal) {
        (T::BoolType, L::Bool(value)) => V::BoolValue(*value),
        (T::S8Type(_), L::Int(value)) => integer!(value, i8, S8Value),
        (T::S16Type(_), L::Int(value)) => integer!(value, i16, S16Value),
        (T::S32Type(_), L::Int(value)) => integer!(value, i32, S32Value),
        (T::S64Type(_), L::Int(value)) => integer!(value, i64, S64Value),
        (T::U8Type(_), L::Int(value)) => integer!(value, u8, U8Value),
        (T::U16Type(_), L::Int(value)) => integer!(value, u16, U16Value),
        (T::U32Type(_), L::Int(value)) => integer!(value, u32, U32Value),
        (T::U64Type(_), L::Int(value)) => integer!(value, u64, U64Value),
        (T::F32Type(_), L::Float(value)) => V::F32Value(*value as f32),
        (T::F32Type(_), L::Int(value)) => V::F32Value(*value as f32),
        (T::F64Type(_), L::Float(value)) => V::F64Value(*value),
        (T::F64Type(_), L::Int(value)) => V::F64Value(*value as f64),
        (T::CharType, L::Char(value)) => V::CharValue(*value),
        (T::StringType, L::Str(value)) => V::StringValue(value.clone()),
        (T::TextType(_), L::Str(value)) => V::TextValue(wire::TextValuePayload {
            text: value.clone(),
            language: None,
        }),
        (T::PathType(_), L::Str(value)) => V::PathValue(value.clone()),
        (T::UrlType(_), L::Str(value)) => V::UrlValue(value.clone()),
        (T::EnumType(cases), L::Str(value)) => V::EnumValue(
            cases
                .iter()
                .position(|case| case == value)
                .ok_or_else(mismatch)? as u32,
        ),
        (T::OptionType(inner), _) => V::OptionValue(Some(literal_node(
            arena,
            *inner,
            literal,
            writer,
            depth - 1,
        )?)),
        (T::ListType(element), L::List(items)) => V::ListValue(
            items
                .iter()
                .map(|item| literal_node(arena, *element, item, writer, depth - 1))
                .collect::<Result<_, _>>()?,
        ),
        (T::FixedListType(spec), L::List(items)) if items.len() == spec.length as usize => {
            V::FixedListValue(
                items
                    .iter()
                    .map(|item| literal_node(arena, spec.element, item, writer, depth - 1))
                    .collect::<Result<_, _>>()?,
            )
        }
        (T::MapType(spec), L::Map(items)) => V::MapValue(
            items
                .iter()
                .map(|(key, value)| {
                    Ok(wire::MapEntry {
                        key: literal_node(arena, spec.key, key, writer, depth - 1)?,
                        value: literal_node(arena, spec.value, value, writer, depth - 1)?,
                    })
                })
                .collect::<Result<_, ToolBuildError>>()?,
        ),
        (T::MapType(_), L::List(items)) if items.is_empty() => V::MapValue(Vec::new()),
        _ => return Err(mismatch()),
    };
    Ok(writer.push(node))
}

fn shapes_match(
    a: &WireSchemaBuilder,
    ai: i32,
    b: &WireSchemaBuilder,
    bi: i32,
    seen: &mut HashSet<(i32, i32)>,
    depth: usize,
) -> bool {
    use wire::SchemaTypeBody::*;
    let (Some(an), Some(bn)) = (a.resolve(ai), b.resolve(bi)) else {
        return false;
    };
    if !seen.insert((ai, bi)) {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let mut compare = |x, y| shapes_match(a, x, b, y, seen, depth - 1);
    match (&an.body, &bn.body) {
        (ListType(x), ListType(y)) | (OptionType(x), OptionType(y)) => compare(*x, *y),
        (FixedListType(x), FixedListType(y)) => {
            x.length == y.length && compare(x.element, y.element)
        }
        (MapType(x), MapType(y)) => compare(x.key, y.key) && compare(x.value, y.value),
        (TupleType(x), TupleType(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| compare(*x, *y))
        }
        (RecordType(x), RecordType(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|(x, y)| x.name == y.name && compare(x.body, y.body))
        }
        (VariantType(x), VariantType(y)) => {
            x.len() == y.len()
                && x.iter().zip(y).all(|(x, y)| {
                    x.name == y.name
                        && match (x.payload, y.payload) {
                            (None, None) => true,
                            (Some(x), Some(y)) => compare(x, y),
                            _ => false,
                        }
                })
        }
        (UnionType(x), UnionType(y)) => {
            x.branches.len() == y.branches.len()
                && x.branches.iter().zip(&y.branches).all(|(x, y)| {
                    x.tag == y.tag
                        && discriminators_match(&x.discriminator, &y.discriminator)
                        && compare(x.body, y.body)
                })
        }
        (EnumType(x), EnumType(y)) | (FlagsType(x), FlagsType(y)) => x == y,
        (TextType(x), TextType(y)) => optional_sets_match(&x.languages, &y.languages),
        (BinaryType(x), BinaryType(y)) => optional_sets_match(&x.mime_types, &y.mime_types),
        (StringType, TextType(x)) | (TextType(x), StringType) => x.languages.is_none(),
        (QuantityType(x), QuantityType(y)) => {
            x.base_unit == y.base_unit
                && units_match(&x.base_unit, &x.allowed_suffixes, &y.allowed_suffixes)
        }
        (SecretType(x), SecretType(y)) => x.category == y.category && compare(x.inner, y.inner),
        (QuotaTokenType(x), QuotaTokenType(y)) => x.resource_name == y.resource_name,
        (PermissionCardType(x), PermissionCardType(y)) => x.polymorphic == y.polymorphic,
        (ResultType(x), ResultType(y)) => {
            [(x.ok, y.ok), (x.err, y.err)]
                .into_iter()
                .all(|(x, y)| match (x, y) {
                    (None, None) => true,
                    (Some(x), Some(y)) => compare(x, y),
                    _ => false,
                })
        }
        (FutureType(x), FutureType(y)) | (StreamType(x), StreamType(y)) => match (x, y) {
            (None, None) => true,
            (Some(x), Some(y)) => compare(*x, *y),
            _ => false,
        },
        _ => std::mem::discriminant(&an.body) == std::mem::discriminant(&bn.body),
    }
}

fn optional_sets_match(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.iter().collect::<HashSet<_>>() == b.iter().collect::<HashSet<_>>(),
        _ => false,
    }
}

fn units_match(base: &str, a: &[String], b: &[String]) -> bool {
    let units = |suffixes: &[String]| {
        if suffixes.is_empty() {
            HashSet::from([base.to_string()])
        } else {
            suffixes.iter().cloned().collect()
        }
    };
    units(a) == units(b)
}

fn discriminators_match(a: &wire::DiscriminatorRule, b: &wire::DiscriminatorRule) -> bool {
    use wire::DiscriminatorRule::*;
    match (a, b) {
        (Prefix(a), Prefix(b))
        | (Suffix(a), Suffix(b))
        | (Contains(a), Contains(b))
        | (Regex(a), Regex(b))
        | (FieldAbsent(a), FieldAbsent(b)) => a == b,
        (FieldEquals(a), FieldEquals(b)) => a.field_name == b.field_name && a.literal == b.literal,
        _ => false,
    }
}

fn numeric_bound(bound: golem_schema::schema::schema_type::NumericBound) -> wire::NumericBound {
    use golem_schema::schema::schema_type::NumericBound;
    match bound {
        NumericBound::Signed(value) => wire::NumericBound::Signed(value),
        NumericBound::Unsigned(value) => wire::NumericBound::Unsigned(value),
        NumericBound::FloatBits(value) => wire::NumericBound::FloatBits(value),
    }
}

impl ExtendedToolType<WireTypeRef> {
    pub fn into_wire(self, schema: WireToolSchema) -> Result<tool::Tool, ToolBuildError> {
        let nodes = self
            .commands
            .into_iter()
            .map(lower_command)
            .collect::<Result<_, _>>()?;
        Ok(tool::Tool {
            version: self.version,
            commands: tool::CommandTree { nodes },
            schema: schema.finish(),
        })
    }
}

fn lower_command(
    node: ExtendedCommandNode<WireTypeRef>,
) -> Result<tool::CommandNode, ToolBuildError> {
    Ok(tool::CommandNode {
        name: node.name,
        aliases: node.aliases,
        doc: (&node.doc).into(),
        globals: tool::Globals {
            options: node.globals.options.into_iter().map(lower_option).collect(),
            flags: node.globals.flags.iter().map(Into::into).collect(),
        },
        subcommands: node.subcommands,
        body: node.body.map(lower_body).transpose()?,
    })
}

fn lower_option(value: ExtendedOptionSpec<WireTypeRef>) -> tool::OptionSpec {
    tool::OptionSpec {
        long: value.long,
        short: value.short,
        aliases: value.aliases,
        doc: (&value.doc).into(),
        value_name: value.value_name,
        shape: match value.shape {
            ExtendedOptionShape::Scalar(t) => tool::OptionShape::Scalar(t.index),
            ExtendedOptionShape::OptionalScalar(t) => tool::OptionShape::OptionalScalar(t.index),
            ExtendedOptionShape::RepeatableList(t) => {
                tool::OptionShape::RepeatableList(tool::RepeatableListShape {
                    repetition: (&t.repetition).into(),
                    item_type: t.item_type.index,
                })
            }
            ExtendedOptionShape::RepeatableMap(t) => {
                tool::OptionShape::RepeatableMap(tool::RepeatableMapShape {
                    repetition: (&t.repetition).into(),
                    map_type: t.map_type.index,
                    duplicate_key_policy: (&t.duplicate_key_policy).into(),
                })
            }
        },
        default: value.default.map(WireMetadataValue::into_wire),
        required: value.required,
        env_var: value.env_var,
    }
}

fn lower_body(body: ExtendedCommandBody<WireTypeRef>) -> Result<tool::CommandBody, ToolBuildError> {
    Ok(tool::CommandBody {
        positionals: tool::Positionals {
            fixed: body
                .positionals
                .fixed
                .into_iter()
                .map(|p| tool::Positional {
                    name: p.name,
                    doc: (&p.doc).into(),
                    value_name: p.value_name,
                    type_: p.type_.index,
                    default: p.default.map(WireMetadataValue::into_wire),
                    required: p.required,
                    accepts_stdio: p.accepts_stdio,
                })
                .collect(),
            tail: body.positionals.tail.map(|p| tool::TailPositional {
                name: p.name,
                doc: (&p.doc).into(),
                value_name: p.value_name,
                item_type: p.item_type.index,
                min: p.min,
                max: p.max,
                separator: p.separator,
                verbatim: p.verbatim,
                accepts_stdio: p.accepts_stdio,
            }),
        },
        options: body.options.into_iter().map(lower_option).collect(),
        flags: body.flags.iter().map(Into::into).collect(),
        constraints: body
            .constraints
            .into_iter()
            .map(lower_constraint)
            .collect::<Result<_, _>>()?,
        stdin: body.stdin.as_ref().map(Into::into),
        stdout: body.stdout.as_ref().map(Into::into),
        result: body.result.map(|r| tool::ResultSpec {
            type_: r.type_.index,
            doc: (&r.doc).into(),
            formatters: r.formatters.iter().map(Into::into).collect(),
            default_formatter: r.default_formatter,
        }),
        errors: body
            .errors
            .into_iter()
            .map(|e| tool::ErrorCase {
                name: e.name,
                doc: (&e.doc).into(),
                kind: (&e.kind).into(),
                exit_code: e.exit_code,
                payload: e.payload.map(|t| t.index),
            })
            .collect(),
        annotations: body.annotations.as_ref().map(Into::into),
    })
}

fn lower_refs(refs: Vec<ExtendedRef<WireTypeRef>>) -> Result<Vec<tool::Ref>, ToolBuildError> {
    refs.into_iter()
        .map(|r| {
            Ok(match r {
                ExtendedRef::Present(name) => tool::Ref::Present(name),
                ExtendedRef::ValueIs(value) => match value.value {
                    ExtendedValueIsLiteral::Resolved(literal) => {
                        tool::Ref::ValueIs(tool::ValueIsRef {
                            name: value.name,
                            value: literal.into_wire(),
                        })
                    }
                    ExtendedValueIsLiteral::Deferred(_) => {
                        return Err(ToolBuildError::UnresolvedValueIsLiteral(value.name));
                    }
                },
            })
        })
        .collect()
}

fn lower_constraint(
    value: ExtendedConstraint<WireTypeRef>,
) -> Result<tool::Constraint, ToolBuildError> {
    Ok(match value {
        ExtendedConstraint::RequiresAll(refs) => tool::Constraint::RequiresAll(lower_refs(refs)?),
        ExtendedConstraint::AllOrNone(refs) => tool::Constraint::AllOrNone(lower_refs(refs)?),
        ExtendedConstraint::RequiresAny(refs) => tool::Constraint::RequiresAny(lower_refs(refs)?),
        ExtendedConstraint::MutexGroups(groups) => tool::Constraint::MutexGroups(
            groups
                .into_iter()
                .map(|g| {
                    Ok(tool::RefGroup {
                        refs: lower_refs(g.refs)?,
                    })
                })
                .collect::<Result<_, ToolBuildError>>()?,
        ),
        ExtendedConstraint::Implies(c) => tool::Constraint::Implies(tool::ImpliesC {
            lhs_quant: (&c.lhs_quant).into(),
            lhs: lower_refs(c.lhs)?,
            rhs_quant: (&c.rhs_quant).into(),
            rhs: lower_refs(c.rhs)?,
        }),
        ExtendedConstraint::Forbids(c) => tool::Constraint::Forbids(tool::ForbidsC {
            lhs_quant: (&c.lhs_quant).into(),
            lhs: lower_refs(c.lhs)?,
            rhs: lower_refs(c.rhs)?,
        }),
    })
}
