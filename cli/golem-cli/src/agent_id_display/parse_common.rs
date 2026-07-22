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
use chrono::DateTime;
use golem_common::schema::agent::NamedField;
use golem_common::schema::canonical::{
    binary as canon_binary, datetime as canon_datetime, duration as canon_duration,
    path as canon_path, permission_card as canon_permission_card, quantity as canon_quantity,
    quota_token as canon_quota_token, secret as canon_secret, text as canon_text, url as canon_url,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{
    NamedFieldType, ResultSpec, SchemaType, UnionBranch, VariantCaseType,
};
use golem_common::schema::schema_value::{DurationValuePayload, SchemaValue, UnionValuePayload};

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
/// The shared dispatcher [`parse_cm_value`] handles primitives, list, ref
/// resolution, and the canonical encoders for the rich semantic types
/// (text/binary/path/url/datetime/duration/quantity/secret/quota-token).
/// Dialect implementations cover the structural composites whose syntax
/// varies between languages.
pub(super) trait Dialect: Sized {
    fn parse_char(lexer: &mut Lexer) -> Result<char, ParseError>;

    fn parse_tuple(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        elements: &[SchemaType],
    ) -> Result<SchemaValue, ParseError>;

    fn parse_record(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        def_name: Option<&str>,
        fields: &[NamedFieldType],
    ) -> Result<SchemaValue, ParseError>;

    fn parse_variant(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        def_name: Option<&str>,
        cases: &[VariantCaseType],
    ) -> Result<SchemaValue, ParseError>;

    fn parse_enum(
        lexer: &mut Lexer,
        def_name: Option<&str>,
        cases: &[String],
    ) -> Result<SchemaValue, ParseError>;

    fn parse_option(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        inner: &SchemaType,
    ) -> Result<SchemaValue, ParseError>;

    fn parse_result(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        spec: &ResultSpec,
    ) -> Result<SchemaValue, ParseError>;

    fn parse_flags(
        lexer: &mut Lexer,
        def_name: Option<&str>,
        flags: &[String],
    ) -> Result<SchemaValue, ParseError>;

    fn parse_list(
        lexer: &mut Lexer,
        graph: &SchemaGraph,
        element: &SchemaType,
    ) -> Result<SchemaValue, ParseError> {
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

    // Rich semantic scalars. The default implementations accept the shared
    // constructor form `Name("payload")` (which every renderer emits);
    // dialects override these to *additionally* accept the language's native
    // literal syntax (e.g. Rust `5.kg()`, TypeScript `Duration.seconds(30)`).
    // `Url` and `Path` have no grounded native literal in any current SDK, so
    // they intentionally stay constructor-only in every dialect.

    fn parse_quantity(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
        parse_quantity_constructor(lexer)
    }

    fn parse_duration(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
        parse_duration_constructor(lexer)
    }

    fn parse_datetime(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
        parse_datetime_constructor(lexer)
    }

    fn parse_url(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
        parse_url_constructor(lexer)
    }

    fn parse_path(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
        parse_path_constructor(lexer)
    }
}

// ── Shared entry point for the agent-id parameter list ──────────────────────

pub(super) fn parse_input_schema_params<D: Dialect>(
    input: &str,
    graph: &SchemaGraph,
    fields: &[NamedField],
) -> Result<SchemaValue, ParseError> {
    let mut lexer = Lexer::new(input);
    let mut values = Vec::with_capacity(fields.len());
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            lexer.expect(&Token::Comma)?;
        }
        values.push(parse_cm_value::<D>(&mut lexer, graph, &field.schema)?);
    }
    let (tok, pos, _) = lexer.next_token()?;
    if tok != Token::Eof {
        return Err(ParseError {
            position: pos,
            message: format!("expected end of input, got {tok:?}"),
        });
    }
    Ok(SchemaValue::Record { fields: values })
}

// ── Shared value parsing dispatcher ─────────────────────────────────────────

pub(super) fn parse_cm_value<D: Dialect>(
    lexer: &mut Lexer,
    graph: &SchemaGraph,
    ty: &SchemaType,
) -> Result<SchemaValue, ParseError> {
    match ty {
        SchemaType::Ref { id, .. } => {
            let def = graph
                .lookup(id)
                .ok_or_else(|| perr(lexer.position(), &format!("dangling type ref '{}'", id.0)))?;
            parse_cm_value_inner::<D>(lexer, graph, &def.body, def.name.as_deref())
        }
        _ => parse_cm_value_inner::<D>(lexer, graph, ty, None),
    }
}

fn parse_cm_value_inner<D: Dialect>(
    lexer: &mut Lexer,
    graph: &SchemaGraph,
    ty: &SchemaType,
    def_name: Option<&str>,
) -> Result<SchemaValue, ParseError> {
    match ty {
        SchemaType::Ref { id, .. } => {
            // Refs nested inside refs: resolve again.
            let def = graph
                .lookup(id)
                .ok_or_else(|| perr(lexer.position(), &format!("dangling type ref '{}'", id.0)))?;
            parse_cm_value_inner::<D>(lexer, graph, &def.body, def.name.as_deref())
        }
        SchemaType::Bool { .. } => {
            let (tok, pos, _) = lexer.next_token()?;
            match tok {
                Token::BoolLit(b) => Ok(SchemaValue::Bool(b)),
                _ => Err(perr(pos, "expected boolean")),
            }
        }
        SchemaType::U8 { .. } => parse_narrow_uint::<u8>(lexer, "u8").map(SchemaValue::U8),
        SchemaType::U16 { .. } => parse_narrow_uint::<u16>(lexer, "u16").map(SchemaValue::U16),
        SchemaType::U32 { .. } => parse_narrow_uint::<u32>(lexer, "u32").map(SchemaValue::U32),
        SchemaType::U64 { .. } => parse_uint(lexer).map(SchemaValue::U64),
        SchemaType::S8 { .. } => parse_narrow_int::<i8>(lexer, "s8").map(SchemaValue::S8),
        SchemaType::S16 { .. } => parse_narrow_int::<i16>(lexer, "s16").map(SchemaValue::S16),
        SchemaType::S32 { .. } => parse_narrow_int::<i32>(lexer, "s32").map(SchemaValue::S32),
        SchemaType::S64 { .. } => parse_int(lexer).map(SchemaValue::S64),
        SchemaType::F32 { .. } => parse_f32(lexer).map(SchemaValue::F32),
        SchemaType::F64 { .. } => parse_float(lexer).map(SchemaValue::F64),
        SchemaType::Char { .. } => {
            let ch = D::parse_char(lexer)?;
            Ok(SchemaValue::Char(ch))
        }
        SchemaType::String { .. } => {
            let (s, _, _) = lexer.expect_string()?;
            Ok(SchemaValue::String(s))
        }
        SchemaType::List { element, .. } => D::parse_list(lexer, graph, element),
        SchemaType::Tuple { elements, .. } => D::parse_tuple(lexer, graph, elements),
        SchemaType::Record { fields, .. } => D::parse_record(lexer, graph, def_name, fields),
        SchemaType::Variant { cases, .. } => D::parse_variant(lexer, graph, def_name, cases),
        SchemaType::Enum { cases, .. } => D::parse_enum(lexer, def_name, cases),
        SchemaType::Option { inner, .. } => D::parse_option(lexer, graph, inner),
        SchemaType::Result { spec, .. } => D::parse_result(lexer, graph, spec),
        SchemaType::Flags { flags, .. } => D::parse_flags(lexer, def_name, flags),
        // Rich semantic types use a uniform constructor-call form
        // `Name("payload")` (optionally `Name("payload", "language")` for
        // text) so they round-trip through the language-specific parsers
        // without needing to teach the lexer about URL/base64/RFC 3339
        // bodies. The body string is parsed by the same per-type canonical
        // decoder regardless of dialect.
        SchemaType::Text { .. } => {
            let (text, language) = parse_text_constructor(lexer)?;
            let mut payload = canon_text::from_text(&text)
                .map_err(|e| perr(lexer.position(), &format!("invalid text value: {e}")))?;
            payload.language = language;
            Ok(SchemaValue::Text(payload))
        }
        SchemaType::Binary { .. } => {
            let s = parse_rich_constructor(lexer, "Binary")?;
            let payload = canon_binary::from_text(&s)
                .map_err(|e| perr(lexer.position(), &format!("invalid binary value: {e}")))?;
            Ok(SchemaValue::Binary(payload))
        }
        SchemaType::Path { .. } => D::parse_path(lexer),
        SchemaType::Url { .. } => D::parse_url(lexer),
        SchemaType::Datetime { .. } => D::parse_datetime(lexer),
        SchemaType::Duration { .. } => D::parse_duration(lexer),
        SchemaType::Quantity { .. } => D::parse_quantity(lexer),
        SchemaType::Secret { .. } => {
            let s = parse_rich_constructor(lexer, "Secret")?;
            let payload = canon_secret::from_text(&s)
                .map_err(|e| perr(lexer.position(), &format!("invalid secret value: {e}")))?;
            Ok(SchemaValue::Secret(payload))
        }
        SchemaType::QuotaToken { .. } => {
            let s = parse_rich_constructor(lexer, "QuotaToken")?;
            let payload = canon_quota_token::from_text(&s)
                .map_err(|e| perr(lexer.position(), &format!("invalid quota-token value: {e}")))?;
            Ok(SchemaValue::QuotaToken(payload))
        }
        SchemaType::PermissionCard { .. } => {
            let s = parse_rich_constructor(lexer, "PermissionCard")?;
            let payload = canon_permission_card::from_text(&s).map_err(|e| {
                perr(
                    lexer.position(),
                    &format!("invalid permission-card value: {e}"),
                )
            })?;
            Ok(SchemaValue::PermissionCard(payload))
        }
        SchemaType::Union { spec, .. } => parse_union::<D>(lexer, graph, spec),
        SchemaType::FixedList {
            element, length, ..
        } => {
            let parsed = D::parse_list(lexer, graph, element)?;
            let SchemaValue::List { elements } = parsed else {
                return Err(perr(
                    lexer.position(),
                    "internal error: parse_list did not return a List",
                ));
            };
            if elements.len() as u32 != *length {
                return Err(perr(
                    lexer.position(),
                    &format!(
                        "fixed-list length mismatch: expected {}, got {}",
                        length,
                        elements.len()
                    ),
                ));
            }
            Ok(SchemaValue::FixedList { elements })
        }
        SchemaType::Map { key, value, .. } => parse_map::<D>(lexer, graph, key, value),
        SchemaType::Future { .. } | SchemaType::Stream { .. } => Err(perr(
            lexer.position(),
            "future/stream values are not parseable from CLI",
        )),
    }
}

fn parse_union<D: Dialect>(
    lexer: &mut Lexer,
    graph: &SchemaGraph,
    spec: &golem_common::schema::schema_type::UnionSpec,
) -> Result<SchemaValue, ParseError> {
    let (tag, pos, _) = lexer.expect_ident()?;
    let branch: &UnionBranch = spec
        .branches
        .iter()
        .find(|b| b.tag == tag)
        .ok_or_else(|| perr(pos, &format!("unknown union branch '{tag}'")))?;
    lexer.expect(&Token::LParen)?;
    let body = parse_cm_value::<D>(lexer, graph, &branch.body)?;
    lexer.expect(&Token::RParen)?;
    Ok(SchemaValue::Union(UnionValuePayload {
        tag: branch.tag.clone(),
        body: Box::new(body),
    }))
}

fn parse_map<D: Dialect>(
    lexer: &mut Lexer,
    graph: &SchemaGraph,
    key_ty: &SchemaType,
    value_ty: &SchemaType,
) -> Result<SchemaValue, ParseError> {
    lexer.expect(&Token::LBrace)?;
    let mut entries: Vec<(SchemaValue, SchemaValue)> = Vec::new();
    while *lexer.peek()? != Token::RBrace {
        if !entries.is_empty() {
            lexer.expect(&Token::Comma)?;
            if *lexer.peek()? == Token::RBrace {
                break;
            }
        }
        let k = parse_cm_value::<D>(lexer, graph, key_ty)?;
        // Map entries are written `k => v` to avoid confusion with record
        // field syntax (`k: v` in many dialects). The lexer has no `>`
        // token, so we consume `=` then the bare `>` byte.
        lexer.expect(&Token::Eq)?;
        if !lexer.skip_raw_char(b'>') {
            return Err(perr(
                lexer.position(),
                "expected '=>' between map key and value",
            ));
        }
        let v = parse_cm_value::<D>(lexer, graph, value_ty)?;
        entries.push((k, v));
    }
    lexer.expect(&Token::RBrace)?;
    Ok(SchemaValue::Map { entries })
}

/// Parse a rich-scalar constructor of the form `Name("payload")`. The
/// identifier must match `expected_name` exactly; the payload must be a
/// JSON-quoted string literal so the lexer can absorb arbitrary canonical
/// text (URLs, base64, RFC 3339 timestamps) without per-type tokenisation.
fn parse_rich_constructor(lexer: &mut Lexer, expected_name: &str) -> Result<String, ParseError> {
    let (name, pos, _) = lexer.expect_ident()?;
    if name != expected_name {
        return Err(perr(
            pos,
            &format!("expected '{expected_name}' constructor, got '{name}'"),
        ));
    }
    lexer.expect(&Token::LParen)?;
    let (body, _, _) = lexer.expect_string()?;
    lexer.expect(&Token::RParen)?;
    Ok(body)
}

/// Parse a text constructor: either `Text("body")` or
/// `Text("body", "language")`.
fn parse_text_constructor(lexer: &mut Lexer) -> Result<(String, Option<String>), ParseError> {
    let (name, pos, _) = lexer.expect_ident()?;
    if name != "Text" {
        return Err(perr(
            pos,
            &format!("expected 'Text' constructor, got '{name}'"),
        ));
    }
    lexer.expect(&Token::LParen)?;
    let (body, _, _) = lexer.expect_string()?;
    let language = if *lexer.peek()? == Token::Comma {
        lexer.expect(&Token::Comma)?;
        let (lang, _, _) = lexer.expect_string()?;
        Some(lang)
    } else {
        None
    };
    lexer.expect(&Token::RParen)?;
    Ok((body, language))
}

// ── Rich-scalar constructor parsers + value builders ────────────────────────
//
// The constructor parsers below implement the `Name("payload")` form accepted
// in every dialect (these back the default `Dialect::parse_*` hooks). The value
// builders are shared by both the constructor parsers and the per-language
// native-literal overrides so all paths funnel through the same canonical
// decoders and produce identically-typed `SchemaValue`s.

pub(super) fn parse_quantity_constructor(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
    let pos = lexer.position();
    let s = parse_rich_constructor(lexer, "Quantity")?;
    quantity_value_from_text(pos, &s)
}

pub(super) fn parse_duration_constructor(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
    let pos = lexer.position();
    let s = parse_rich_constructor(lexer, "Duration")?;
    duration_value_from_text(pos, &s)
}

pub(super) fn parse_datetime_constructor(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
    let pos = lexer.position();
    let s = parse_rich_constructor(lexer, "Datetime")?;
    datetime_value_from_text(pos, &s)
}

pub(super) fn parse_url_constructor(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
    let pos = lexer.position();
    let s = parse_rich_constructor(lexer, "Url")?;
    let url =
        canon_url::from_text(&s).map_err(|e| perr(pos, &format!("invalid url value: {e}")))?;
    Ok(SchemaValue::Url { url })
}

pub(super) fn parse_path_constructor(lexer: &mut Lexer) -> Result<SchemaValue, ParseError> {
    let pos = lexer.position();
    let s = parse_rich_constructor(lexer, "Path")?;
    let path =
        canon_path::from_text(&s).map_err(|e| perr(pos, &format!("invalid path value: {e}")))?;
    Ok(SchemaValue::Path { path })
}

/// Parse the `("body")` tail of a rich constructor whose name identifier has
/// already been consumed. Dialect overrides use this after looking past the
/// name to disambiguate a constructor from a native literal.
pub(super) fn parse_rich_constructor_body(lexer: &mut Lexer) -> Result<String, ParseError> {
    lexer.expect(&Token::LParen)?;
    let (body, _, _) = lexer.expect_string()?;
    lexer.expect(&Token::RParen)?;
    Ok(body)
}

pub(super) fn quantity_value_from_text(pos: usize, s: &str) -> Result<SchemaValue, ParseError> {
    let payload = canon_quantity::from_text(s)
        .map_err(|e| perr(pos, &format!("invalid quantity value: {e}")))?;
    Ok(SchemaValue::Quantity(payload))
}

pub(super) fn duration_value_from_text(pos: usize, s: &str) -> Result<SchemaValue, ParseError> {
    let payload = canon_duration::from_text(s)
        .map_err(|e| perr(pos, &format!("invalid duration value: {e}")))?;
    Ok(SchemaValue::Duration(payload))
}

pub(super) fn duration_value_from_nanos(nanos: i64) -> SchemaValue {
    SchemaValue::Duration(DurationValuePayload { nanoseconds: nanos })
}

pub(super) fn datetime_value_from_text(pos: usize, s: &str) -> Result<SchemaValue, ParseError> {
    let value = canon_datetime::from_text(s)
        .or_else(|_| DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&chrono::Utc)))
        .map_err(|e| perr(pos, &format!("invalid datetime value: {e}")))?;
    Ok(SchemaValue::Datetime { value })
}

pub(super) fn datetime_value_from_millis(
    pos: usize,
    millis: i64,
) -> Result<SchemaValue, ParseError> {
    let value = DateTime::<chrono::Utc>::from_timestamp_millis(millis).ok_or_else(|| {
        perr(
            pos,
            &format!("datetime out of range for epoch millis {millis}"),
        )
    })?;
    Ok(SchemaValue::Datetime { value })
}

pub(super) fn datetime_value_from_secs_nanos(
    pos: usize,
    secs: i64,
    nanos: u32,
) -> Result<SchemaValue, ParseError> {
    let value = DateTime::<chrono::Utc>::from_timestamp(secs, nanos).ok_or_else(|| {
        perr(
            pos,
            &format!("datetime out of range for epoch seconds {secs}"),
        )
    })?;
    Ok(SchemaValue::Datetime { value })
}

/// Build a datetime from a (possibly fractional) epoch-milliseconds value. The
/// SDK epoch constructors take a floating-point argument, so the fractional
/// millisecond part is honoured down to nanosecond resolution; non-finite
/// values are rejected rather than silently coerced to the epoch.
///
/// The split is done in floating-point *seconds* (not total nanoseconds) so the
/// supported range matches `chrono`'s own range rather than being clipped to the
/// ±292-year window where epoch nanoseconds happen to fit in `i64`.
pub(super) fn datetime_value_from_epoch_millis_f64(
    pos: usize,
    millis: f64,
) -> Result<SchemaValue, ParseError> {
    if !millis.is_finite() {
        return Err(perr(pos, "datetime epoch value must be a finite number"));
    }
    let secs_f = (millis / 1_000.0).floor();
    if secs_f < i64::MIN as f64 || secs_f > i64::MAX as f64 {
        return Err(perr(
            pos,
            &format!("datetime out of range for epoch millis {millis}"),
        ));
    }
    let mut secs = secs_f as i64;
    // `secs_f` is floored, so the remainder is non-negative; the carry branches
    // below only correct floating-point rounding drift at the boundaries.
    let mut nanos = ((millis - secs_f * 1_000.0) * 1_000_000.0).floor() as i64;
    if nanos < 0 {
        secs = secs
            .checked_sub(1)
            .ok_or_else(|| perr(pos, "datetime out of range for epoch millis"))?;
        nanos += 1_000_000_000;
    } else if nanos >= 1_000_000_000 {
        secs = secs
            .checked_add(1)
            .ok_or_else(|| perr(pos, "datetime out of range for epoch millis"))?;
        nanos -= 1_000_000_000;
    }
    datetime_value_from_secs_nanos(pos, secs, nanos as u32)
}

// ── Shared numeric helpers ──────────────────────────────────────────────────

/// A numeric literal used by epoch-based datetime constructors. The
/// integer/float distinction is preserved so integer epochs stay bit-exact
/// while fractional epochs (the SDK constructors take floating-point arguments)
/// keep their sub-unit precision instead of being truncated.
pub(super) enum EpochNumber {
    Int(i64),
    Float(f64),
}

pub(super) fn parse_epoch_number(lexer: &mut Lexer) -> Result<EpochNumber, ParseError> {
    let (tok, pos, _) = lexer.next_token()?;
    match tok {
        Token::UintLit(v) => i64::try_from(v)
            .map(EpochNumber::Int)
            .map_err(|_| perr(pos, &format!("number {v} does not fit in i64"))),
        Token::IntLit(v) => Ok(EpochNumber::Int(v)),
        Token::FloatLit(v) => Ok(EpochNumber::Float(v)),
        _ => Err(perr(pos, "expected a number")),
    }
}

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
        Token::UintLit(v) => i64::try_from(v).map_err(|_| {
            perr(
                pos,
                &format!("integer literal {v} does not fit in signed 64-bit"),
            )
        }),
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

/// Parse an unsigned literal and check it fits in the target narrow type
/// (`u8`/`u16`/`u32`). `as` casts wrap silently on overflow, which would
/// accept `256` as a `u8`; this rejects with a position-tagged error.
pub(super) fn parse_narrow_uint<T>(lexer: &mut Lexer, type_name: &str) -> Result<T, ParseError>
where
    T: TryFrom<u64>,
{
    let pos_before = lexer.position();
    let v = parse_uint(lexer)?;
    T::try_from(v).map_err(|_| {
        perr(
            pos_before,
            &format!("integer literal {v} does not fit in {type_name}"),
        )
    })
}

/// Parse a signed literal and check it fits in the target narrow type
/// (`i8`/`i16`/`i32`). `as` casts wrap silently on overflow; this rejects
/// with a position-tagged error.
pub(super) fn parse_narrow_int<T>(lexer: &mut Lexer, type_name: &str) -> Result<T, ParseError>
where
    T: TryFrom<i64>,
{
    let pos_before = lexer.position();
    let v = parse_int(lexer)?;
    T::try_from(v).map_err(|_| {
        perr(
            pos_before,
            &format!("integer literal {v} does not fit in {type_name}"),
        )
    })
}

/// Parse an `f32` literal: accept the wider `f64` parse and check it
/// rounds to a finite `f32` (so `1e1000` is rejected instead of silently
/// becoming `+inf`).
pub(super) fn parse_f32(lexer: &mut Lexer) -> Result<f32, ParseError> {
    let pos_before = lexer.position();
    let v = parse_float(lexer)?;
    let as_f32 = v as f32;
    if v.is_finite() && !as_f32.is_finite() {
        return Err(perr(
            pos_before,
            &format!("float literal {v} does not fit in f32 (would saturate to ±infinity)"),
        ));
    }
    Ok(as_f32)
}

pub(super) fn perr(position: usize, message: &str) -> ParseError {
    ParseError {
        position,
        message: message.to_string(),
    }
}
