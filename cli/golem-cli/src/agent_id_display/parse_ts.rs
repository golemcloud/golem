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
use heck::ToLowerCamelCase;

pub fn parse_data_value_ts(input: &str, schema: &DataSchema) -> Result<DataValue, ParseError> {
    parse_common::parse_data_value::<TsDialect>(input, schema)
}

pub(super) struct TsDialect;

impl Dialect for TsDialect {
    fn normalize_field_name(name: &str) -> String {
        name.to_lower_camel_case()
    }

    fn parse_char(lexer: &mut Lexer) -> Result<char, ParseError> {
        let (s, pos, _) = lexer.expect_string()?;
        let mut chars = s.chars();
        let ch = chars
            .next()
            .ok_or_else(|| perr(pos, "empty string for char"))?;
        if chars.next().is_some() {
            return Err(perr(pos, "expected single character"));
        }
        Ok(ch)
    }

    fn parse_tuple(
        lexer: &mut Lexer,
        tt: &golem_wasm::analysis::TypeTuple,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        lexer.expect(&Token::LBrack)?;
        let mut items = Vec::new();
        for (i, item_type) in tt.items.iter().enumerate() {
            if i > 0 {
                lexer.expect(&Token::Comma)?;
            }
            items.push(parse_cm_value::<Self>(lexer, item_type)?.value);
        }
        lexer.expect(&Token::RBrack)?;
        Ok(ValueAndType::new(Value::Tuple(items), typ.clone()))
    }

    fn parse_record(
        lexer: &mut Lexer,
        tr: &golem_wasm::analysis::TypeRecord,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        lexer.expect(&Token::LBrace)?;
        let name_map: Vec<(String, usize)> = tr
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.to_lower_camel_case(), i))
            .collect();
        let mut fields: Vec<Option<Value>> = vec![None; tr.fields.len()];
        while *lexer.peek()? != Token::RBrace {
            if fields.iter().any(|f| f.is_some()) {
                lexer.expect(&Token::Comma)?;
                if *lexer.peek()? == Token::RBrace {
                    break;
                }
            }
            let (key, pos, _) = lexer.expect_ident()?;
            lexer.expect(&Token::Colon)?;
            let (_, idx) = name_map
                .iter()
                .find(|(n, _)| *n == key)
                .ok_or_else(|| perr(pos, &format!("unknown field '{key}'")))?;
            fields[*idx] = Some(parse_cm_value::<Self>(lexer, &tr.fields[*idx].typ)?.value);
        }
        lexer.expect(&Token::RBrace)?;
        let values = fields
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.ok_or_else(|| perr(0, &format!("missing field '{}'", tr.fields[i].name)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ValueAndType::new(Value::Record(values), typ.clone()))
    }

    fn parse_variant(
        lexer: &mut Lexer,
        tv: &golem_wasm::analysis::TypeVariant,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        lexer.expect(&Token::LBrace)?;
        expect_ident_key(lexer, "tag")?;
        let (case_name, pos, _) = lexer.expect_string()?;
        let case_idx = tv
            .cases
            .iter()
            .position(|c| c.name == case_name)
            .ok_or_else(|| perr(pos, &format!("unknown variant case '{case_name}'")))?;
        let case_value = if *lexer.peek()? == Token::Comma {
            lexer.next_token()?;
            if *lexer.peek()? == Token::RBrace {
                None
            } else {
                expect_ident_key(lexer, "value")?;
                tv.cases[case_idx]
                    .typ
                    .as_ref()
                    .map(|t| parse_cm_value::<Self>(lexer, t).map(|vt| vt.value))
                    .transpose()?
            }
        } else {
            None
        };
        if *lexer.peek()? == Token::Comma {
            lexer.next_token()?;
        }
        lexer.expect(&Token::RBrace)?;
        Ok(ValueAndType::new(
            Value::Variant {
                case_idx: case_idx as u32,
                case_value: case_value.map(Box::new),
            },
            typ.clone(),
        ))
    }

    fn parse_enum(
        lexer: &mut Lexer,
        te: &golem_wasm::analysis::TypeEnum,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        let (s, pos, _) = lexer.expect_string()?;
        let idx = te
            .cases
            .iter()
            .position(|c| *c == s)
            .ok_or_else(|| perr(pos, &format!("unknown enum case '{s}'")))?;
        Ok(ValueAndType::new(Value::Enum(idx as u32), typ.clone()))
    }

    fn parse_option(
        lexer: &mut Lexer,
        to: &golem_wasm::analysis::TypeOption,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        let is_nested = matches!(&*to.inner, AnalysedType::Option(_));
        match lexer.peek()? {
            Token::Null | Token::Undefined => {
                lexer.next_token()?;
                Ok(ValueAndType::new(Value::Option(None), typ.clone()))
            }
            Token::LBrace if is_nested => {
                lexer.next_token()?;
                let (key, pos, _) = lexer.expect_ident()?;
                if key != "some" {
                    return Err(perr(pos, &format!("expected 'some', got '{key}'")));
                }
                lexer.expect(&Token::Colon)?;
                let inner = parse_cm_value::<Self>(lexer, &to.inner)?;
                lexer.expect(&Token::RBrace)?;
                Ok(ValueAndType::new(
                    Value::Option(Some(Box::new(inner.value))),
                    typ.clone(),
                ))
            }
            _ => {
                let inner = parse_cm_value::<Self>(lexer, &to.inner)?;
                Ok(ValueAndType::new(
                    Value::Option(Some(Box::new(inner.value))),
                    typ.clone(),
                ))
            }
        }
    }

    fn parse_result(
        lexer: &mut Lexer,
        tr: &golem_wasm::analysis::TypeResult,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        lexer.expect(&Token::LBrace)?;
        let (key, pos, _) = lexer.expect_ident()?;
        lexer.expect(&Token::Colon)?;
        let result = match key.as_str() {
            "ok" => {
                let val = match &tr.ok {
                    Some(ok_type) => Some(Box::new(parse_cm_value::<Self>(lexer, ok_type)?.value)),
                    None => {
                        if matches!(lexer.peek()?, Token::Null | Token::Undefined) {
                            lexer.next_token()?;
                        }
                        None
                    }
                };
                Value::Result(Ok(val))
            }
            "error" => {
                let val = match &tr.err {
                    Some(err_type) => {
                        Some(Box::new(parse_cm_value::<Self>(lexer, err_type)?.value))
                    }
                    None => {
                        if matches!(lexer.peek()?, Token::Null | Token::Undefined) {
                            lexer.next_token()?;
                        }
                        None
                    }
                };
                Value::Result(Err(val))
            }
            _ => return Err(perr(pos, &format!("expected 'ok' or 'error', got '{key}'"))),
        };
        if *lexer.peek()? == Token::Comma {
            lexer.next_token()?;
        }
        lexer.expect(&Token::RBrace)?;
        Ok(ValueAndType::new(result, typ.clone()))
    }

    fn parse_flags(
        lexer: &mut Lexer,
        tf: &golem_wasm::analysis::TypeFlags,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        lexer.expect(&Token::LBrace)?;
        let name_map: Vec<(String, usize)> = tf
            .names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lower_camel_case(), i))
            .collect();
        let mut flags = vec![false; tf.names.len()];
        while *lexer.peek()? != Token::RBrace {
            if flags.iter().any(|f| *f) {
                lexer.expect(&Token::Comma)?;
                if *lexer.peek()? == Token::RBrace {
                    break;
                }
            }
            let (key, pos, _) = lexer.expect_ident()?;
            lexer.expect(&Token::Colon)?;
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
        }
        lexer.expect(&Token::RBrace)?;
        Ok(ValueAndType::new(Value::Flags(flags), typ.clone()))
    }

    fn parse_unstructured_text(lexer: &mut Lexer) -> Result<TextReference, ParseError> {
        lexer.expect(&Token::LBrace)?;
        expect_ident_key(lexer, "tag")?;
        let (tag, pos, _) = lexer.expect_string()?;
        lexer.expect(&Token::Comma)?;
        expect_ident_key(lexer, "val")?;
        let (val, _, _) = lexer.expect_string()?;
        let result = match tag.as_str() {
            "url" => TextReference::Url(Url { value: val }),
            "inline" => {
                let text_type = if *lexer.peek()? == Token::Comma {
                    lexer.next_token()?;
                    if *lexer.peek()? != Token::RBrace {
                        expect_ident_key(lexer, "lang")?;
                        let (lang, _, _) = lexer.expect_string()?;
                        Some(TextType {
                            language_code: lang,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };
                TextReference::Inline(TextSource {
                    data: val,
                    text_type,
                })
            }
            _ => {
                return Err(perr(
                    pos,
                    &format!("expected 'url' or 'inline', got '{tag}'"),
                ));
            }
        };
        if *lexer.peek()? == Token::Comma {
            lexer.next_token()?;
        }
        lexer.expect(&Token::RBrace)?;
        Ok(result)
    }

    fn parse_unstructured_binary(lexer: &mut Lexer) -> Result<BinaryReference, ParseError> {
        lexer.expect(&Token::LBrace)?;
        expect_ident_key(lexer, "tag")?;
        let (tag, pos, _) = lexer.expect_string()?;
        lexer.expect(&Token::Comma)?;
        match tag.as_str() {
            "url" => {
                expect_ident_key(lexer, "val")?;
                let (val, _, _) = lexer.expect_string()?;
                if *lexer.peek()? == Token::Comma {
                    lexer.next_token()?;
                }
                lexer.expect(&Token::RBrace)?;
                Ok(BinaryReference::Url(Url { value: val }))
            }
            "inline" => {
                expect_ident_key(lexer, "val")?;
                let (ident, ipos, _) = lexer.expect_ident()?;
                if ident != "Uint8Array" {
                    return Err(perr(ipos, &format!("expected 'Uint8Array', got '{ident}'")));
                }
                lexer.expect(&Token::LParen)?;
                lexer.expect(&Token::LBrack)?;
                let mut bytes = Vec::new();
                while *lexer.peek()? != Token::RBrack {
                    if !bytes.is_empty() {
                        lexer.expect(&Token::Comma)?;
                        if *lexer.peek()? == Token::RBrack {
                            break;
                        }
                    }
                    let b = parse_uint(lexer)? as u8;
                    bytes.push(b);
                }
                lexer.expect(&Token::RBrack)?;
                lexer.expect(&Token::RParen)?;
                lexer.expect(&Token::Comma)?;
                expect_ident_key(lexer, "mime")?;
                let (mime, _, _) = lexer.expect_string()?;
                if *lexer.peek()? == Token::Comma {
                    lexer.next_token()?;
                }
                lexer.expect(&Token::RBrace)?;
                Ok(BinaryReference::Inline(BinarySource {
                    data: bytes,
                    binary_type: BinaryType { mime_type: mime },
                }))
            }
            _ => Err(perr(
                pos,
                &format!("expected 'url' or 'inline', got '{tag}'"),
            )),
        }
    }
}

fn expect_ident_key(lexer: &mut Lexer, expected: &str) -> Result<(), ParseError> {
    let (name, pos, _) = lexer.expect_ident()?;
    if name != expected {
        return Err(perr(pos, &format!("expected '{expected}', got '{name}'")));
    }
    lexer.expect(&Token::Colon)?;
    Ok(())
}
