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

use combine::parser::char::{char, space, spaces, string};
use combine::{
    EasyParser, ParseError, Parser, Stream, any, attempt, between, choice, eof, many, none_of,
    one_of, optional, parser, sep_by, skip_many1, token,
};
use regex::Regex;
use std::error::Error;
use std::fmt;
use std::fmt::{Display, Formatter};

/// A simplified version of the Lucene query language
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Or { queries: Vec<Query> },
    And { queries: Vec<Query> },
    Not { query: Box<Query> },
    Regex { pattern: String },
    Term { value: String },
    Phrase { value: String },
    Field { field: String, query: Box<Query> },
}

impl Query {
    pub fn parse(s: &str) -> Result<Query, String> {
        query()
            .skip(eof())
            .easy_parse(s)
            .map_err(|err| err.to_string())
            .map(|(query, _)| query)
    }
}

#[derive(Debug, Clone)]
pub enum LeafQuery {
    Term { value: String },
    Phrase { value: String },
    Regex { pattern: Regex },
}

impl LeafQuery {
    pub fn matches(&self, s: &str) -> bool {
        match self {
            LeafQuery::Term { value } => s.to_lowercase().contains(&value.to_lowercase()),
            LeafQuery::Phrase { value } => s.contains(value),
            LeafQuery::Regex { pattern } => pattern.is_match(s),
        }
    }
}

impl TryFrom<Query> for LeafQuery {
    type Error = String;

    fn try_from(value: Query) -> Result<Self, Self::Error> {
        match value {
            Query::Regex { pattern } => {
                let pattern = Regex::new(&pattern)
                    .map_err(|err| format!("Invalid regular expression: {err}"))?;
                Ok(Self::Regex { pattern })
            }
            Query::Term { value } => Ok(Self::Term { value }),
            Query::Phrase { value } => Ok(Self::Phrase { value }),
            _ => Err("Not a leaf query".to_string()),
        }
    }
}

#[derive(Debug, Clone)]
struct LuceneParseError {
    pub message: String,
}

impl Display for LuceneParseError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for LuceneParseError {}

const KEYWORDS: &[&str; 3] = &["AND", "OR", "NOT"];

fn escaped_char<Input>() -> impl Parser<Input, Output = String>
where
    Input: Stream<Token = char>,
{
    char('\\').with(any()).map(|c: char| c.to_string())
}

fn term_start_char<Input>() -> impl Parser<Input, Output = String>
where
    Input: Stream<Token = char>,
{
    none_of(" \t\n\r+-!():^<>=[]\"{}~\\/\u{3000}".chars())
        .map(|c: char| c.to_string())
        .or(escaped_char())
}

fn term<Input>() -> impl Parser<Input, Output = String>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    let term_char = term_start_char()
        .or(escaped_char())
        .or(one_of("+-".chars()).map(|c: char| c.to_string()));

    (
        term_start_char(),
        many(term_char).map(|chars: Vec<String>| chars.concat()),
    )
        .map(|(start, rest)| start + &rest)
        .and_then(|s: String| {
            if KEYWORDS.contains(&s.as_str()) {
                Err(LuceneParseError {
                    message: format!("Keyword {s} used in term position"),
                })
            } else {
                Ok(s)
            }
        })
        .map(unescape)
}

fn unescape(s: String) -> String {
    s.replace("\\", "")
}

fn unescape_wildcards(s: String) -> String {
    s.replace("\\?", "?").replace("\\*", "*")
}

fn field_name_prefix<Input>() -> impl Parser<Input, Output = String>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    let op_colon = token(':');
    let op_equal = token('=');
    let field_name = term().map(unescape_wildcards);

    field_name.skip(spaces().skip(op_colon.or(op_equal).skip(spaces())))
}

fn regexp_term_query<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
{
    between(
        token('/'),
        token('/'),
        many(string("\\/").map(|_| '/').or(none_of("/".chars()))),
    )
    .map(|pattern| Query::Regex { pattern })
}

fn quoted_term_query<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
{
    between(
        token('"'),
        token('"'),
        many(string("\\\"").map(|_| '"').or(none_of("\"".chars()))),
    )
    .map(|value| Query::Phrase { value })
}

fn field_term_query<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    term().map(|value| Query::Term { value })
}

fn term_query<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    choice!(regexp_term_query(), quoted_term_query(), field_term_query())
}

fn grouping<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    between(token('('), token(')'), query())
}

fn clause<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    (
        optional(attempt(field_name_prefix())),
        grouping().or(term_query()),
    )
        .map(|(field, inner)| {
            if let Some(field) = field {
                Query::Field {
                    field,
                    query: Box::new(inner),
                }
            } else {
                inner
            }
        })
}

fn mod_clause<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    let not = between(spaces(), skip_many1(space()), string("NOT"));

    (optional(attempt(not)), clause()).map(|(not, clause)| {
        if not.is_some() {
            Query::Not {
                query: Box::new(clause),
            }
        } else {
            clause
        }
    })
}

fn conjunct_query<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    let and = between(spaces(), skip_many1(space()), string("AND"));
    sep_by(mod_clause(), attempt(and)).map(|clauses: Vec<Query>| {
        if clauses.len() == 1 {
            clauses.into_iter().next().unwrap()
        } else {
            Query::And { queries: clauses }
        }
    })
}

fn disjunct_query<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    let or = between(spaces(), skip_many1(space()), string("OR"));
    sep_by(conjunct_query(), attempt(or)).map(|clauses: Vec<Query>| {
        if clauses.len() == 1 {
            clauses.into_iter().next().unwrap()
        } else {
            Query::Or { queries: clauses }
        }
    })
}

fn query_<Input>() -> impl Parser<Input, Output = Query>
where
    Input: Stream<Token = char>,
    LuceneParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    sep_by(disjunct_query(), skip_many1(space())).map(|queries: Vec<Query>| {
        if queries.len() == 1 {
            queries.into_iter().next().unwrap()
        } else {
            Query::And { queries }
        }
    })
}

parser! {
    fn query[Input]()(Input) -> Query
    where [Input: Stream<Token = char>, LuceneParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]
    {
        query_()
    }
}

#[cfg(test)]
mod tests;
