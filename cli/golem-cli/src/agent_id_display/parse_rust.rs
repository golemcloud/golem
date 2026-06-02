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
use super::parse_common::{Dialect, ParseError, parse_cm_value, perr};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, VariantValuePayload};
use heck::{ToSnakeCase, ToUpperCamelCase};

pub(super) struct RustDialect;

impl Dialect for RustDialect {
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
        graph: &SchemaGraph,
        elements: &[SchemaType],
    ) -> Result<SchemaValue, ParseError> {
        lexer.expect(&Token::LParen)?;
        let mut items = Vec::new();
        for (i, ty) in elements.iter().enumerate() {
            if i > 0 {
                lexer.expect(&Token::Comma)?;
            }
            items.push(parse_cm_value::<Self>(lexer, graph, ty)?);
        }
        lexer.expect(&Token::RParen)?;
        Ok(SchemaValue::Tuple { elements: items })
    }

    fn parse_record(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        def_name: Option<&str>,
        fields: &[NamedFieldType],
    ) -> Result<SchemaValue, ParseError> {
        if let Some(name) = def_name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && id == &camel
            {
                lexer.next_token()?;
            }
        }
        lexer.expect(&Token::LBrace)?;
        let field_map: Vec<(String, usize)> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.to_snake_case(), i))
            .collect();
        let mut values: Vec<Option<SchemaValue>> = (0..fields.len()).map(|_| None).collect();
        if *lexer.peek()? != Token::RBrace {
            loop {
                let (fname, fp, _) = lexer.expect_ident()?;
                lexer.expect(&Token::Colon)?;
                let idx = field_map
                    .iter()
                    .find(|(n, _)| *n == fname)
                    .map(|(_, i)| *i)
                    .ok_or_else(|| perr(fp, &format!("unknown field '{fname}'")))?;
                values[idx] = Some(parse_cm_value::<Self>(lexer, graph, &fields[idx].body)?);
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
        let out: Result<Vec<SchemaValue>, ParseError> = values
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.ok_or_else(|| perr(pos, &format!("missing field '{}'", fields[i].name)))
            })
            .collect();
        Ok(SchemaValue::Record { fields: out? })
    }

    fn parse_variant(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        def_name: Option<&str>,
        cases: &[VariantCaseType],
    ) -> Result<SchemaValue, ParseError> {
        if let Some(name) = def_name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && id == &camel
            {
                lexer.next_token()?;
                lexer.expect(&Token::DoubleColon)?;
            }
        }
        let (case_name, cp, _) = lexer.expect_ident()?;
        let (case_idx, case_def) = cases
            .iter()
            .enumerate()
            .find(|(_, c)| c.name.to_upper_camel_case() == case_name)
            .ok_or_else(|| perr(cp, &format!("unknown variant case '{case_name}'")))?;
        let payload = if let Some(case_ty) = &case_def.payload {
            lexer.expect(&Token::LParen)?;
            let v = parse_cm_value::<Self>(lexer, graph, case_ty)?;
            lexer.expect(&Token::RParen)?;
            Some(Box::new(v))
        } else {
            None
        };
        Ok(SchemaValue::Variant(VariantValuePayload {
            case: case_idx as u32,
            payload,
        }))
    }

    fn parse_enum(
        lexer: &mut Lexer,
        def_name: Option<&str>,
        cases: &[String],
    ) -> Result<SchemaValue, ParseError> {
        if let Some(name) = def_name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && id == &camel
            {
                lexer.next_token()?;
                lexer.expect(&Token::DoubleColon)?;
            }
        }
        let (case_name, cp, _) = lexer.expect_ident()?;
        let case_idx = cases
            .iter()
            .position(|c| c.to_upper_camel_case() == case_name)
            .ok_or_else(|| perr(cp, &format!("unknown enum case '{case_name}'")))?;
        Ok(SchemaValue::Enum {
            case: case_idx as u32,
        })
    }

    fn parse_option(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        inner: &SchemaType,
    ) -> Result<SchemaValue, ParseError> {
        let (ident, p, _) = lexer.expect_ident()?;
        match ident.as_str() {
            "None" => Ok(SchemaValue::Option { inner: None }),
            "Some" => {
                lexer.expect(&Token::LParen)?;
                let v = parse_cm_value::<Self>(lexer, graph, inner)?;
                lexer.expect(&Token::RParen)?;
                Ok(SchemaValue::Option {
                    inner: Some(Box::new(v)),
                })
            }
            _ => Err(perr(p, &format!("expected Some or None, got '{ident}'"))),
        }
    }

    fn parse_result(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        spec: &ResultSpec,
    ) -> Result<SchemaValue, ParseError> {
        let (ident, p, _) = lexer.expect_ident()?;
        match ident.as_str() {
            "Ok" => {
                lexer.expect(&Token::LParen)?;
                let v = if let Some(ok_ty) = &spec.ok {
                    Some(Box::new(parse_cm_value::<Self>(lexer, graph, ok_ty)?))
                } else {
                    if *lexer.peek()? == Token::LParen {
                        lexer.next_token()?;
                        lexer.expect(&Token::RParen)?;
                    }
                    None
                };
                lexer.expect(&Token::RParen)?;
                Ok(SchemaValue::Result(ResultValuePayload::Ok { value: v }))
            }
            "Err" => {
                lexer.expect(&Token::LParen)?;
                let v = if let Some(err_ty) = &spec.err {
                    Some(Box::new(parse_cm_value::<Self>(lexer, graph, err_ty)?))
                } else {
                    if *lexer.peek()? == Token::LParen {
                        lexer.next_token()?;
                        lexer.expect(&Token::RParen)?;
                    }
                    None
                };
                lexer.expect(&Token::RParen)?;
                Ok(SchemaValue::Result(ResultValuePayload::Err { value: v }))
            }
            _ => Err(perr(p, &format!("expected Ok or Err, got '{ident}'"))),
        }
    }

    fn parse_flags(
        lexer: &mut Lexer,
        def_name: Option<&str>,
        flags: &[String],
    ) -> Result<SchemaValue, ParseError> {
        if let Some(name) = def_name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && id == &camel
            {
                lexer.next_token()?;
            }
        }
        lexer.expect(&Token::LBrace)?;
        let mut bits = vec![false; flags.len()];
        if *lexer.peek()? != Token::RBrace {
            loop {
                let (fname, fp, _) = lexer.expect_ident()?;
                let idx = flags
                    .iter()
                    .position(|n| n.to_snake_case() == fname)
                    .ok_or_else(|| perr(fp, &format!("unknown flag '{fname}'")))?;
                bits[idx] = true;
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
        Ok(SchemaValue::Flags { bits })
    }
}
