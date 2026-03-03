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
    BinaryReference, BinarySource, BinaryType, ComponentModelElementValue, DataSchema, DataValue,
    ElementSchema, ElementValue, ElementValues, NamedElementValue, NamedElementValues,
    TextReference, TextSource, TextType, UnstructuredBinaryElementValue,
    UnstructuredTextElementValue, Url,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};
use heck::{ToSnakeCase, ToUpperCamelCase};
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

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError {
            position: e.position,
            message: e.message,
        }
    }
}

pub fn parse_data_value_rust(input: &str, schema: &DataSchema) -> Result<DataValue, ParseError> {
    let mut lexer = Lexer::new(input);
    let result = match schema {
        DataSchema::Tuple(schemas) => {
            let mut elements = Vec::new();
            for (i, s) in schemas.elements.iter().enumerate() {
                if i > 0 {
                    lexer.expect(&Token::Comma)?;
                }
                elements.push(parse_element(&mut lexer, &s.schema)?);
            }
            DataValue::Tuple(ElementValues { elements })
        }
        DataSchema::Multimodal(schemas) => {
            let mut elements = Vec::new();
            let mut first = true;
            while *lexer.peek()? != Token::Eof {
                if !first {
                    lexer.expect(&Token::Comma)?;
                }
                first = false;
                let pos = lexer.position();
                let (name, _, _) = lexer.expect_ident()?;
                lexer.expect(&Token::Colon)?;
                let snake = name.to_snake_case();
                let (schema_index, schema_elem) = schemas
                    .elements
                    .iter()
                    .enumerate()
                    .find(|(_, s)| s.name.to_snake_case() == snake)
                    .ok_or_else(|| ParseError {
                        position: pos,
                        message: format!("unknown field '{name}'"),
                    })?;
                let value = parse_element(&mut lexer, &schema_elem.schema)?;
                elements.push(NamedElementValue {
                    name: schema_elem.name.clone(),
                    value,
                    schema_index: schema_index as u32,
                });
            }
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

fn parse_element(lexer: &mut Lexer, schema: &ElementSchema) -> Result<ElementValue, ParseError> {
    match schema {
        ElementSchema::ComponentModel(cm) => {
            let value = parse_cm_value(lexer, &cm.element_type)?;
            let typ = cm.element_type.clone();
            Ok(ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType { value, typ },
            }))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let value = parse_unstructured_text(lexer)?;
            Ok(ElementValue::UnstructuredText(UnstructuredTextElementValue {
                value,
                descriptor: descriptor.clone(),
            }))
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let value = parse_unstructured_binary(lexer)?;
            Ok(ElementValue::UnstructuredBinary(
                UnstructuredBinaryElementValue {
                    value,
                    descriptor: descriptor.clone(),
                },
            ))
        }
    }
}

fn parse_cm_value(lexer: &mut Lexer, typ: &AnalysedType) -> Result<Value, ParseError> {
    let pos = lexer.position();
    match typ {
        AnalysedType::Bool(_) => match lexer.next_token()? {
            (Token::BoolLit(b), _, _) => Ok(Value::Bool(b)),
            (tok, p, _) => Err(perr(p, format!("expected bool, got {tok:?}"))),
        },
        AnalysedType::U8(_) => parse_uint(lexer).map(|v| Value::U8(v as u8)),
        AnalysedType::U16(_) => parse_uint(lexer).map(|v| Value::U16(v as u16)),
        AnalysedType::U32(_) => parse_uint(lexer).map(|v| Value::U32(v as u32)),
        AnalysedType::U64(_) => parse_uint(lexer).map(|v| Value::U64(v)),
        AnalysedType::S8(_) => parse_int(lexer).map(|v| Value::S8(v as i8)),
        AnalysedType::S16(_) => parse_int(lexer).map(|v| Value::S16(v as i16)),
        AnalysedType::S32(_) => parse_int(lexer).map(|v| Value::S32(v as i32)),
        AnalysedType::S64(_) => parse_int(lexer).map(|v| Value::S64(v)),
        AnalysedType::F32(_) => parse_float(lexer).map(|v| Value::F32(v as f32)),
        AnalysedType::F64(_) => parse_float(lexer).map(|v| Value::F64(v)),
        AnalysedType::Chr(_) => match lexer.next_token()? {
            (Token::CharLit(c), _, _) => Ok(Value::Char(c)),
            (tok, p, _) => Err(perr(p, format!("expected char, got {tok:?}"))),
        },
        AnalysedType::Str(_) => match lexer.next_token()? {
            (Token::StringLit(s), _, _) => Ok(Value::String(s)),
            (tok, p, _) => Err(perr(p, format!("expected string, got {tok:?}"))),
        },
        AnalysedType::List(tl) => {
            lexer.expect(&Token::LBrack)?;
            let mut items = Vec::new();
            if *lexer.peek()? != Token::RBrack {
                items.push(parse_cm_value(lexer, &tl.inner)?);
                while *lexer.peek()? == Token::Comma {
                    lexer.next_token()?;
                    if *lexer.peek()? == Token::RBrack {
                        break;
                    }
                    items.push(parse_cm_value(lexer, &tl.inner)?);
                }
            }
            lexer.expect(&Token::RBrack)?;
            Ok(Value::List(items))
        }
        AnalysedType::Tuple(tt) => {
            lexer.expect(&Token::LParen)?;
            let mut items = Vec::new();
            for (i, item_typ) in tt.items.iter().enumerate() {
                if i > 0 {
                    lexer.expect(&Token::Comma)?;
                }
                items.push(parse_cm_value(lexer, item_typ)?);
            }
            lexer.expect(&Token::RParen)?;
            Ok(Value::Tuple(items))
        }
        AnalysedType::Record(tr) => {
            if let Some(name) = &tr.name {
                let camel = name.to_upper_camel_case();
                if let Token::Ident(id) = lexer.peek()? {
                    if id == &camel {
                        lexer.next_token()?;
                    }
                }
            }
            lexer.expect(&Token::LBrace)?;
            let field_map: Vec<(String, usize)> = tr
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| (f.name.to_snake_case(), i))
                .collect();
            let mut values: Vec<Option<Value>> = vec![None; tr.fields.len()];
            if *lexer.peek()? != Token::RBrace {
                loop {
                    let (fname, fp, _) = lexer.expect_ident()?;
                    lexer.expect(&Token::Colon)?;
                    let idx = field_map
                        .iter()
                        .find(|(n, _)| *n == fname)
                        .map(|(_, i)| *i)
                        .ok_or_else(|| perr(fp, format!("unknown field '{fname}'")))?;
                    values[idx] = Some(parse_cm_value(lexer, &tr.fields[idx].typ)?);
                    if *lexer.peek()? == Token::Comma {
                        lexer.next_token()?;
                        if *lexer.peek()? == Token::RBrace {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            lexer.expect(&Token::RBrace)?;
            let fields: Result<Vec<Value>, ParseError> = values
                .into_iter()
                .enumerate()
                .map(|(i, v)| {
                    v.ok_or_else(|| {
                        perr(pos, format!("missing field '{}'", tr.fields[i].name))
                    })
                })
                .collect();
            Ok(Value::Record(fields?))
        }
        AnalysedType::Variant(tv) => {
            if let Some(name) = &tv.name {
                let camel = name.to_upper_camel_case();
                if let Token::Ident(id) = lexer.peek()? {
                    if id == &camel {
                        lexer.next_token()?;
                        lexer.expect(&Token::DoubleColon)?;
                    }
                }
            }
            let (case_name, cp, _) = lexer.expect_ident()?;
            let (case_idx, case_def) = tv
                .cases
                .iter()
                .enumerate()
                .find(|(_, c)| c.name.to_upper_camel_case() == case_name)
                .ok_or_else(|| perr(cp, format!("unknown variant case '{case_name}'")))?;
            let case_value = if let Some(case_typ) = &case_def.typ {
                lexer.expect(&Token::LParen)?;
                let v = parse_cm_value(lexer, case_typ)?;
                lexer.expect(&Token::RParen)?;
                Some(Box::new(v))
            } else {
                None
            };
            Ok(Value::Variant {
                case_idx: case_idx as u32,
                case_value,
            })
        }
        AnalysedType::Enum(te) => {
            if let Some(name) = &te.name {
                let camel = name.to_upper_camel_case();
                if let Token::Ident(id) = lexer.peek()? {
                    if id == &camel {
                        lexer.next_token()?;
                        lexer.expect(&Token::DoubleColon)?;
                    }
                }
            }
            let (case_name, cp, _) = lexer.expect_ident()?;
            let case_idx = te
                .cases
                .iter()
                .position(|c| c.to_upper_camel_case() == case_name)
                .ok_or_else(|| perr(cp, format!("unknown enum case '{case_name}'")))?;
            Ok(Value::Enum(case_idx as u32))
        }
        AnalysedType::Option(to) => {
            let (ident, p, _) = lexer.expect_ident()?;
            match ident.as_str() {
                "None" => Ok(Value::Option(None)),
                "Some" => {
                    lexer.expect(&Token::LParen)?;
                    let v = parse_cm_value(lexer, &to.inner)?;
                    lexer.expect(&Token::RParen)?;
                    Ok(Value::Option(Some(Box::new(v))))
                }
                _ => Err(perr(p, format!("expected Some or None, got '{ident}'"))),
            }
        }
        AnalysedType::Result(tr) => {
            let (ident, p, _) = lexer.expect_ident()?;
            match ident.as_str() {
                "Ok" => {
                    lexer.expect(&Token::LParen)?;
                    let v = if let Some(ok_typ) = &tr.ok {
                        Some(Box::new(parse_cm_value(lexer, ok_typ)?))
                    } else {
                        // Unit ok: accept Ok(()) or Ok()
                        if *lexer.peek()? == Token::LParen {
                            lexer.next_token()?;
                            lexer.expect(&Token::RParen)?;
                        }
                        None
                    };
                    lexer.expect(&Token::RParen)?;
                    Ok(Value::Result(Ok(v)))
                }
                "Err" => {
                    lexer.expect(&Token::LParen)?;
                    let v = if let Some(err_typ) = &tr.err {
                        Some(Box::new(parse_cm_value(lexer, err_typ)?))
                    } else {
                        // Unit err: accept Err(()) or Err()
                        if *lexer.peek()? == Token::LParen {
                            lexer.next_token()?;
                            lexer.expect(&Token::RParen)?;
                        }
                        None
                    };
                    lexer.expect(&Token::RParen)?;
                    Ok(Value::Result(Err(v)))
                }
                _ => Err(perr(p, format!("expected Ok or Err, got '{ident}'"))),
            }
        }
        AnalysedType::Flags(tf) => {
            if let Some(name) = &tf.name {
                let camel = name.to_upper_camel_case();
                if let Token::Ident(id) = lexer.peek()? {
                    if id == &camel {
                        lexer.next_token()?;
                    }
                }
            }
            lexer.expect(&Token::LBrace)?;
            let mut flags = vec![false; tf.names.len()];
            if *lexer.peek()? != Token::RBrace {
                loop {
                    let (fname, fp, _) = lexer.expect_ident()?;
                    let idx = tf
                        .names
                        .iter()
                        .position(|n| n.to_snake_case() == fname)
                        .ok_or_else(|| perr(fp, format!("unknown flag '{fname}'")))?;
                    flags[idx] = true;
                    if *lexer.peek()? == Token::Comma {
                        lexer.next_token()?;
                        if *lexer.peek()? == Token::RBrace {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            lexer.expect(&Token::RBrace)?;
            Ok(Value::Flags(flags))
        }
        AnalysedType::Handle(_) => Err(perr(pos, "handle types not supported".into())),
    }
}

fn parse_uint(lexer: &mut Lexer) -> Result<u64, ParseError> {
    match lexer.next_token()? {
        (Token::UintLit(v), _, _) => Ok(v),
        (tok, p, _) => Err(perr(p, format!("expected unsigned integer, got {tok:?}"))),
    }
}

fn parse_int(lexer: &mut Lexer) -> Result<i64, ParseError> {
    match lexer.next_token()? {
        (Token::UintLit(v), _, _) => Ok(v as i64),
        (Token::IntLit(v), _, _) => Ok(v),
        (tok, p, _) => Err(perr(p, format!("expected integer, got {tok:?}"))),
    }
}

fn parse_float(lexer: &mut Lexer) -> Result<f64, ParseError> {
    match lexer.next_token()? {
        (Token::FloatLit(v), _, _) => Ok(v),
        (Token::UintLit(v), _, _) => Ok(v as f64),
        (Token::IntLit(v), _, _) => Ok(v as f64),
        (tok, p, _) => Err(perr(p, format!("expected float, got {tok:?}"))),
    }
}

fn parse_unstructured_text(lexer: &mut Lexer) -> Result<TextReference, ParseError> {
    let (ident, p, _) = lexer.expect_ident()?;
    if ident != "UnstructuredText" {
        return Err(perr(p, format!("expected 'UnstructuredText', got '{ident}'")));
    }
    lexer.expect(&Token::DoubleColon)?;
    let (method, mp, _) = lexer.expect_ident()?;
    match method.as_str() {
        "Url" => {
            lexer.expect(&Token::LParen)?;
            let (url, _, _) = lexer.expect_string()?;
            lexer.expect(&Token::RParen)?;
            Ok(TextReference::Url(Url { value: url }))
        }
        "from_inline_any" => {
            lexer.expect(&Token::LParen)?;
            let (data, _, _) = lexer.expect_string()?;
            lexer.expect(&Token::RParen)?;
            Ok(TextReference::Inline(TextSource {
                data,
                text_type: None,
            }))
        }
        "from_inline" => {
            lexer.expect(&Token::LParen)?;
            let (data, _, _) = lexer.expect_string()?;
            lexer.expect(&Token::Comma)?;
            let (lang_ns, lp, _) = lexer.expect_ident()?;
            if lang_ns != "Languages" {
                return Err(perr(lp, format!("expected 'Languages', got '{lang_ns}'")));
            }
            lexer.expect(&Token::DoubleColon)?;
            let (lang, _, _) = lexer.expect_ident()?;
            lexer.expect(&Token::RParen)?;
            Ok(TextReference::Inline(TextSource {
                data,
                text_type: Some(TextType {
                    language_code: lang,
                }),
            }))
        }
        _ => Err(perr(mp, format!("unknown UnstructuredText method '{method}'"))),
    }
}

fn parse_unstructured_binary(lexer: &mut Lexer) -> Result<BinaryReference, ParseError> {
    let (ident, p, _) = lexer.expect_ident()?;
    if ident != "UnstructuredBinary" {
        return Err(perr(
            p,
            format!("expected 'UnstructuredBinary', got '{ident}'"),
        ));
    }
    lexer.expect(&Token::DoubleColon)?;
    let (method, mp, _) = lexer.expect_ident()?;
    match method.as_str() {
        "from_url" => {
            lexer.expect(&Token::LParen)?;
            let (url, _, _) = lexer.expect_string()?;
            lexer.expect(&Token::RParen)?;
            Ok(BinaryReference::Url(Url { value: url }))
        }
        "from_inline" => {
            lexer.expect(&Token::LParen)?;
            let (vec_ident, vp, _) = lexer.expect_ident()?;
            if vec_ident != "vec" {
                return Err(perr(vp, format!("expected 'vec', got '{vec_ident}'")));
            }
            lexer.skip_raw_char(b'!');
            lexer.expect(&Token::LBrack)?;
            let mut data = Vec::new();
            if *lexer.peek()? != Token::RBrack {
                loop {
                    let b = parse_uint(lexer)? as u8;
                    data.push(b);
                    if *lexer.peek()? == Token::Comma {
                        lexer.next_token()?;
                        if *lexer.peek()? == Token::RBrack {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            lexer.expect(&Token::RBrack)?;
            lexer.expect(&Token::Comma)?;
            let (mime_ns, mnp, _) = lexer.expect_ident()?;
            if mime_ns != "MimeTypes" {
                return Err(perr(mnp, format!("expected 'MimeTypes', got '{mime_ns}'")));
            }
            lexer.expect(&Token::DoubleColon)?;
            let (mime, _, _) = lexer.expect_ident()?;
            lexer.expect(&Token::RParen)?;
            Ok(BinaryReference::Inline(BinarySource {
                data,
                binary_type: BinaryType { mime_type: mime },
            }))
        }
        _ => Err(perr(
            mp,
            format!("unknown UnstructuredBinary method '{method}'"),
        )),
    }
}

fn perr(position: usize, message: String) -> ParseError {
    ParseError { position, message }
}
