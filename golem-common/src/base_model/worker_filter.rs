// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::base_model::agent::AgentMode;
use crate::base_model::component::ComponentRevision;
use crate::base_model::{AgentStatus, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentNameFilter {
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl AgentNameFilter {
    pub fn new(comparator: StringFilterComparator, value: String) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentNameFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "name {} {}", self.comparator, FilterText(&self.value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusFilter {
    pub comparator: FilterComparator,
    pub value: AgentStatus,
}

impl AgentStatusFilter {
    pub fn new(comparator: FilterComparator, value: AgentStatus) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentStatusFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "status {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentRevisionFilter {
    pub comparator: FilterComparator,
    pub value: ComponentRevision,
}

impl AgentRevisionFilter {
    pub fn new(comparator: FilterComparator, value: ComponentRevision) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentRevisionFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "revision {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentCreatedAtFilter {
    pub comparator: FilterComparator,
    pub value: Timestamp,
}

impl AgentCreatedAtFilter {
    pub fn new(comparator: FilterComparator, value: Timestamp) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentCreatedAtFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "created_at {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentEnvFilter {
    pub name: String,
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl AgentEnvFilter {
    pub fn new(name: String, comparator: StringFilterComparator, value: String) -> Self {
        Self {
            name,
            comparator,
            value,
        }
    }
}

impl Display for AgentEnvFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "env.{} {} {}",
            FilterText(&self.name),
            self.comparator,
            FilterText(&self.value)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigVarsFilter {
    pub name: String,
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl AgentConfigVarsFilter {
    pub fn new(name: String, comparator: StringFilterComparator, value: String) -> Self {
        Self {
            name,
            comparator,
            value,
        }
    }
}

impl Display for AgentConfigVarsFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "config.{} {} {}",
            FilterText(&self.name),
            self.comparator,
            FilterText(&self.value)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentAndFilter {
    pub filters: Vec<AgentFilter>,
}

impl AgentAndFilter {
    pub fn new(filters: Vec<AgentFilter>) -> Self {
        Self { filters }
    }
}

impl Display for AgentAndFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        fmt_junction(f, &self.filters, "AND", "true")
    }
}

/// Renders an `And` or `Or` filter in parentheses, so that it parses back as one node whatever
/// surrounds it. An empty junction renders as its identity, `true` or `false`.
fn fmt_junction(
    f: &mut Formatter<'_>,
    filters: &[AgentFilter],
    keyword: &str,
    empty: &str,
) -> std::fmt::Result {
    match filters {
        [] => return write!(f, "{empty}"),
        [filter] => return write!(f, "{filter}"),
        _ => {}
    }
    write!(f, "(")?;
    for (i, filter) in filters.iter().enumerate() {
        if i > 0 {
            write!(f, " {keyword} ")?;
        }
        write!(f, "{filter}")?;
    }
    write!(f, ")")
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentOrFilter {
    pub filters: Vec<AgentFilter>,
}

impl AgentOrFilter {
    pub fn new(filters: Vec<AgentFilter>) -> Self {
        Self { filters }
    }
}

impl Display for AgentOrFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        fmt_junction(f, &self.filters, "OR", "false")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentModeFilter {
    pub comparator: FilterComparator,
    pub value: AgentMode,
}

impl AgentModeFilter {
    pub fn new(comparator: FilterComparator, value: AgentMode) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentModeFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "mode {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentNotFilter {
    pub filter: Box<AgentFilter>,
}

impl AgentNotFilter {
    pub fn new(filter: AgentFilter) -> Self {
        Self {
            filter: Box::new(filter),
        }
    }
}

impl Display for AgentNotFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NOT {}", self.filter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum AgentFilter {
    Name(AgentNameFilter),
    Status(AgentStatusFilter),
    Revision(AgentRevisionFilter),
    CreatedAt(AgentCreatedAtFilter),
    Env(AgentEnvFilter),
    And(AgentAndFilter),
    Or(AgentOrFilter),
    Not(AgentNotFilter),
    Config(AgentConfigVarsFilter),
    Mode(AgentModeFilter),
}

impl AgentFilter {
    pub fn and(&self, filter: AgentFilter) -> Self {
        match self.clone() {
            AgentFilter::And(AgentAndFilter { filters }) => {
                Self::new_and([filters, vec![filter]].concat())
            }
            f => Self::new_and(vec![f, filter]),
        }
    }

    pub fn or(&self, filter: AgentFilter) -> Self {
        match self.clone() {
            AgentFilter::Or(AgentOrFilter { filters }) => {
                Self::new_or([filters, vec![filter]].concat())
            }
            f => Self::new_or(vec![f, filter]),
        }
    }

    pub fn not(&self) -> Self {
        Self::new_not(self.clone())
    }

    pub fn new_and(filters: Vec<AgentFilter>) -> Self {
        AgentFilter::And(AgentAndFilter::new(filters))
    }

    pub fn new_or(filters: Vec<AgentFilter>) -> Self {
        AgentFilter::Or(AgentOrFilter::new(filters))
    }

    pub fn new_not(filter: AgentFilter) -> Self {
        AgentFilter::Not(AgentNotFilter::new(filter))
    }

    pub fn new_name(comparator: StringFilterComparator, value: String) -> Self {
        AgentFilter::Name(AgentNameFilter::new(comparator, value))
    }

    pub fn new_env(name: String, comparator: StringFilterComparator, value: String) -> Self {
        AgentFilter::Env(AgentEnvFilter::new(name, comparator, value))
    }

    pub fn new_config(name: String, comparator: StringFilterComparator, value: String) -> Self {
        AgentFilter::Config(AgentConfigVarsFilter::new(name, comparator, value))
    }

    pub fn new_revision(comparator: FilterComparator, value: ComponentRevision) -> Self {
        AgentFilter::Revision(AgentRevisionFilter::new(comparator, value))
    }

    pub fn new_status(comparator: FilterComparator, value: AgentStatus) -> Self {
        AgentFilter::Status(AgentStatusFilter::new(comparator, value))
    }

    pub fn new_created_at(comparator: FilterComparator, value: Timestamp) -> Self {
        AgentFilter::CreatedAt(AgentCreatedAtFilter::new(comparator, value))
    }

    pub fn new_mode(comparator: FilterComparator, value: AgentMode) -> Self {
        AgentFilter::Mode(AgentModeFilter::new(comparator, value))
    }

    pub fn from(filters: Vec<String>) -> Result<AgentFilter, String> {
        let mut fs = Vec::new();
        for f in filters {
            fs.push(AgentFilter::from_str(&f)?);
        }
        AgentFilter::new_and(fs).validated()
    }

    /// The canonical form of this filter, refused when it is deeper than
    /// [`MAX_AGENT_FILTER_DEPTH`]. Every entry point that receives a filter tree applies it.
    pub fn validated(self) -> Result<AgentFilter, String> {
        let filter = self.canonical();
        filter.validate_depth()?;
        Ok(filter)
    }

    /// Whether a `Mode` filter appears anywhere in this filter.
    pub fn mentions_mode(&self) -> bool {
        match self {
            AgentFilter::Mode(_) => true,
            AgentFilter::And(AgentAndFilter { filters })
            | AgentFilter::Or(AgentOrFilter { filters }) => {
                filters.iter().any(AgentFilter::mentions_mode)
            }
            AgentFilter::Not(AgentNotFilter { filter }) => filter.mentions_mode(),
            _ => false,
        }
    }

    /// The canonical form of this filter: every `And` or `Or` with one child is replaced by
    /// that child, at every level. Both forms match the same agents. The filter syntax cannot
    /// tell them apart, because `(x)` is only grouping, so every entry point canonicalizes and
    /// a filter means the same tree whether it arrives as text or as JSON.
    pub fn canonical(self) -> AgentFilter {
        match self {
            AgentFilter::And(AgentAndFilter { filters }) => {
                let mut filters: Vec<_> = filters.into_iter().map(Self::canonical).collect();
                if filters.len() == 1 {
                    filters.remove(0)
                } else {
                    AgentFilter::new_and(filters)
                }
            }
            AgentFilter::Or(AgentOrFilter { filters }) => {
                let mut filters: Vec<_> = filters.into_iter().map(Self::canonical).collect();
                if filters.len() == 1 {
                    filters.remove(0)
                } else {
                    AgentFilter::new_or(filters)
                }
            }
            AgentFilter::Not(AgentNotFilter { filter }) => filter.canonical().not(),
            leaf => leaf,
        }
    }

    /// The number of levels from this filter to its deepest leaf, counting both.
    pub fn depth(&self) -> usize {
        match self {
            AgentFilter::And(AgentAndFilter { filters })
            | AgentFilter::Or(AgentOrFilter { filters }) => {
                1 + filters.iter().map(AgentFilter::depth).max().unwrap_or(0)
            }
            AgentFilter::Not(AgentNotFilter { filter }) => 1 + filter.depth(),
            _ => 1,
        }
    }

    /// Refuses a filter deeper than [`MAX_AGENT_FILTER_DEPTH`].
    pub fn validate_depth(&self) -> Result<(), String> {
        if self.depth() > MAX_AGENT_FILTER_DEPTH {
            Err(filter_depth_error())
        } else {
            Ok(())
        }
    }
}

impl Display for AgentFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentFilter::Name(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Revision(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Status(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::CreatedAt(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Env(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Config(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Not(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::And(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Or(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Mode(filter) => {
                write!(f, "{filter}")
            }
        }
    }
}

/// Parses the filter syntax that `Display` renders. The grammar, with case-insensitive keywords:
///
/// ```text
/// filter := and ("or" and)*
/// and    := unary ("and" unary)*
/// unary  := "not" unary | "(" filter ")" | "true" | "false" | property comparator value
/// ```
///
/// A value, and the key of an `env.` or `config.` property, is a bare word or a double-quoted
/// string with `\"` and `\\` escapes. A bare word ends at whitespace or at an unmatched `)`; it
/// may contain balanced parentheses and quoted parts, with whitespace inside them, so agent IDs
/// such as `Counter("a b", 2)` need no quotes. `true` is the empty `And`, `false` the empty `Or`.
impl FromStr for AgentFilter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parser = FilterParser {
            tokens: tokenize(s).map_err(|e| format!("Invalid filter `{s}`: {e}"))?,
            pos: 0,
            depth: 0,
        };
        let filter = parser
            .parse_or()
            .and_then(|filter| match parser.tokens.get(parser.pos) {
                None => Ok(filter),
                Some(token) => Err(format!("unexpected {token}")),
            })
            .and_then(|filter| filter.validate_depth().map(|()| filter))
            .map_err(|e| format!("Invalid filter `{s}`: {e}"))?;
        Ok(filter)
    }
}

/// The deepest filter tree that Golem accepts, counting the root and the leaf as levels.
///
/// Filters have a bounded depth; they are not processed stack-safely. Every walk over a filter
/// recurses once per level: the parser, `Display`, `matches`, the protobuf and serde
/// conversions, and the derived `Clone`, `PartialEq` and `Drop`. Making all of them iterative
/// would buy nothing, because the transport bounds the depth anyway. The bound is the depth
/// that still reaches the executor: the worker service sends a filter inside
/// `GetWorkersMetadataRequest`, prost refuses a message nested deeper than 100, and each filter
/// level takes two protobuf messages, with three for a `created_at` leaf. 49 levels is the
/// deepest filter that decodes; `agent_filter_depth_limit_matches_grpc` checks that.
///
/// The filter parser, the worker service's REST list endpoints and the executor's gRPC
/// metadata endpoints refuse a deeper filter with the same error, so no entry point accepts a
/// filter that another entry point refuses. They check the canonical form
/// ([`AgentFilter::validated`]), which is the tree that reaches the executor.
///
/// The parser also bounds the nesting of the text: each `not` and each pair of parentheses
/// takes a level, even redundant parentheses that the canonical form drops. This keeps the
/// parser's own recursion bounded. `Display` never writes redundant parentheses, so the text of
/// every accepted filter parses back.
pub const MAX_AGENT_FILTER_DEPTH: usize = 49;

fn filter_depth_error() -> String {
    format!("the filter is nested deeper than {MAX_AGENT_FILTER_DEPTH} levels")
}

/// Renders a value or a property key so that `tokenize` reads it back as the same text: bare
/// when it lexes as one bare word, quoted otherwise.
struct FilterText<'a>(&'a str);

impl Display for FilterText<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let text = self.0;
        let starts_bare = text
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace() && !matches!(c, '(' | ')' | '"'));
        if starts_bare && bare_word_len(text) == Ok(text.len()) {
            write!(f, "{text}")
        } else {
            write!(f, "\"")?;
            for c in text.chars() {
                if matches!(c, '"' | '\\') {
                    write!(f, "\\")?;
                }
                write!(f, "{c}")?;
            }
            write!(f, "\"")
        }
    }
}

#[derive(Debug)]
enum FilterToken {
    Open,
    Close,
    Quoted(String),
    Word(String),
}

impl Display for FilterToken {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterToken::Open => write!(f, "`(`"),
            FilterToken::Close => write!(f, "`)`"),
            FilterToken::Quoted(text) => write!(f, "quoted text `{text}`"),
            FilterToken::Word(word) => write!(f, "`{word}`"),
        }
    }
}

fn tokenize(s: &str) -> Result<Vec<FilterToken>, String> {
    let mut tokens = Vec::new();
    let mut rest = s.trim_start();
    while let Some(c) = rest.chars().next() {
        let len = match c {
            '(' => {
                tokens.push(FilterToken::Open);
                1
            }
            ')' => {
                tokens.push(FilterToken::Close);
                1
            }
            '"' => {
                let (text, len) = lex_quoted(rest)?;
                tokens.push(FilterToken::Quoted(text));
                len
            }
            _ => {
                let len = bare_word_len(rest)?;
                tokens.push(FilterToken::Word(rest[..len].to_string()));
                len
            }
        };
        rest = rest[len..].trim_start();
    }
    Ok(tokens)
}

/// Reads the quoted string at the start of `s`, which starts with `"`. Returns the unescaped
/// text and the length of the quoted string in bytes.
fn lex_quoted(s: &str) -> Result<(String, usize), String> {
    let mut text = String::new();
    let mut chars = s.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok((text, i + 1)),
            '\\' => match chars.next() {
                Some((_, escaped)) => text.push(escaped),
                None => break,
            },
            _ => text.push(c),
        }
    }
    Err(format!("unclosed quote in `{s}`"))
}

/// The length in bytes of the bare word at the start of `s`.
fn bare_word_len(s: &str) -> Result<usize, String> {
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(c) = s[i..].chars().next() {
        match c {
            c if c.is_whitespace() && depth == 0 => return Ok(i),
            ')' if depth == 0 => return Ok(i),
            ')' => depth -= 1,
            '(' => depth += 1,
            '"' => {
                i += lex_quoted(&s[i..])?.1;
                continue;
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    if depth > 0 {
        Err(format!("unclosed parenthesis in `{s}`"))
    } else {
        Ok(s.len())
    }
}

struct FilterParser {
    tokens: Vec<FilterToken>,
    pos: usize,
    depth: usize,
}

impl FilterParser {
    fn next(&mut self) -> Option<&FilterToken> {
        let token = self.tokens.get(self.pos);
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn eat_keyword(&mut self, keyword: &str) -> bool {
        match self.tokens.get(self.pos) {
            Some(FilterToken::Word(word)) if word.eq_ignore_ascii_case(keyword) => {
                self.pos += 1;
                true
            }
            _ => false,
        }
    }

    fn parse_or(&mut self) -> Result<AgentFilter, String> {
        let mut filters = vec![self.parse_and()?];
        while self.eat_keyword("or") {
            filters.push(self.parse_and()?);
        }
        Ok(if filters.len() == 1 {
            filters.remove(0)
        } else {
            AgentFilter::new_or(filters)
        })
    }

    fn parse_and(&mut self) -> Result<AgentFilter, String> {
        let mut filters = vec![self.parse_unary()?];
        while self.eat_keyword("and") {
            filters.push(self.parse_unary()?);
        }
        Ok(if filters.len() == 1 {
            filters.remove(0)
        } else {
            AgentFilter::new_and(filters)
        })
    }

    fn parse_unary(&mut self) -> Result<AgentFilter, String> {
        if self.depth == MAX_AGENT_FILTER_DEPTH {
            return Err(filter_depth_error());
        }
        self.depth += 1;
        let filter = self.parse_unary_nested();
        self.depth -= 1;
        filter
    }

    fn parse_unary_nested(&mut self) -> Result<AgentFilter, String> {
        if self.eat_keyword("not") {
            return Ok(self.parse_unary()?.not());
        }
        match self.next() {
            Some(FilterToken::Open) => {
                let filter = self.parse_or()?;
                match self.next() {
                    Some(FilterToken::Close) => Ok(filter),
                    Some(token) => Err(format!("expected `)`, found {token}")),
                    None => Err("expected `)`, found the end".to_string()),
                }
            }
            Some(FilterToken::Word(word)) if word.eq_ignore_ascii_case("true") => {
                Ok(AgentFilter::new_and(vec![]))
            }
            Some(FilterToken::Word(word)) if word.eq_ignore_ascii_case("false") => {
                Ok(AgentFilter::new_or(vec![]))
            }
            Some(FilterToken::Word(property)) => {
                let property = property.clone();
                let comparator = match self.next() {
                    Some(FilterToken::Word(comparator)) => comparator.clone(),
                    Some(token) => {
                        return Err(format!(
                            "expected a comparator after `{property}`, found {token}"
                        ));
                    }
                    None => return Err(format!("expected a comparator after `{property}`")),
                };
                let value = match self.next() {
                    Some(FilterToken::Word(value) | FilterToken::Quoted(value)) => value.clone(),
                    Some(token) => {
                        return Err(format!(
                            "expected a value after `{property} {comparator}`, found {token}"
                        ));
                    }
                    None => {
                        return Err(format!("expected a value after `{property} {comparator}`"));
                    }
                };
                parse_property_filter(&property, &comparator, value)
            }
            Some(token) => Err(format!("expected a filter, found {token}")),
            None => Err("expected a filter, found the end".to_string()),
        }
    }
}

fn parse_property_filter(
    property: &str,
    comparator: &str,
    value: String,
) -> Result<AgentFilter, String> {
    match property {
        "name" => Ok(AgentFilter::new_name(comparator.parse()?, value)),
        "revision" => Ok(AgentFilter::new_revision(
            comparator.parse()?,
            value
                .parse()
                .map_err(|e| format!("Invalid filter value: {e}"))?,
        )),
        "status" => Ok(AgentFilter::new_status(comparator.parse()?, value.parse()?)),
        "mode" => Ok(AgentFilter::new_mode(comparator.parse()?, value.parse()?)),
        "created_at" | "createdAt" => Ok(AgentFilter::new_created_at(
            comparator.parse()?,
            value.parse()?,
        )),
        _ => {
            if let Some(name) = property.strip_prefix("env.") {
                Ok(AgentFilter::new_env(
                    parse_property_key(name)?,
                    comparator.parse()?,
                    value,
                ))
            } else if let Some(name) = property.strip_prefix("config.") {
                Ok(AgentFilter::new_config(
                    parse_property_key(name)?,
                    comparator.parse()?,
                    value,
                ))
            } else {
                Err(format!("unknown property `{property}`"))
            }
        }
    }
}

/// Reads the key after `env.` or `config.`: a bare word, or one quoted string.
fn parse_property_key(key: &str) -> Result<String, String> {
    if key.starts_with('"') {
        match lex_quoted(key)? {
            (text, len) if len == key.len() => Ok(text),
            _ => Err(format!("unexpected text after the quoted key `{key}`")),
        }
    } else if key.is_empty() {
        Err("missing key after `env.` or `config.`".to_string())
    } else {
        Ok(key.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum StringFilterComparator {
    Equal,
    NotEqual,
    Like,
    NotLike,
    StartsWith,
}

impl StringFilterComparator {
    pub fn matches<T: Display>(&self, value1: &T, value2: &T) -> bool {
        match self {
            StringFilterComparator::Equal => value1.to_string() == value2.to_string(),
            StringFilterComparator::NotEqual => value1.to_string() != value2.to_string(),
            StringFilterComparator::Like => {
                value1.to_string().contains(value2.to_string().as_str())
            }
            StringFilterComparator::NotLike => {
                !value1.to_string().contains(value2.to_string().as_str())
            }
            StringFilterComparator::StartsWith => {
                value1.to_string().starts_with(value2.to_string().as_str())
            }
        }
    }
}

impl FromStr for StringFilterComparator {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "==" | "=" | "equal" | "eq" => Ok(StringFilterComparator::Equal),
            "!=" | "notequal" | "ne" => Ok(StringFilterComparator::NotEqual),
            "like" => Ok(StringFilterComparator::Like),
            "notlike" => Ok(StringFilterComparator::NotLike),
            "startswith" => Ok(StringFilterComparator::StartsWith),
            _ => Err(format!("Unknown String Filter Comparator: {s}")),
        }
    }
}

impl TryFrom<i32> for StringFilterComparator {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(StringFilterComparator::Equal),
            1 => Ok(StringFilterComparator::NotEqual),
            2 => Ok(StringFilterComparator::Like),
            3 => Ok(StringFilterComparator::NotLike),
            4 => Ok(StringFilterComparator::StartsWith),
            _ => Err(format!("Unknown String Filter Comparator: {value}")),
        }
    }
}

impl From<StringFilterComparator> for i32 {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => 0,
            StringFilterComparator::NotEqual => 1,
            StringFilterComparator::Like => 2,
            StringFilterComparator::NotLike => 3,
            StringFilterComparator::StartsWith => 4,
        }
    }
}

impl Display for StringFilterComparator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StringFilterComparator::Equal => "==",
            StringFilterComparator::NotEqual => "!=",
            StringFilterComparator::Like => "like",
            StringFilterComparator::NotLike => "notlike",
            StringFilterComparator::StartsWith => "startswith",
        };
        write!(f, "{s}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum FilterComparator {
    Equal,
    NotEqual,
    GreaterEqual,
    Greater,
    LessEqual,
    Less,
}

impl Display for FilterComparator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            FilterComparator::Equal => "==",
            FilterComparator::NotEqual => "!=",
            FilterComparator::GreaterEqual => ">=",
            FilterComparator::Greater => ">",
            FilterComparator::LessEqual => "<=",
            FilterComparator::Less => "<",
        };
        write!(f, "{s}")
    }
}

impl FilterComparator {
    pub fn matches<T: Ord>(&self, value1: &T, value2: &T) -> bool {
        match self {
            FilterComparator::Equal => value1 == value2,
            FilterComparator::NotEqual => value1 != value2,
            FilterComparator::Less => value1 < value2,
            FilterComparator::LessEqual => value1 <= value2,
            FilterComparator::Greater => value1 > value2,
            FilterComparator::GreaterEqual => value1 >= value2,
        }
    }
}

impl FromStr for FilterComparator {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "==" | "=" | "equal" | "eq" => Ok(FilterComparator::Equal),
            "!=" | "notequal" | "ne" => Ok(FilterComparator::NotEqual),
            ">=" | "greaterequal" | "ge" => Ok(FilterComparator::GreaterEqual),
            ">" | "greater" | "gt" => Ok(FilterComparator::Greater),
            "<=" | "lessequal" | "le" => Ok(FilterComparator::LessEqual),
            "<" | "less" | "lt" => Ok(FilterComparator::Less),
            _ => Err(format!("Unknown Filter Comparator: {s}")),
        }
    }
}

impl TryFrom<i32> for FilterComparator {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(FilterComparator::Equal),
            1 => Ok(FilterComparator::NotEqual),
            2 => Ok(FilterComparator::Less),
            3 => Ok(FilterComparator::LessEqual),
            4 => Ok(FilterComparator::Greater),
            5 => Ok(FilterComparator::GreaterEqual),
            _ => Err(format!("Unknown Filter Comparator: {value}")),
        }
    }
}

impl From<FilterComparator> for i32 {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => 0,
            FilterComparator::NotEqual => 1,
            FilterComparator::Less => 2,
            FilterComparator::LessEqual => 3,
            FilterComparator::Greater => 4,
            FilterComparator::GreaterEqual => 5,
        }
    }
}
