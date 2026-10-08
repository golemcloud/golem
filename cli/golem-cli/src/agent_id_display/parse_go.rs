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

//! Reads the Go syntax `render_go` writes. Names match the schema exactly,
//! bare or quoted. Beyond the rendered forms it accepts what a Go author may
//! paste from source: composite literals with their type (`[]int32{1, 2}`,
//! `Order{…}`), `golem.Some(x)`, `golem.None[T]()`, `golem.Ok(x)` and
//! `golem.Err(e)`, and durations such as `30 * time.Second`.

use super::lexer::{Lexer, Token};
use super::parse_common::{
    Dialect, ParseError, duration_value_from_nanos, duration_value_from_text, parse_cm_value,
    parse_rich_constructor_body, parse_uint, perr,
};
use super::resolve_named_ref;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, VariantValuePayload};

pub(super) struct GoDialect;

/// Reads a name: a bare identifier or a quoted string.
fn expect_name(lexer: &mut Lexer) -> Result<(String, usize), ParseError> {
    let (tok, pos, _) = lexer.next_token()?;
    match tok {
        Token::Ident(s) | Token::StringLit(s) => Ok((s, pos)),
        other => Err(perr(pos, &format!("expected a name, got {other:?}"))),
    }
}

/// Skips the type a Go composite literal may carry before its `{`: `[]int32`,
/// `[3]string`, `map[string]int64`, `Order`, `shop.Order`, `golem.Tuple2[A, B]`.
/// Nothing is consumed unless a `{` follows the type.
fn skip_type_prefix(lexer: &mut Lexer) -> Result<(), ParseError> {
    if !matches!(lexer.peek()?, Token::LBrack | Token::Ident(_) | Token::Star) {
        return Ok(());
    }
    let mut depth = 0i32;
    loop {
        match lexer.peek()? {
            Token::LBrace if depth == 0 => return Ok(()),
            Token::LBrack => depth += 1,
            Token::RBrack => depth -= 1,
            Token::Ident(_)
            | Token::Dot
            | Token::Star
            | Token::UintLit(_)
            | Token::LParen
            | Token::RParen => {}
            Token::Comma if depth > 0 => {}
            other => {
                let other = other.clone();
                return Err(perr(
                    lexer.position(),
                    &format!("expected a composite literal type, got {other:?}"),
                ));
            }
        }
        lexer.next_token()?;
    }
}

/// Consumes an optional `golem.` qualifier.
fn skip_golem_qualifier(lexer: &mut Lexer) -> Result<(), ParseError> {
    if let Token::Ident(id) = lexer.peek()?
        && id == "golem"
    {
        lexer.next_token()?;
        lexer.expect(&Token::Dot)?;
    }
    Ok(())
}

/// Parses comma-separated elements up to the closing `}`; the opening brace is
/// already consumed. A trailing comma is accepted.
fn parse_braced<T>(
    lexer: &mut Lexer,
    mut element: impl FnMut(&mut Lexer) -> Result<T, ParseError>,
) -> Result<Vec<T>, ParseError> {
    let mut out = Vec::new();
    while *lexer.peek()? != Token::RBrace {
        if !out.is_empty() {
            lexer.expect(&Token::Comma)?;
            if *lexer.peek()? == Token::RBrace {
                break;
            }
        }
        out.push(element(lexer)?);
    }
    lexer.expect(&Token::RBrace)?;
    Ok(out)
}

fn parse_record_body(
    lexer: &mut Lexer,
    graph: &SchemaGraph,
    fields: &[NamedFieldType],
) -> Result<SchemaValue, ParseError> {
    lexer.expect(&Token::LBrace)?;
    let mut values: Vec<Option<SchemaValue>> = (0..fields.len()).map(|_| None).collect();
    parse_braced(lexer, |lexer| {
        let (name, pos) = expect_name(lexer)?;
        lexer.expect(&Token::Colon)?;
        let idx = fields
            .iter()
            .position(|f| f.name == name)
            .ok_or_else(|| perr(pos, &format!("unknown field '{name}'")))?;
        if values[idx].is_some() {
            return Err(perr(pos, &format!("field '{name}' given twice")));
        }
        values[idx] = Some(parse_cm_value::<GoDialect>(
            lexer,
            graph,
            &fields[idx].body,
        )?);
        Ok(())
    })?;
    let pos = lexer.position();
    let fields = values
        .into_iter()
        .enumerate()
        .map(|(i, v)| v.ok_or_else(|| perr(pos, &format!("missing field '{}'", fields[i].name))))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SchemaValue::Record { fields })
}

impl Dialect for GoDialect {
    fn parse_char(lexer: &mut Lexer) -> Result<char, ParseError> {
        let (tok, pos, _) = lexer.next_token()?;
        match tok {
            Token::CharLit(c) => Ok(c),
            _ => Err(perr(pos, "expected rune literal")),
        }
    }

    fn parse_list(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        element: &SchemaType,
    ) -> Result<SchemaValue, ParseError> {
        skip_type_prefix(lexer)?;
        lexer.expect(&Token::LBrace)?;
        let elements = parse_braced(lexer, |lexer| parse_cm_value::<Self>(lexer, graph, element))?;
        Ok(SchemaValue::List { elements })
    }

    fn parse_tuple(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        elements: &[SchemaType],
    ) -> Result<SchemaValue, ParseError> {
        skip_type_prefix(lexer)?;
        lexer.expect(&Token::LBrace)?;
        let mut types = elements.iter();
        let items = parse_braced(lexer, |lexer| {
            let ty = types
                .next()
                .ok_or_else(|| perr(lexer.position(), "too many tuple elements"))?;
            parse_cm_value::<Self>(lexer, graph, ty)
        })?;
        if items.len() != elements.len() {
            return Err(perr(
                lexer.position(),
                &format!(
                    "tuple has {} element(s), expected {}",
                    items.len(),
                    elements.len()
                ),
            ));
        }
        Ok(SchemaValue::Tuple { elements: items })
    }

    fn parse_record(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        _def_name: Option<&str>,
        fields: &[NamedFieldType],
    ) -> Result<SchemaValue, ParseError> {
        skip_type_prefix(lexer)?;
        parse_record_body(lexer, graph, fields)
    }

    fn parse_variant(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        _def_name: Option<&str>,
        cases: &[VariantCaseType],
    ) -> Result<SchemaValue, ParseError> {
        let (name, pos) = expect_name(lexer)?;
        let (case_idx, case_def) = cases
            .iter()
            .enumerate()
            .find(|(_, c)| c.name == name)
            .ok_or_else(|| perr(pos, &format!("unknown variant case '{name}'")))?;
        let payload = match &case_def.payload {
            Some(case_ty) => {
                let (resolved, _) = resolve_named_ref(graph, case_ty);
                let value = match (lexer.peek()?, resolved) {
                    (Token::LBrace, SchemaType::Record { fields, .. }) => {
                        parse_record_body(lexer, graph, fields)?
                    }
                    _ => {
                        lexer.expect(&Token::LParen)?;
                        let v = parse_cm_value::<Self>(lexer, graph, case_ty)?;
                        lexer.expect(&Token::RParen)?;
                        v
                    }
                };
                Some(Box::new(value))
            }
            None => None,
        };
        Ok(SchemaValue::Variant(VariantValuePayload {
            case: case_idx as u32,
            payload,
        }))
    }

    fn parse_enum(
        lexer: &mut Lexer,
        _def_name: Option<&str>,
        cases: &[String],
    ) -> Result<SchemaValue, ParseError> {
        let (name, pos) = expect_name(lexer)?;
        let case = cases
            .iter()
            .position(|c| *c == name)
            .ok_or_else(|| perr(pos, &format!("unknown enum case '{name}'")))?;
        Ok(SchemaValue::Enum { case: case as u32 })
    }

    /// `nil` (or `golem.None[T]()`) is absent. A present value is written bare,
    /// or as `Some(x)` / `golem.Some(x)`; when the inner type is itself an
    /// option, the outer level must use `Some`.
    fn parse_option(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        inner: &SchemaType,
    ) -> Result<SchemaValue, ParseError> {
        let qualified = matches!(lexer.peek()?, Token::Ident(id) if id == "golem");
        skip_golem_qualifier(lexer)?;
        match lexer.peek()? {
            Token::Ident(id) if id == "nil" && !qualified => {
                lexer.next_token()?;
                return Ok(SchemaValue::Option { inner: None });
            }
            Token::Null | Token::Undefined if !qualified => {
                lexer.next_token()?;
                return Ok(SchemaValue::Option { inner: None });
            }
            Token::Ident(id) if id == "None" => {
                lexer.next_token()?;
                if *lexer.peek()? == Token::LBrack {
                    lexer.next_token()?;
                    let mut depth = 1;
                    while depth > 0 {
                        match lexer.next_token()?.0 {
                            Token::LBrack => depth += 1,
                            Token::RBrack => depth -= 1,
                            Token::Eof => return Err(perr(lexer.position(), "unterminated type")),
                            _ => {}
                        }
                    }
                }
                if *lexer.peek()? == Token::LParen {
                    lexer.next_token()?;
                    lexer.expect(&Token::RParen)?;
                }
                return Ok(SchemaValue::Option { inner: None });
            }
            Token::Ident(id) if id == "Some" => {
                lexer.next_token()?;
                lexer.expect(&Token::LParen)?;
                let v = parse_cm_value::<Self>(lexer, graph, inner)?;
                lexer.expect(&Token::RParen)?;
                return Ok(SchemaValue::Option {
                    inner: Some(Box::new(v)),
                });
            }
            _ => {}
        }
        if qualified {
            return Err(perr(lexer.position(), "expected golem.Some or golem.None"));
        }
        let (inner_resolved, _) = resolve_named_ref(graph, inner);
        if matches!(inner_resolved, SchemaType::Option { .. }) {
            return Err(perr(
                lexer.position(),
                "a present value of a nested option is written Some(…)",
            ));
        }
        let v = parse_cm_value::<Self>(lexer, graph, inner)?;
        Ok(SchemaValue::Option {
            inner: Some(Box::new(v)),
        })
    }

    fn parse_result(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        spec: &ResultSpec,
    ) -> Result<SchemaValue, ParseError> {
        skip_golem_qualifier(lexer)?;
        let (ident, pos, _) = lexer.expect_ident()?;
        let (is_ok, ty) = match ident.as_str() {
            "Ok" => (true, &spec.ok),
            "Err" => (false, &spec.err),
            _ => return Err(perr(pos, &format!("expected Ok or Err, got '{ident}'"))),
        };
        // golem.Ok[T, E](x) names its type parameters; they are implied here.
        if *lexer.peek()? == Token::LBrack {
            lexer.next_token()?;
            let mut depth = 1;
            while depth > 0 {
                match lexer.next_token()?.0 {
                    Token::LBrack => depth += 1,
                    Token::RBrack => depth -= 1,
                    Token::Eof => return Err(perr(lexer.position(), "unterminated type")),
                    _ => {}
                }
            }
        }
        lexer.expect(&Token::LParen)?;
        let value = match ty {
            Some(t) => Some(Box::new(parse_cm_value::<Self>(lexer, graph, t)?)),
            None => {
                // A unit side may be spelled with the SDK's unit value.
                if let Token::Ident(id) = lexer.peek()?
                    && (id == "golem" || id == "Unit")
                {
                    skip_golem_qualifier(lexer)?;
                    lexer.expect_ident()?;
                    lexer.expect(&Token::LBrace)?;
                    lexer.expect(&Token::RBrace)?;
                } else if *lexer.peek()? == Token::LBrace {
                    lexer.next_token()?;
                    lexer.expect(&Token::RBrace)?;
                }
                None
            }
        };
        lexer.expect(&Token::RParen)?;
        Ok(SchemaValue::Result(if is_ok {
            ResultValuePayload::Ok { value }
        } else {
            ResultValuePayload::Err { value }
        }))
    }

    fn parse_flags(
        lexer: &mut Lexer,
        _def_name: Option<&str>,
        flags: &[String],
    ) -> Result<SchemaValue, ParseError> {
        skip_type_prefix(lexer)?;
        lexer.expect(&Token::LBrace)?;
        let mut bits = vec![false; flags.len()];
        parse_braced(lexer, |lexer| {
            let (name, pos) = expect_name(lexer)?;
            let idx = flags
                .iter()
                .position(|f| *f == name)
                .ok_or_else(|| perr(pos, &format!("unknown flag '{name}'")))?;
            lexer.expect(&Token::Colon)?;
            let (tok, bpos, _) = lexer.next_token()?;
            match tok {
                Token::BoolLit(b) => bits[idx] = b,
                _ => return Err(perr(bpos, "expected true or false")),
            }
            Ok(())
        })?;
        Ok(SchemaValue::Flags { bits })
    }

    /// Durations are written `Duration("PT30S")`; Go's own `30 * time.Second`
    /// (and a bare `time.Second`) is accepted too, for nanosecond through hour
    /// units with a non-negative integer factor.
    fn parse_duration(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
        match lexer.peek()? {
            Token::Ident(id) if id == "Duration" => {
                lexer.next_token()?;
                let pos = lexer.position();
                let body = parse_rich_constructor_body(lexer)?;
                duration_value_from_text(pos, &body)
            }
            _ => {
                let pos = lexer.position();
                let factor = if matches!(lexer.peek()?, Token::UintLit(_)) {
                    let n = parse_uint(lexer)?;
                    lexer.expect(&Token::Star)?;
                    n
                } else {
                    1
                };
                let (pkg, ppos, _) = lexer.expect_ident()?;
                if pkg != "time" {
                    return Err(perr(
                        ppos,
                        &format!("expected Duration(\"…\") or a time unit, got '{pkg}'"),
                    ));
                }
                lexer.expect(&Token::Dot)?;
                let (unit, upos, _) = lexer.expect_ident()?;
                let nanos_per: i64 = match unit.as_str() {
                    "Nanosecond" => 1,
                    "Microsecond" => 1_000,
                    "Millisecond" => 1_000_000,
                    "Second" => 1_000_000_000,
                    "Minute" => 60_000_000_000,
                    "Hour" => 3_600_000_000_000,
                    _ => return Err(perr(upos, &format!("unknown time unit '{unit}'"))),
                };
                let nanos = (factor as i128)
                    .checked_mul(nanos_per as i128)
                    .and_then(|v| i64::try_from(v).ok())
                    .ok_or_else(|| perr(pos, "duration overflows int64 nanoseconds"))?;
                Ok(duration_value_from_nanos(nanos))
            }
        }
    }
}
