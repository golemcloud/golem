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

use super::lexer::{Lexer, Token};
use super::parse_common::{self, Dialect, ParseError, parse_cm_value, parse_uint, perr};
use golem_common::model::agent::{
    BinaryReference, BinarySource, BinaryType, DataSchema, DataValue, TextReference, TextSource,
    TextType, Url,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};
use heck::{ToLowerCamelCase, ToUpperCamelCase};

pub fn parse_data_value_scala(input: &str, schema: &DataSchema) -> Result<DataValue, ParseError> {
    parse_common::parse_data_value::<ScalaDialect>(input, schema)
}

pub(super) struct ScalaDialect;

impl Dialect for ScalaDialect {
    fn normalize_field_name(name: &str) -> String {
        name.to_lower_camel_case()
    }

    fn named_element_separator() -> Token {
        Token::Eq
    }

    fn parse_char(lexer: &mut Lexer) -> Result<char, ParseError> {
        let (tok, pos, _) = lexer.next_token()?;
        match tok {
            Token::CharLit(c) => Ok(c),
            _ => Err(perr(pos, "expected char literal")),
        }
    }

    fn parse_tuple(
        lexer: &mut Lexer,
        tt: &golem_wasm::analysis::TypeTuple,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        // For arity == 1: accept both Tuple1(a) and (a)
        if tt.items.len() == 1 {
            if let Token::Ident(id) = lexer.peek()? {
                if id == "Tuple1" {
                    lexer.next_token()?;
                    lexer.expect(&Token::LParen)?;
                    let v = parse_cm_value::<Self>(lexer, &tt.items[0])?.value;
                    lexer.expect(&Token::RParen)?;
                    return Ok(ValueAndType::new(Value::Tuple(vec![v]), typ.clone()));
                }
            }
        }
        // For arity > 1 or fallback for arity == 1: (a, b, ...)
        lexer.expect(&Token::LParen)?;
        let mut items = Vec::new();
        for (i, item_typ) in tt.items.iter().enumerate() {
            if i > 0 {
                lexer.expect(&Token::Comma)?;
            }
            items.push(parse_cm_value::<Self>(lexer, item_typ)?.value);
        }
        lexer.expect(&Token::RParen)?;
        Ok(ValueAndType::new(Value::Tuple(items), typ.clone()))
    }

    fn parse_record(
        lexer: &mut Lexer,
        tr: &golem_wasm::analysis::TypeRecord,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        if let Some(name) = &tr.name {
            // Named record: optionally accept TypeName prefix, then (...)
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()? {
                if *id == camel {
                    lexer.next_token()?;
                }
            }
            lexer.expect(&Token::LParen)?;
            let values = parse_record_fields(lexer, tr, &Token::RParen)?;
            lexer.expect(&Token::RParen)?;
            Ok(ValueAndType::new(Value::Record(values), typ.clone()))
        } else {
            // Anonymous record: { fieldOne = 42, fieldTwo = "hi" }
            lexer.expect(&Token::LBrace)?;
            let values = parse_record_fields(lexer, tr, &Token::RBrace)?;
            lexer.expect(&Token::RBrace)?;
            Ok(ValueAndType::new(Value::Record(values), typ.clone()))
        }
    }

    fn parse_variant(
        lexer: &mut Lexer,
        tv: &golem_wasm::analysis::TypeVariant,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        // Optionally accept TypeName.CaseA or just CaseA
        if let Some(name) = &tv.name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()? {
                if *id == camel {
                    lexer.next_token()?;
                    lexer.expect(&Token::Dot)?;
                }
            }
        }
        let (case_name, cp, _) = lexer.expect_ident()?;
        let (case_idx, case_def) = tv
            .cases
            .iter()
            .enumerate()
            .find(|(_, c)| c.name.to_upper_camel_case() == case_name)
            .ok_or_else(|| perr(cp, &format!("unknown variant case '{case_name}'")))?;
        let case_value = if let Some(case_typ) = &case_def.typ {
            lexer.expect(&Token::LParen)?;
            let v = parse_cm_value::<Self>(lexer, case_typ)?.value;
            lexer.expect(&Token::RParen)?;
            Some(Box::new(v))
        } else {
            None
        };
        Ok(ValueAndType::new(
            Value::Variant {
                case_idx: case_idx as u32,
                case_value,
            },
            typ.clone(),
        ))
    }

    fn parse_enum(
        lexer: &mut Lexer,
        te: &golem_wasm::analysis::TypeEnum,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        // MyEnum.CaseA or just CaseA
        if let Some(name) = &te.name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()? {
                if *id == camel {
                    lexer.next_token()?;
                    lexer.expect(&Token::Dot)?;
                }
            }
        }
        let (case_name, cp, _) = lexer.expect_ident()?;
        let case_idx = te
            .cases
            .iter()
            .position(|c| c.to_upper_camel_case() == case_name)
            .ok_or_else(|| perr(cp, &format!("unknown enum case '{case_name}'")))?;
        Ok(ValueAndType::new(Value::Enum(case_idx as u32), typ.clone()))
    }

    fn parse_option(
        lexer: &mut Lexer,
        to: &golem_wasm::analysis::TypeOption,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        let (ident, p, _) = lexer.expect_ident()?;
        match ident.as_str() {
            "None" => Ok(ValueAndType::new(Value::Option(None), typ.clone())),
            "Some" => {
                lexer.expect(&Token::LParen)?;
                let v = parse_cm_value::<Self>(lexer, &to.inner)?.value;
                lexer.expect(&Token::RParen)?;
                Ok(ValueAndType::new(
                    Value::Option(Some(Box::new(v))),
                    typ.clone(),
                ))
            }
            _ => Err(perr(p, &format!("expected Some or None, got '{ident}'"))),
        }
    }

    fn parse_result(
        lexer: &mut Lexer,
        tr: &golem_wasm::analysis::TypeResult,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        // Accept WitResult.Ok(value) or just Ok(value)
        let (ident, p, _) = lexer.expect_ident()?;
        let ok_or_err = if ident == "WitResult" {
            lexer.expect(&Token::Dot)?;
            let (inner, _, _) = lexer.expect_ident()?;
            inner
        } else {
            ident
        };
        match ok_or_err.as_str() {
            "Ok" => {
                lexer.expect(&Token::LParen)?;
                let v = if let Some(ok_typ) = &tr.ok {
                    Some(Box::new(parse_cm_value::<Self>(lexer, ok_typ)?.value))
                } else {
                    if *lexer.peek()? == Token::LParen {
                        lexer.next_token()?;
                        lexer.expect(&Token::RParen)?;
                    }
                    None
                };
                lexer.expect(&Token::RParen)?;
                Ok(ValueAndType::new(Value::Result(Ok(v)), typ.clone()))
            }
            "Err" => {
                lexer.expect(&Token::LParen)?;
                let v = if let Some(err_typ) = &tr.err {
                    Some(Box::new(parse_cm_value::<Self>(lexer, err_typ)?.value))
                } else {
                    if *lexer.peek()? == Token::LParen {
                        lexer.next_token()?;
                        lexer.expect(&Token::RParen)?;
                    }
                    None
                };
                lexer.expect(&Token::RParen)?;
                Ok(ValueAndType::new(Value::Result(Err(v)), typ.clone()))
            }
            _ => Err(perr(p, &format!("expected Ok or Err, got '{ok_or_err}'"))),
        }
    }

    fn parse_flags(
        lexer: &mut Lexer,
        tf: &golem_wasm::analysis::TypeFlags,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        let (open, close) = if let Some(name) = &tf.name {
            // Named: optionally TypeName prefix, then (...)
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()? {
                if *id == camel {
                    lexer.next_token()?;
                }
            }
            lexer.expect(&Token::LParen)?;
            (Token::LParen, Token::RParen)
        } else {
            // Anonymous: { ... }
            lexer.expect(&Token::LBrace)?;
            (Token::LBrace, Token::RBrace)
        };
        let name_map: Vec<(String, usize)> = tf
            .names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lower_camel_case(), i))
            .collect();
        let mut flags = vec![false; tf.names.len()];
        let _ = open; // already consumed
        let mut parsed_count = 0;
        while *lexer.peek()? != close {
            if parsed_count > 0 {
                lexer.expect(&Token::Comma)?;
                if *lexer.peek()? == close {
                    break;
                }
            }
            let (key, pos, _) = lexer.expect_ident()?;
            lexer.expect(&Token::Eq)?;
            let (tok, vpos, _) = lexer.next_token()?;
            let Token::BoolLit(val) = tok else {
                return Err(perr(vpos, "expected boolean"));
            };
            let (_, idx) = name_map
                .iter()
                .find(|(n, _)| *n == key)
                .ok_or_else(|| perr(pos, &format!("unknown flag '{key}'")))?;
            if val {
                flags[*idx] = true;
            }
            parsed_count += 1;
        }
        lexer.expect(&close)?;
        Ok(ValueAndType::new(Value::Flags(flags), typ.clone()))
    }

    fn parse_list(
        lexer: &mut Lexer,
        tl: &golem_wasm::analysis::TypeList,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        // Accept List(1, 2, 3) or [1, 2, 3]
        if let Token::Ident(id) = lexer.peek()? {
            if id == "List" {
                lexer.next_token()?;
                lexer.expect(&Token::LParen)?;
                let mut items = Vec::new();
                while *lexer.peek()? != Token::RParen {
                    if !items.is_empty() {
                        lexer.expect(&Token::Comma)?;
                        if *lexer.peek()? == Token::RParen {
                            break;
                        }
                    }
                    items.push(parse_cm_value::<Self>(lexer, &tl.inner)?.value);
                }
                lexer.expect(&Token::RParen)?;
                return Ok(ValueAndType::new(Value::List(items), typ.clone()));
            }
        }
        // Fall back to default bracket syntax [1, 2, 3]
        lexer.expect(&Token::LBrack)?;
        let mut items = Vec::new();
        while *lexer.peek()? != Token::RBrack {
            if !items.is_empty() {
                lexer.expect(&Token::Comma)?;
                if *lexer.peek()? == Token::RBrack {
                    break;
                }
            }
            items.push(parse_cm_value::<Self>(lexer, &tl.inner)?.value);
        }
        lexer.expect(&Token::RBrack)?;
        Ok(ValueAndType::new(Value::List(items), typ.clone()))
    }

    fn parse_unstructured_text(lexer: &mut Lexer) -> Result<TextReference, ParseError> {
        let (ident, p, _) = lexer.expect_ident()?;
        if ident != "UnstructuredTextValue" {
            return Err(perr(
                p,
                &format!("expected 'UnstructuredTextValue', got '{ident}'"),
            ));
        }
        lexer.expect(&Token::Dot)?;
        let (method, mp, _) = lexer.expect_ident()?;
        match method.as_str() {
            "Url" => {
                lexer.expect(&Token::LParen)?;
                let (url, _, _) = lexer.expect_string()?;
                lexer.expect(&Token::RParen)?;
                Ok(TextReference::Url(Url { value: url }))
            }
            "Inline" => {
                lexer.expect(&Token::LParen)?;
                let (data, _, _) = lexer.expect_string()?;
                let text_type = if *lexer.peek()? == Token::Comma {
                    lexer.next_token()?;
                    // Some("en") or None
                    let (opt_ident, op, _) = lexer.expect_ident()?;
                    match opt_ident.as_str() {
                        "None" => None,
                        "Some" => {
                            lexer.expect(&Token::LParen)?;
                            let (lang, _, _) = lexer.expect_string()?;
                            lexer.expect(&Token::RParen)?;
                            Some(TextType {
                                language_code: lang,
                            })
                        }
                        _ => {
                            return Err(perr(
                                op,
                                &format!("expected Some or None, got '{opt_ident}'"),
                            ));
                        }
                    }
                } else {
                    None
                };
                lexer.expect(&Token::RParen)?;
                Ok(TextReference::Inline(TextSource { data, text_type }))
            }
            _ => Err(perr(
                mp,
                &format!("unknown UnstructuredTextValue method '{method}'"),
            )),
        }
    }

    fn parse_unstructured_binary(lexer: &mut Lexer) -> Result<BinaryReference, ParseError> {
        let (ident, p, _) = lexer.expect_ident()?;
        if ident != "UnstructuredBinaryValue" {
            return Err(perr(
                p,
                &format!("expected 'UnstructuredBinaryValue', got '{ident}'"),
            ));
        }
        lexer.expect(&Token::Dot)?;
        let (method, mp, _) = lexer.expect_ident()?;
        match method.as_str() {
            "Url" => {
                lexer.expect(&Token::LParen)?;
                let (url, _, _) = lexer.expect_string()?;
                lexer.expect(&Token::RParen)?;
                Ok(BinaryReference::Url(Url { value: url }))
            }
            "Inline" => {
                lexer.expect(&Token::LParen)?;
                // Array[Byte](1, 2, 3)
                let (arr_ident, ap, _) = lexer.expect_ident()?;
                if arr_ident != "Array" {
                    return Err(perr(ap, &format!("expected 'Array', got '{arr_ident}'")));
                }
                lexer.expect(&Token::LBrack)?;
                let (byte_ident, bp, _) = lexer.expect_ident()?;
                if byte_ident != "Byte" {
                    return Err(perr(bp, &format!("expected 'Byte', got '{byte_ident}'")));
                }
                lexer.expect(&Token::RBrack)?;
                lexer.expect(&Token::LParen)?;
                let mut data = Vec::new();
                while *lexer.peek()? != Token::RParen {
                    if !data.is_empty() {
                        lexer.expect(&Token::Comma)?;
                        if *lexer.peek()? == Token::RParen {
                            break;
                        }
                    }
                    let b = parse_uint(lexer)? as u8;
                    data.push(b);
                }
                lexer.expect(&Token::RParen)?;
                lexer.expect(&Token::Comma)?;
                let (mime, _, _) = lexer.expect_string()?;
                lexer.expect(&Token::RParen)?;
                Ok(BinaryReference::Inline(BinarySource {
                    data,
                    binary_type: BinaryType { mime_type: mime },
                }))
            }
            _ => Err(perr(
                mp,
                &format!("unknown UnstructuredBinaryValue method '{method}'"),
            )),
        }
    }
}

/// Parse record fields with `=` separator (named arguments).
/// Fields are specified as `fieldName = value, fieldName2 = value2`.
fn parse_record_fields(
    lexer: &mut Lexer,
    tr: &golem_wasm::analysis::TypeRecord,
    close: &Token,
) -> Result<Vec<Value>, ParseError> {
    let field_map: Vec<(String, usize)> = tr
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.to_lower_camel_case(), i))
        .collect();
    let mut values: Vec<Option<Value>> = vec![None; tr.fields.len()];

    if *lexer.peek()? == *close {
        let pos = lexer.position();
        return values
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.ok_or_else(|| perr(pos, &format!("missing field '{}'", tr.fields[i].name)))
            })
            .collect();
    }

    loop {
        let (fname, fp, _) = lexer.expect_ident()?;
        if *lexer.peek()? != Token::Eq {
            return Err(perr(
                fp,
                &format!("expected '=' after field name '{fname}'"),
            ));
        }
        lexer.expect(&Token::Eq)?;
        let field_idx = field_map
            .iter()
            .find(|(n, _)| *n == fname)
            .map(|(_, i)| *i)
            .ok_or_else(|| perr(fp, &format!("unknown field '{fname}'")))?;
        values[field_idx] =
            Some(parse_cm_value::<ScalaDialect>(lexer, &tr.fields[field_idx].typ)?.value);
        if *lexer.peek()? == Token::Comma {
            lexer.next_token()?;
            if *lexer.peek()? == *close {
                break;
            }
        } else {
            break;
        }
    }

    let pos = lexer.position();
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            v.ok_or_else(|| perr(pos, &format!("missing field '{}'", tr.fields[i].name)))
        })
        .collect()
}
