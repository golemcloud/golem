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
use super::parse_common::{
    Dialect, ParseError, duration_value_from_nanos, duration_value_from_text, parse_cm_value,
    parse_quantity_constructor, parse_rich_constructor_body, parse_uint, perr,
    quantity_value_from_text,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, VariantValuePayload};
use heck::ToLowerCamelCase;

pub(super) struct TsDialect;

impl Dialect for TsDialect {
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
        graph: &SchemaGraph,
        elements: &[SchemaType],
    ) -> Result<SchemaValue, ParseError> {
        lexer.expect(&Token::LBrack)?;
        let mut items = Vec::new();
        for (i, ty) in elements.iter().enumerate() {
            if i > 0 {
                lexer.expect(&Token::Comma)?;
            }
            items.push(parse_cm_value::<Self>(lexer, graph, ty)?);
        }
        lexer.expect(&Token::RBrack)?;
        Ok(SchemaValue::Tuple { elements: items })
    }

    fn parse_record(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        _def_name: Option<&str>,
        fields: &[NamedFieldType],
    ) -> Result<SchemaValue, ParseError> {
        lexer.expect(&Token::LBrace)?;
        let name_map: Vec<(String, usize)> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.to_lower_camel_case(), i))
            .collect();
        let mut values: Vec<Option<SchemaValue>> = (0..fields.len()).map(|_| None).collect();
        while *lexer.peek()? != Token::RBrace {
            if values.iter().any(|f| f.is_some()) {
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
            values[*idx] = Some(parse_cm_value::<Self>(lexer, graph, &fields[*idx].body)?);
        }
        lexer.expect(&Token::RBrace)?;
        let out: Result<Vec<SchemaValue>, _> = values
            .into_iter()
            .enumerate()
            .map(|(i, v)| v.ok_or_else(|| perr(0, &format!("missing field '{}'", fields[i].name))))
            .collect();
        Ok(SchemaValue::Record { fields: out? })
    }

    fn parse_variant(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        _def_name: Option<&str>,
        cases: &[VariantCaseType],
    ) -> Result<SchemaValue, ParseError> {
        lexer.expect(&Token::LBrace)?;
        expect_ident_key(lexer, "tag")?;
        let (case_name, pos, _) = lexer.expect_string()?;
        let case_idx = cases
            .iter()
            .position(|c| c.name == case_name)
            .ok_or_else(|| perr(pos, &format!("unknown variant case '{case_name}'")))?;
        let payload = if *lexer.peek()? == Token::Comma {
            lexer.next_token()?;
            if *lexer.peek()? == Token::RBrace {
                None
            } else {
                expect_ident_key(lexer, "value")?;
                match &cases[case_idx].payload {
                    Some(t) => Some(parse_cm_value::<Self>(lexer, graph, t)?),
                    None => None,
                }
            }
        } else {
            None
        };
        if *lexer.peek()? == Token::Comma {
            lexer.next_token()?;
        }
        lexer.expect(&Token::RBrace)?;
        Ok(SchemaValue::Variant(VariantValuePayload {
            case: case_idx as u32,
            payload: payload.map(Box::new),
        }))
    }

    fn parse_enum(
        lexer: &mut Lexer,
        _def_name: Option<&str>,
        cases: &[String],
    ) -> Result<SchemaValue, ParseError> {
        let (s, pos, _) = lexer.expect_string()?;
        let idx = cases
            .iter()
            .position(|c| *c == s)
            .ok_or_else(|| perr(pos, &format!("unknown enum case '{s}'")))?;
        Ok(SchemaValue::Enum { case: idx as u32 })
    }

    fn parse_option(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        inner: &SchemaType,
    ) -> Result<SchemaValue, ParseError> {
        let is_nested = matches!(inner, SchemaType::Option { .. });
        match lexer.peek()? {
            Token::Null | Token::Undefined => {
                lexer.next_token()?;
                Ok(SchemaValue::Option { inner: None })
            }
            Token::LBrace if is_nested => {
                lexer.next_token()?;
                let (key, pos, _) = lexer.expect_ident()?;
                if key != "some" {
                    return Err(perr(pos, &format!("expected 'some', got '{key}'")));
                }
                lexer.expect(&Token::Colon)?;
                let value = parse_cm_value::<Self>(lexer, graph, inner)?;
                lexer.expect(&Token::RBrace)?;
                Ok(SchemaValue::Option {
                    inner: Some(Box::new(value)),
                })
            }
            _ => {
                let value = parse_cm_value::<Self>(lexer, graph, inner)?;
                Ok(SchemaValue::Option {
                    inner: Some(Box::new(value)),
                })
            }
        }
    }

    fn parse_result(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        spec: &ResultSpec,
    ) -> Result<SchemaValue, ParseError> {
        lexer.expect(&Token::LBrace)?;
        let (key, pos, _) = lexer.expect_ident()?;
        lexer.expect(&Token::Colon)?;
        let result = match key.as_str() {
            "ok" => {
                let val = match &spec.ok {
                    Some(ok_ty) => Some(Box::new(parse_cm_value::<Self>(lexer, graph, ok_ty)?)),
                    None => {
                        if matches!(lexer.peek()?, Token::Null | Token::Undefined) {
                            lexer.next_token()?;
                        }
                        None
                    }
                };
                ResultValuePayload::Ok { value: val }
            }
            "error" => {
                let val = match &spec.err {
                    Some(err_ty) => Some(Box::new(parse_cm_value::<Self>(lexer, graph, err_ty)?)),
                    None => {
                        if matches!(lexer.peek()?, Token::Null | Token::Undefined) {
                            lexer.next_token()?;
                        }
                        None
                    }
                };
                ResultValuePayload::Err { value: val }
            }
            _ => return Err(perr(pos, &format!("expected 'ok' or 'error', got '{key}'"))),
        };
        if *lexer.peek()? == Token::Comma {
            lexer.next_token()?;
        }
        lexer.expect(&Token::RBrace)?;
        Ok(SchemaValue::Result(result))
    }

    fn parse_flags(
        lexer: &mut Lexer,
        _def_name: Option<&str>,
        flags: &[String],
    ) -> Result<SchemaValue, ParseError> {
        lexer.expect(&Token::LBrace)?;
        let name_map: Vec<(String, usize)> = flags
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lower_camel_case(), i))
            .collect();
        let mut bits = vec![false; flags.len()];
        while *lexer.peek()? != Token::RBrace {
            if bits.iter().any(|f| *f) {
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
                bits[*idx] = true;
            }
        }
        lexer.expect(&Token::RBrace)?;
        Ok(SchemaValue::Flags { bits })
    }

    /// TypeScript quantities accept the native `Nn * unit` literal (e.g.
    /// `5n * kg`, `-5n * kg`) in addition to the `Quantity("5kg")` constructor.
    /// Only integer BigInt magnitudes and identifier units are recognised
    /// natively; decimals and complex units stay constructor-only.
    fn parse_quantity(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
        if matches!(lexer.peek()?, Token::IntLit(_) | Token::UintLit(_)) {
            let (_, start, end) = lexer.next_token()?;
            let number = lexer.slice(start, end).to_string();
            if !matches!(lexer.peek()?, Token::Ident(id) if id == "n") {
                return Err(perr(
                    lexer.position(),
                    "expected 'n' BigInt suffix in quantity literal",
                ));
            }
            lexer.next_token()?;
            lexer.expect(&Token::Star)?;
            let (unit, _, _) = lexer.expect_ident()?;
            quantity_value_from_text(start, &format!("{number}{unit}"))
        } else {
            parse_quantity_constructor(lexer)
        }
    }

    /// TypeScript durations accept the native `Duration.<unit>(N)` family
    /// (`nanoseconds`/`microseconds`/`milliseconds`/`seconds`/`minutes`/`hours`,
    /// with a non-negative integer or BigInt argument) in addition to the
    /// `Duration("PT30S")` constructor.
    fn parse_duration(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
        let (name, pos, _) = lexer.expect_ident()?;
        if name != "Duration" {
            return Err(perr(
                pos,
                &format!("expected 'Duration' constructor or literal, got '{name}'"),
            ));
        }
        match lexer.peek()? {
            Token::Dot => {
                lexer.next_token()?;
                let (unit, upos, _) = lexer.expect_ident()?;
                let factor: i64 = match unit.as_str() {
                    "nanoseconds" => 1,
                    "microseconds" => 1_000,
                    "milliseconds" => 1_000_000,
                    "seconds" => 1_000_000_000,
                    "minutes" => 60 * 1_000_000_000,
                    "hours" => 3_600 * 1_000_000_000,
                    _ => return Err(perr(upos, &format!("unknown Duration unit '{unit}'"))),
                };
                lexer.expect(&Token::LParen)?;
                let n = parse_uint(lexer)?;
                if matches!(lexer.peek()?, Token::Ident(id) if id == "n") {
                    lexer.next_token()?;
                }
                lexer.expect(&Token::RParen)?;
                let nanos = (n as i128)
                    .checked_mul(factor as i128)
                    .and_then(|v| i64::try_from(v).ok())
                    .ok_or_else(|| perr(upos, "duration literal overflows i64 nanoseconds"))?;
                Ok(duration_value_from_nanos(nanos))
            }
            Token::LParen => {
                let bpos = lexer.position();
                let body = parse_rich_constructor_body(lexer)?;
                duration_value_from_text(bpos, &body)
            }
            other => {
                let other = other.clone();
                Err(perr(
                    lexer.position(),
                    &format!("expected '.' or '(' after 'Duration', got {other:?}"),
                ))
            }
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
