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

use super::lexer::{LexError, Lexer, Token};
use golem_common::model::agent::{
    BinaryReference, ComponentModelElementValue, DataSchema, DataValue, ElementSchema,
    ElementValue, ElementValues, NamedElementSchema, NamedElementValue, NamedElementValues,
    TextReference, UnstructuredBinaryElementValue, UnstructuredTextElementValue,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub position: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Parse error at position {}: {}",
            self.position, self.message
        )
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError {
            position: e.position,
            message: e.message,
        }
    }
}

/// Trait for language-specific parsing behaviour.
///
/// The shared framework handles the top-level `DataSchema` dispatch,
/// element dispatch, and common primitives (bool, integers, floats, strings).
/// Dialect implementations provide language-specific parsing for complex
/// CM types, unstructured elements, and field-name normalisation.
pub(super) trait Dialect: Sized {
    /// Normalise a schema field name to the dialect's casing convention
    /// (e.g. `snake_case` for Rust, `lowerCamelCase` for TypeScript).
    fn normalize_field_name(name: &str) -> String;

    // ── Complex CM value parsing (dialect-specific syntax) ──────────

    fn parse_char(lexer: &mut Lexer) -> Result<char, ParseError>;

    fn parse_tuple(
        lexer: &mut Lexer,
        tt: &golem_wasm::analysis::TypeTuple,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError>;

    fn parse_record(
        lexer: &mut Lexer,
        tr: &golem_wasm::analysis::TypeRecord,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError>;

    fn parse_variant(
        lexer: &mut Lexer,
        tv: &golem_wasm::analysis::TypeVariant,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError>;

    fn parse_enum(
        lexer: &mut Lexer,
        te: &golem_wasm::analysis::TypeEnum,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError>;

    fn parse_option(
        lexer: &mut Lexer,
        to: &golem_wasm::analysis::TypeOption,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError>;

    fn parse_result(
        lexer: &mut Lexer,
        tr: &golem_wasm::analysis::TypeResult,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError>;

    fn parse_flags(
        lexer: &mut Lexer,
        tf: &golem_wasm::analysis::TypeFlags,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError>;

    fn parse_unstructured_text(lexer: &mut Lexer) -> Result<TextReference, ParseError>;
    fn parse_unstructured_binary(lexer: &mut Lexer) -> Result<BinaryReference, ParseError>;
}

// ── Shared entry point ──────────────────────────────────────────────────────

pub(super) fn parse_data_value<D: Dialect>(
    input: &str,
    schema: &DataSchema,
) -> Result<DataValue, ParseError> {
    let mut lexer = Lexer::new(input);
    let result = match schema {
        DataSchema::Tuple(schemas) => {
            let mut elements = Vec::with_capacity(schemas.elements.len());
            for (i, s) in schemas.elements.iter().enumerate() {
                if i > 0 {
                    lexer.expect(&Token::Comma)?;
                }
                elements.push(parse_element::<D>(&mut lexer, &s.schema)?);
            }
            DataValue::Tuple(ElementValues { elements })
        }
        DataSchema::Multimodal(schemas) => {
            let elements = parse_named_elements::<D>(&mut lexer, &schemas.elements)?;
            DataValue::Multimodal(NamedElementValues { elements })
        }
    };
    let (tok, pos, _) = lexer.next_token()?;
    if tok != Token::Eof {
        return Err(ParseError {
            position: pos,
            message: format!("expected end of input, got {tok:?}"),
        });
    }
    Ok(result)
}

fn parse_named_elements<D: Dialect>(
    lexer: &mut Lexer,
    schemas: &[NamedElementSchema],
) -> Result<Vec<NamedElementValue>, ParseError> {
    let name_map: Vec<(String, usize)> = schemas
        .iter()
        .enumerate()
        .map(|(i, s)| (D::normalize_field_name(&s.name), i))
        .collect();
    let mut elements = Vec::new();
    let mut first = true;
    while *lexer.peek()? != Token::Eof {
        if !first {
            lexer.expect(&Token::Comma)?;
            if *lexer.peek()? == Token::Eof {
                break;
            }
        }
        first = false;
        let (key, pos, _) = lexer.expect_ident()?;
        lexer.expect(&Token::Colon)?;
        let normalized_key = D::normalize_field_name(&key);
        let (_, idx) = name_map
            .iter()
            .find(|(n, _)| *n == normalized_key)
            .ok_or_else(|| ParseError {
                position: pos,
                message: format!("unknown field '{key}'"),
            })?;
        let schema = &schemas[*idx];
        let value = parse_element::<D>(lexer, &schema.schema)?;
        elements.push(NamedElementValue {
            name: schema.name.clone(),
            value,
            schema_index: *idx as u32,
        });
    }
    Ok(elements)
}

// ── Element dispatch ────────────────────────────────────────────────────────

pub(super) fn parse_element<D: Dialect>(
    lexer: &mut Lexer,
    schema: &ElementSchema,
) -> Result<ElementValue, ParseError> {
    match schema {
        ElementSchema::ComponentModel(cm) => {
            let value = parse_cm_value::<D>(lexer, &cm.element_type)?;
            Ok(ElementValue::ComponentModel(ComponentModelElementValue {
                value,
            }))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let value = D::parse_unstructured_text(lexer)?;
            Ok(ElementValue::UnstructuredText(
                UnstructuredTextElementValue {
                    value,
                    descriptor: descriptor.clone(),
                },
            ))
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let value = D::parse_unstructured_binary(lexer)?;
            Ok(ElementValue::UnstructuredBinary(
                UnstructuredBinaryElementValue {
                    value,
                    descriptor: descriptor.clone(),
                },
            ))
        }
    }
}

// ── CM value parsing (shared primitives + dialect delegation) ───────────────

pub(super) fn parse_cm_value<D: Dialect>(
    lexer: &mut Lexer,
    typ: &AnalysedType,
) -> Result<ValueAndType, ParseError> {
    match typ {
        AnalysedType::Bool(_) => {
            let (tok, pos, _) = lexer.next_token()?;
            match tok {
                Token::BoolLit(b) => Ok(ValueAndType::new(Value::Bool(b), typ.clone())),
                _ => Err(perr(pos, "expected boolean")),
            }
        }
        AnalysedType::U8(_) => {
            parse_uint(lexer).map(|v| ValueAndType::new(Value::U8(v as u8), typ.clone()))
        }
        AnalysedType::U16(_) => {
            parse_uint(lexer).map(|v| ValueAndType::new(Value::U16(v as u16), typ.clone()))
        }
        AnalysedType::U32(_) => {
            parse_uint(lexer).map(|v| ValueAndType::new(Value::U32(v as u32), typ.clone()))
        }
        AnalysedType::U64(_) => {
            parse_uint(lexer).map(|v| ValueAndType::new(Value::U64(v), typ.clone()))
        }
        AnalysedType::S8(_) => {
            parse_int(lexer).map(|v| ValueAndType::new(Value::S8(v as i8), typ.clone()))
        }
        AnalysedType::S16(_) => {
            parse_int(lexer).map(|v| ValueAndType::new(Value::S16(v as i16), typ.clone()))
        }
        AnalysedType::S32(_) => {
            parse_int(lexer).map(|v| ValueAndType::new(Value::S32(v as i32), typ.clone()))
        }
        AnalysedType::S64(_) => {
            parse_int(lexer).map(|v| ValueAndType::new(Value::S64(v), typ.clone()))
        }
        AnalysedType::F32(_) => {
            parse_float(lexer).map(|v| ValueAndType::new(Value::F32(v as f32), typ.clone()))
        }
        AnalysedType::F64(_) => {
            parse_float(lexer).map(|v| ValueAndType::new(Value::F64(v), typ.clone()))
        }
        AnalysedType::Chr(_) => {
            let ch = D::parse_char(lexer)?;
            Ok(ValueAndType::new(Value::Char(ch), typ.clone()))
        }
        AnalysedType::Str(_) => {
            let (s, _, _) = lexer.expect_string()?;
            Ok(ValueAndType::new(Value::String(s), typ.clone()))
        }
        AnalysedType::List(tl) => {
            lexer.expect(&Token::LBrack)?;
            let mut items = Vec::new();
            while *lexer.peek()? != Token::RBrack {
                if !items.is_empty() {
                    lexer.expect(&Token::Comma)?;
                    if *lexer.peek()? == Token::RBrack {
                        break;
                    }
                }
                items.push(parse_cm_value::<D>(lexer, &tl.inner)?.value);
            }
            lexer.expect(&Token::RBrack)?;
            Ok(ValueAndType::new(Value::List(items), typ.clone()))
        }
        AnalysedType::Tuple(tt) => D::parse_tuple(lexer, tt, typ),
        AnalysedType::Record(tr) => D::parse_record(lexer, tr, typ),
        AnalysedType::Variant(tv) => D::parse_variant(lexer, tv, typ),
        AnalysedType::Enum(te) => D::parse_enum(lexer, te, typ),
        AnalysedType::Option(to) => D::parse_option(lexer, to, typ),
        AnalysedType::Result(tr) => D::parse_result(lexer, tr, typ),
        AnalysedType::Flags(tf) => D::parse_flags(lexer, tf, typ),
        AnalysedType::Handle(_) => {
            let pos = lexer.position();
            Err(perr(pos, "handle types not supported"))
        }
    }
}

// ── Shared numeric helpers ──────────────────────────────────────────────────

pub(super) fn parse_uint(lexer: &mut Lexer) -> Result<u64, ParseError> {
    let (tok, pos, _) = lexer.next_token()?;
    match tok {
        Token::UintLit(v) => Ok(v),
        _ => Err(perr(pos, "expected unsigned integer")),
    }
}

pub(super) fn parse_int(lexer: &mut Lexer) -> Result<i64, ParseError> {
    let (tok, pos, _) = lexer.next_token()?;
    match tok {
        Token::UintLit(v) => Ok(v as i64),
        Token::IntLit(v) => Ok(v),
        _ => Err(perr(pos, "expected integer")),
    }
}

pub(super) fn parse_float(lexer: &mut Lexer) -> Result<f64, ParseError> {
    let (tok, pos, _) = lexer.next_token()?;
    match tok {
        Token::FloatLit(v) => Ok(v),
        Token::UintLit(v) => Ok(v as f64),
        Token::IntLit(v) => Ok(v as f64),
        _ => Err(perr(pos, "expected number")),
    }
}

pub(super) fn perr(position: usize, message: &str) -> ParseError {
    ParseError {
        position,
        message: message.to_string(),
    }
}
