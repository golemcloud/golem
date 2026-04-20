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
use heck::{ToSnakeCase, ToUpperCamelCase};

pub fn parse_data_value_moonbit(
    input: &str,
    schema: &DataSchema,
) -> Result<DataValue, ParseError> {
    parse_common::parse_data_value::<MoonBitDialect>(input, schema)
}

pub(super) struct MoonBitDialect;

impl Dialect for MoonBitDialect {
    fn normalize_field_name(name: &str) -> String {
        name.to_snake_case()
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
        // Accept optional TypeName:: prefix before the opening brace
        if let Some(name) = &tr.name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && id == &camel
            {
                lexer.next_token()?;
                lexer.expect(&Token::DoubleColon)?;
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
                    .ok_or_else(|| perr(fp, &format!("unknown field '{fname}'")))?;
                values[idx] = Some(parse_cm_value::<Self>(lexer, &tr.fields[idx].typ)?.value);
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
        let pos = lexer.position();
        let fields: Result<Vec<Value>, ParseError> = values
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.ok_or_else(|| perr(pos, &format!("missing field '{}'", tr.fields[i].name)))
            })
            .collect();
        Ok(ValueAndType::new(Value::Record(fields?), typ.clone()))
    }

    fn parse_variant(
        lexer: &mut Lexer,
        tv: &golem_wasm::analysis::TypeVariant,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        if let Some(name) = &tv.name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && id == &camel
            {
                lexer.next_token()?;
                lexer.expect(&Token::DoubleColon)?;
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
        if let Some(name) = &te.name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && id == &camel
            {
                lexer.next_token()?;
                lexer.expect(&Token::DoubleColon)?;
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
        let (ident, p, _) = lexer.expect_ident()?;
        match ident.as_str() {
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
            _ => Err(perr(p, &format!("expected Ok or Err, got '{ident}'"))),
        }
    }

    fn parse_flags(
        lexer: &mut Lexer,
        tf: &golem_wasm::analysis::TypeFlags,
        typ: &AnalysedType,
    ) -> Result<ValueAndType, ParseError> {
        // Accept optional TypeName:: prefix before the opening brace
        if let Some(name) = &tf.name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && id == &camel
            {
                lexer.next_token()?;
                lexer.expect(&Token::DoubleColon)?;
            }
        }
        lexer.expect(&Token::LBrace)?;
        let name_map: Vec<(String, usize)> = tf
            .names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_snake_case(), i))
            .collect();
        let mut flags = vec![false; tf.names.len()];
        if *lexer.peek()? != Token::RBrace {
            loop {
                let (fname, fp, _) = lexer.expect_ident()?;
                let idx = name_map
                    .iter()
                    .find(|(n, _)| *n == fname)
                    .map(|(_, i)| *i)
                    .ok_or_else(|| perr(fp, &format!("unknown flag '{fname}'")))?;
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
        Ok(ValueAndType::new(Value::Flags(flags), typ.clone()))
    }

    fn parse_unstructured_text(lexer: &mut Lexer) -> Result<TextReference, ParseError> {
        let (ident, p, _) = lexer.expect_ident()?;
        if ident != "UnstructuredText" {
            return Err(perr(
                p,
                &format!("expected 'UnstructuredText', got '{ident}'"),
            ));
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
                    return Err(perr(lp, &format!("expected 'Languages', got '{lang_ns}'")));
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
            _ => Err(perr(
                mp,
                &format!("unknown UnstructuredText method '{method}'"),
            )),
        }
    }

    fn parse_unstructured_binary(lexer: &mut Lexer) -> Result<BinaryReference, ParseError> {
        let (ident, p, _) = lexer.expect_ident()?;
        if ident != "UnstructuredBinary" {
            return Err(perr(
                p,
                &format!("expected 'UnstructuredBinary', got '{ident}'"),
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
                let (bytes_ident, bp, _) = lexer.expect_ident()?;
                if bytes_ident != "Bytes" {
                    return Err(perr(bp, &format!("expected 'Bytes', got '{bytes_ident}'")));
                }
                lexer.expect(&Token::DoubleColon)?;
                let (from_ident, fp, _) = lexer.expect_ident()?;
                if from_ident != "from_array" {
                    return Err(perr(
                        fp,
                        &format!("expected 'from_array', got '{from_ident}'"),
                    ));
                }
                lexer.expect(&Token::LParen)?;
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
                lexer.expect(&Token::RParen)?;
                lexer.expect(&Token::Comma)?;
                let (mime_ns, mnp, _) = lexer.expect_ident()?;
                if mime_ns != "MimeTypes" {
                    return Err(perr(mnp, &format!("expected 'MimeTypes', got '{mime_ns}'")));
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
                &format!("unknown UnstructuredBinary method '{method}'"),
            )),
        }
    }
}
