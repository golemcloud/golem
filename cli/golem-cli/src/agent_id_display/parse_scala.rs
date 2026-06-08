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
use heck::{ToLowerCamelCase, ToUpperCamelCase};

pub(super) struct ScalaDialect;

impl Dialect for ScalaDialect {
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
        // For arity == 1: accept both Tuple1(a) and (a)
        if elements.len() == 1
            && let Token::Ident(id) = lexer.peek()?
            && id == "Tuple1"
        {
            lexer.next_token()?;
            lexer.expect(&Token::LParen)?;
            let v = parse_cm_value::<Self>(lexer, graph, &elements[0])?;
            lexer.expect(&Token::RParen)?;
            return Ok(SchemaValue::Tuple { elements: vec![v] });
        }
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
                && *id == camel
            {
                lexer.next_token()?;
            }
            lexer.expect(&Token::LParen)?;
            let values = parse_record_fields(lexer, graph, fields, &Token::RParen)?;
            lexer.expect(&Token::RParen)?;
            Ok(SchemaValue::Record { fields: values })
        } else {
            lexer.expect(&Token::LBrace)?;
            let values = parse_record_fields(lexer, graph, fields, &Token::RBrace)?;
            lexer.expect(&Token::RBrace)?;
            Ok(SchemaValue::Record { fields: values })
        }
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
                && *id == camel
            {
                lexer.next_token()?;
                lexer.expect(&Token::Dot)?;
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
                && *id == camel
            {
                lexer.next_token()?;
                lexer.expect(&Token::Dot)?;
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
            _ => Err(perr(p, &format!("expected Ok or Err, got '{ok_or_err}'"))),
        }
    }

    fn parse_flags(
        lexer: &mut Lexer,
        def_name: Option<&str>,
        flags: &[String],
    ) -> Result<SchemaValue, ParseError> {
        let (_open, close) = if let Some(name) = def_name {
            let camel = name.to_upper_camel_case();
            if let Token::Ident(id) = lexer.peek()?
                && *id == camel
            {
                lexer.next_token()?;
            }
            lexer.expect(&Token::LParen)?;
            (Token::LParen, Token::RParen)
        } else {
            lexer.expect(&Token::LBrace)?;
            (Token::LBrace, Token::RBrace)
        };
        let name_map: Vec<(String, usize)> = flags
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lower_camel_case(), i))
            .collect();
        let mut bits = vec![false; flags.len()];
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
                bits[*idx] = true;
            }
            parsed_count += 1;
        }
        lexer.expect(&close)?;
        Ok(SchemaValue::Flags { bits })
    }

    fn parse_list(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        element: &SchemaType,
    ) -> Result<SchemaValue, ParseError> {
        if let Token::Ident(id) = lexer.peek()?
            && id == "List"
        {
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
                items.push(parse_cm_value::<Self>(lexer, graph, element)?);
            }
            lexer.expect(&Token::RParen)?;
            return Ok(SchemaValue::List { elements: items });
        }
        lexer.expect(&Token::LBrack)?;
        let mut items = Vec::new();
        while *lexer.peek()? != Token::RBrack {
            if !items.is_empty() {
                lexer.expect(&Token::Comma)?;
                if *lexer.peek()? == Token::RBrack {
                    break;
                }
            }
            items.push(parse_cm_value::<Self>(lexer, graph, element)?);
        }
        lexer.expect(&Token::RBrack)?;
        Ok(SchemaValue::List { elements: items })
    }
}

fn parse_record_fields(
    lexer: &mut Lexer,
    graph: &SchemaGraph,
    fields: &[NamedFieldType],
    close: &Token,
) -> Result<Vec<SchemaValue>, ParseError> {
    let field_map: Vec<(String, usize)> = fields
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.to_lower_camel_case(), i))
        .collect();
    let mut values: Vec<Option<SchemaValue>> = (0..fields.len()).map(|_| None).collect();
    if *lexer.peek()? == *close {
        let pos = lexer.position();
        return values
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.ok_or_else(|| perr(pos, &format!("missing field '{}'", fields[i].name)))
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
        values[field_idx] = Some(parse_cm_value::<ScalaDialect>(
            lexer,
            graph,
            &fields[field_idx].body,
        )?);
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
        .map(|(i, v)| v.ok_or_else(|| perr(pos, &format!("missing field '{}'", fields[i].name))))
        .collect()
}
