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
    BinaryReference, BinarySource, BinaryType, ComponentModelElementSchema,
    ComponentModelElementValue, DataSchema, DataValue, ElementSchema, ElementValue, ElementValues,
    NamedElementSchema, NamedElementValue, NamedElementValues, TextReference, TextSource, TextType,
    UnstructuredBinaryElementValue, UnstructuredTextElementValue, Url,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};
use heck::ToLowerCamelCase;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub position: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Parse error at position {}: {}", self.position, self.message)
    }
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError { position: e.position, message: e.message }
    }
}

pub fn parse_data_value_ts(input: &str, schema: &DataSchema) -> Result<DataValue, ParseError> {
    let mut lex = Lexer::new(input);
    let result = match schema {
        DataSchema::Tuple(schemas) => {
            let elements = parse_element_list(&mut lex, &schemas.elements)?;
            DataValue::Tuple(ElementValues { elements })
        }
        DataSchema::Multimodal(schemas) => {
            let elements = parse_named_elements(&mut lex, &schemas.elements)?;
            DataValue::Multimodal(NamedElementValues { elements })
        }
    };
    lex.expect(&Token::Eof)?;
    Ok(result)
}

fn parse_element_list(
    lex: &mut Lexer,
    schemas: &[NamedElementSchema],
) -> Result<Vec<ElementValue>, ParseError> {
    let mut elements = Vec::with_capacity(schemas.len());
    for (i, named) in schemas.iter().enumerate() {
        if i > 0 {
            lex.expect(&Token::Comma)?;
        }
        elements.push(parse_element(lex, &named.schema)?);
    }
    Ok(elements)
}

fn parse_named_elements(
    lex: &mut Lexer,
    schemas: &[NamedElementSchema],
) -> Result<Vec<NamedElementValue>, ParseError> {
    let name_map: Vec<(String, usize)> = schemas
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.to_lower_camel_case(), i))
        .collect();
    let mut elements = Vec::new();
    let mut first = true;
    while *lex.peek()? != Token::Eof {
        if !first {
            lex.expect(&Token::Comma)?;
            if *lex.peek()? == Token::Eof {
                break;
            }
        }
        first = false;
        let (key, pos, _) = lex.expect_ident()?;
        lex.expect(&Token::Colon)?;
        let (_, idx) = name_map
            .iter()
            .find(|(n, _)| *n == key)
            .ok_or_else(|| ParseError { position: pos, message: format!("unknown field '{key}'") })?;
        let schema = &schemas[*idx];
        let value = parse_element(lex, &schema.schema)?;
        elements.push(NamedElementValue {
            name: schema.name.clone(),
            value,
            schema_index: *idx as u32,
        });
    }
    Ok(elements)
}

fn parse_element(lex: &mut Lexer, schema: &ElementSchema) -> Result<ElementValue, ParseError> {
    match schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            let vt = parse_cm_value(lex, element_type)?;
            Ok(ElementValue::ComponentModel(ComponentModelElementValue { value: vt }))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let value = parse_unstructured_text(lex)?;
            Ok(ElementValue::UnstructuredText(UnstructuredTextElementValue {
                value,
                descriptor: descriptor.clone(),
            }))
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let value = parse_unstructured_binary(lex)?;
            Ok(ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                value,
                descriptor: descriptor.clone(),
            }))
        }
    }
}

fn parse_cm_value(lex: &mut Lexer, typ: &AnalysedType) -> Result<ValueAndType, ParseError> {
    match typ {
        AnalysedType::Bool(_) => {
            let (tok, pos, _) = lex.next_token()?;
            match tok {
                Token::BoolLit(b) => Ok(ValueAndType::new(Value::Bool(b), typ.clone())),
                _ => Err(ParseError { position: pos, message: "expected boolean".into() }),
            }
        }
        AnalysedType::U8(_) => parse_uint(lex, typ, |v| Value::U8(v as u8)),
        AnalysedType::U16(_) => parse_uint(lex, typ, |v| Value::U16(v as u16)),
        AnalysedType::U32(_) => parse_uint(lex, typ, |v| Value::U32(v as u32)),
        AnalysedType::U64(_) => parse_uint(lex, typ, |v| Value::U64(v)),
        AnalysedType::S8(_) => parse_sint(lex, typ, |v| Value::S8(v as i8)),
        AnalysedType::S16(_) => parse_sint(lex, typ, |v| Value::S16(v as i16)),
        AnalysedType::S32(_) => parse_sint(lex, typ, |v| Value::S32(v as i32)),
        AnalysedType::S64(_) => parse_sint(lex, typ, |v| Value::S64(v)),
        AnalysedType::F32(_) => {
            let v = parse_float(lex)?;
            Ok(ValueAndType::new(Value::F32(v as f32), typ.clone()))
        }
        AnalysedType::F64(_) => {
            let v = parse_float(lex)?;
            Ok(ValueAndType::new(Value::F64(v), typ.clone()))
        }
        AnalysedType::Chr(_) => {
            let (s, pos, _) = lex.expect_string()?;
            let mut chars = s.chars();
            let ch = chars.next().ok_or_else(|| ParseError { position: pos, message: "empty string for char".into() })?;
            if chars.next().is_some() {
                return Err(ParseError { position: pos, message: "expected single character".into() });
            }
            Ok(ValueAndType::new(Value::Char(ch), typ.clone()))
        }
        AnalysedType::Str(_) => {
            let (s, _, _) = lex.expect_string()?;
            Ok(ValueAndType::new(Value::String(s), typ.clone()))
        }
        AnalysedType::Enum(te) => {
            let (s, pos, _) = lex.expect_string()?;
            let idx = te.cases.iter().position(|c| *c == s)
                .ok_or_else(|| ParseError { position: pos, message: format!("unknown enum case '{s}'") })?;
            Ok(ValueAndType::new(Value::Enum(idx as u32), typ.clone()))
        }
        AnalysedType::Option(to) => {
            let is_nested = matches!(&*to.inner, AnalysedType::Option(_));
            match lex.peek()? {
                Token::Null | Token::Undefined => {
                    lex.next_token()?;
                    Ok(ValueAndType::new(Value::Option(None), typ.clone()))
                }
                Token::LBrace if is_nested => {
                    // Nested Option<Option<…>>: `{ some: <inner> }` means Some(inner)
                    lex.next_token()?;
                    let (key, pos, _) = lex.expect_ident()?;
                    if key != "some" {
                        return Err(ParseError { position: pos, message: format!("expected 'some', got '{key}'") });
                    }
                    lex.expect(&Token::Colon)?;
                    let inner = parse_cm_value(lex, &to.inner)?;
                    lex.expect(&Token::RBrace)?;
                    Ok(ValueAndType::new(Value::Option(Some(Box::new(inner.value))), typ.clone()))
                }
                _ => {
                    let inner = parse_cm_value(lex, &to.inner)?;
                    Ok(ValueAndType::new(Value::Option(Some(Box::new(inner.value))), typ.clone()))
                }
            }
        }
        AnalysedType::List(tl) => {
            lex.expect(&Token::LBrack)?;
            let mut items = Vec::new();
            while *lex.peek()? != Token::RBrack {
                if !items.is_empty() {
                    lex.expect(&Token::Comma)?;
                    if *lex.peek()? == Token::RBrack { break; }
                }
                items.push(parse_cm_value(lex, &tl.inner)?.value);
            }
            lex.expect(&Token::RBrack)?;
            Ok(ValueAndType::new(Value::List(items), typ.clone()))
        }
        AnalysedType::Tuple(tt) => {
            lex.expect(&Token::LBrack)?;
            let mut items = Vec::new();
            for (i, item_type) in tt.items.iter().enumerate() {
                if i > 0 {
                    lex.expect(&Token::Comma)?;
                }
                items.push(parse_cm_value(lex, item_type)?.value);
            }
            lex.expect(&Token::RBrack)?;
            Ok(ValueAndType::new(Value::Tuple(items), typ.clone()))
        }
        AnalysedType::Record(tr) => {
            lex.expect(&Token::LBrace)?;
            let name_map: Vec<(String, usize)> = tr.fields.iter().enumerate()
                .map(|(i, f)| (f.name.to_lower_camel_case(), i))
                .collect();
            let mut fields: Vec<Option<Value>> = vec![None; tr.fields.len()];
            while *lex.peek()? != Token::RBrace {
                if fields.iter().any(|f| f.is_some()) {
                    lex.expect(&Token::Comma)?;
                    if *lex.peek()? == Token::RBrace { break; }
                }
                let (key, pos, _) = lex.expect_ident()?;
                lex.expect(&Token::Colon)?;
                let (_, idx) = name_map.iter().find(|(n, _)| *n == key)
                    .ok_or_else(|| ParseError { position: pos, message: format!("unknown field '{key}'") })?;
                fields[*idx] = Some(parse_cm_value(lex, &tr.fields[*idx].typ)?.value);
            }
            lex.expect(&Token::RBrace)?;
            let values = fields.into_iter().enumerate()
                .map(|(i, v)| v.ok_or_else(|| ParseError { position: 0, message: format!("missing field '{}'", tr.fields[i].name) }))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ValueAndType::new(Value::Record(values), typ.clone()))
        }
        AnalysedType::Variant(tv) => {
            lex.expect(&Token::LBrace)?;
            expect_ident_key(lex, "tag")?;
            let (case_name, pos, _) = lex.expect_string()?;
            let case_idx = tv.cases.iter().position(|c| c.name == case_name)
                .ok_or_else(|| ParseError { position: pos, message: format!("unknown variant case '{case_name}'") })?;
            let case_value = if *lex.peek()? == Token::Comma {
                lex.next_token()?;
                if *lex.peek()? == Token::RBrace {
                    None
                } else {
                    expect_ident_key(lex, "value")?;
                    tv.cases[case_idx].typ.as_ref()
                        .map(|t| parse_cm_value(lex, t).map(|vt| vt.value))
                        .transpose()?
                }
            } else {
                None
            };
            if *lex.peek()? == Token::Comma { lex.next_token()?; }
            lex.expect(&Token::RBrace)?;
            Ok(ValueAndType::new(Value::Variant { case_idx: case_idx as u32, case_value: case_value.map(Box::new) }, typ.clone()))
        }
        AnalysedType::Result(tr) => {
            lex.expect(&Token::LBrace)?;
            let (key, pos, _) = lex.expect_ident()?;
            lex.expect(&Token::Colon)?;
            let result = match key.as_str() {
                "ok" => {
                    let val = match &tr.ok {
                        Some(ok_type) => Some(Box::new(parse_cm_value(lex, ok_type)?.value)),
                        None => {
                            // Unit ok: accept null or undefined
                            if matches!(lex.peek()?, Token::Null | Token::Undefined) { lex.next_token()?; }
                            None
                        }
                    };
                    Value::Result(Ok(val))
                }
                "error" => {
                    let val = match &tr.err {
                        Some(err_type) => Some(Box::new(parse_cm_value(lex, err_type)?.value)),
                        None => {
                            // Unit err: accept null or undefined
                            if matches!(lex.peek()?, Token::Null | Token::Undefined) { lex.next_token()?; }
                            None
                        }
                    };
                    Value::Result(Err(val))
                }
                _ => return Err(ParseError { position: pos, message: format!("expected 'ok' or 'error', got '{key}'") }),
            };
            if *lex.peek()? == Token::Comma { lex.next_token()?; }
            lex.expect(&Token::RBrace)?;
            Ok(ValueAndType::new(result, typ.clone()))
        }
        AnalysedType::Flags(tf) => {
            lex.expect(&Token::LBrace)?;
            let name_map: Vec<(String, usize)> = tf.names.iter().enumerate()
                .map(|(i, n)| (n.to_lower_camel_case(), i))
                .collect();
            let mut flags = vec![false; tf.names.len()];
            while *lex.peek()? != Token::RBrace {
                if flags.iter().any(|f| *f) {
                    lex.expect(&Token::Comma)?;
                    if *lex.peek()? == Token::RBrace { break; }
                }
                let (key, pos, _) = lex.expect_ident()?;
                lex.expect(&Token::Colon)?;
                let (tok, vpos, _) = lex.next_token()?;
                let Token::BoolLit(val) = tok else {
                    return Err(ParseError { position: vpos, message: "expected boolean".into() });
                };
                let (_, idx) = name_map.iter().find(|(n, _)| *n == key)
                    .ok_or_else(|| ParseError { position: pos, message: format!("unknown flag '{key}'") })?;
                if val { flags[*idx] = true; }
            }
            lex.expect(&Token::RBrace)?;
            Ok(ValueAndType::new(Value::Flags(flags), typ.clone()))
        }
        _ => {
            let pos = lex.position();
            Err(ParseError { position: pos, message: format!("unsupported type: {typ:?}") })
        }
    }
}

fn parse_uint(lex: &mut Lexer, typ: &AnalysedType, wrap: fn(u64) -> Value) -> Result<ValueAndType, ParseError> {
    let (tok, pos, _) = lex.next_token()?;
    match tok {
        Token::UintLit(v) => Ok(ValueAndType::new(wrap(v), typ.clone())),
        _ => Err(ParseError { position: pos, message: "expected unsigned integer".into() }),
    }
}

fn parse_sint(lex: &mut Lexer, typ: &AnalysedType, wrap: fn(i64) -> Value) -> Result<ValueAndType, ParseError> {
    let (tok, pos, _) = lex.next_token()?;
    match tok {
        Token::IntLit(v) => Ok(ValueAndType::new(wrap(v), typ.clone())),
        Token::UintLit(v) => Ok(ValueAndType::new(wrap(v as i64), typ.clone())),
        _ => Err(ParseError { position: pos, message: "expected integer".into() }),
    }
}

fn parse_float(lex: &mut Lexer) -> Result<f64, ParseError> {
    let (tok, pos, _) = lex.next_token()?;
    match tok {
        Token::FloatLit(v) => Ok(v),
        Token::UintLit(v) => Ok(v as f64),
        Token::IntLit(v) => Ok(v as f64),
        _ => Err(ParseError { position: pos, message: "expected number".into() }),
    }
}

fn expect_ident_key(lex: &mut Lexer, expected: &str) -> Result<(), ParseError> {
    let (name, pos, _) = lex.expect_ident()?;
    if name != expected {
        return Err(ParseError { position: pos, message: format!("expected '{expected}', got '{name}'") });
    }
    lex.expect(&Token::Colon)?;
    Ok(())
}

fn parse_unstructured_text(lex: &mut Lexer) -> Result<TextReference, ParseError> {
    lex.expect(&Token::LBrace)?;
    expect_ident_key(lex, "tag")?;
    let (tag, pos, _) = lex.expect_string()?;
    lex.expect(&Token::Comma)?;
    expect_ident_key(lex, "val")?;
    let (val, _, _) = lex.expect_string()?;
    let result = match tag.as_str() {
        "url" => TextReference::Url(Url { value: val }),
        "inline" => {
            let text_type = if *lex.peek()? == Token::Comma {
                lex.next_token()?;
                if *lex.peek()? != Token::RBrace {
                    expect_ident_key(lex, "lang")?;
                    let (lang, _, _) = lex.expect_string()?;
                    Some(TextType { language_code: lang })
                } else {
                    None
                }
            } else {
                None
            };
            TextReference::Inline(TextSource { data: val, text_type })
        }
        _ => return Err(ParseError { position: pos, message: format!("expected 'url' or 'inline', got '{tag}'") }),
    };
    if *lex.peek()? == Token::Comma { lex.next_token()?; }
    lex.expect(&Token::RBrace)?;
    Ok(result)
}

fn parse_unstructured_binary(lex: &mut Lexer) -> Result<BinaryReference, ParseError> {
    lex.expect(&Token::LBrace)?;
    expect_ident_key(lex, "tag")?;
    let (tag, pos, _) = lex.expect_string()?;
    lex.expect(&Token::Comma)?;
    match tag.as_str() {
        "url" => {
            expect_ident_key(lex, "val")?;
            let (val, _, _) = lex.expect_string()?;
            if *lex.peek()? == Token::Comma { lex.next_token()?; }
            lex.expect(&Token::RBrace)?;
            Ok(BinaryReference::Url(Url { value: val }))
        }
        "inline" => {
            expect_ident_key(lex, "val")?;
            let (ident, ipos, _) = lex.expect_ident()?;
            if ident != "Uint8Array" {
                return Err(ParseError { position: ipos, message: format!("expected 'Uint8Array', got '{ident}'") });
            }
            lex.expect(&Token::LParen)?;
            lex.expect(&Token::LBrack)?;
            let mut bytes = Vec::new();
            while *lex.peek()? != Token::RBrack {
                if !bytes.is_empty() {
                    lex.expect(&Token::Comma)?;
                    if *lex.peek()? == Token::RBrack { break; }
                }
                let (tok, bpos, _) = lex.next_token()?;
                match tok {
                    Token::UintLit(v) => bytes.push(v as u8),
                    _ => return Err(ParseError { position: bpos, message: "expected byte value".into() }),
                }
            }
            lex.expect(&Token::RBrack)?;
            lex.expect(&Token::RParen)?;
            lex.expect(&Token::Comma)?;
            expect_ident_key(lex, "mime")?;
            let (mime, _, _) = lex.expect_string()?;
            if *lex.peek()? == Token::Comma { lex.next_token()?; }
            lex.expect(&Token::RBrace)?;
            Ok(BinaryReference::Inline(BinarySource {
                data: bytes,
                binary_type: BinaryType { mime_type: mime },
            }))
        }
        _ => Err(ParseError { position: pos, message: format!("expected 'url' or 'inline', got '{tag}'") }),
    }
}
