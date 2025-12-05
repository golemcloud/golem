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

use super::path_pattern::{AllPathPatterns, PathPattern, QueryInfo};
use nom::branch::alt;
use nom::bytes::complete::take_while1;
use nom::character::complete::{self, char, multispace0};
use nom::combinator::{map, map_res, not, opt, peek};
use nom::multi::{many0, separated_list0};
use nom::sequence::{delimited, preceded, terminated, tuple};
use nom::{IResult, Parser};

pub fn parse_path_pattern(input: &str) -> IResult<&str, AllPathPatterns> {
    let (input, (path, query)) =
        tuple((path_parser, opt(preceded(char('?'), query_parser))))(input)?;

    Ok((
        input,
        AllPathPatterns {
            path_patterns: path,
            query_params: query.unwrap_or_default(),
        },
    ))
}

fn path_parser(input: &str) -> IResult<&str, Vec<PathPattern>> {
    let middle_segment_parser = alt((path_var_parser, literal_parser));

    let final_segment_parser = alt((path_var_parser, literal_parser, catch_all_path_var_parser));

    let (input, (_, mut patterns, final_pattern)) = tuple((
        opt(slash_parser),
        many0(terminated(middle_segment_parser, slash_parser)),
        opt(final_segment_parser),
    ))(input)?;

    if let Some(pattern) = final_pattern {
        patterns.push(pattern);
    };

    let indexed_patterns = patterns
        .into_iter()
        .map(|pattern| match pattern {
            ParsedPattern::Literal(literal) => PathPattern::literal(literal.trim()),
            ParsedPattern::Var(var) => PathPattern::var(var.trim()),
            ParsedPattern::CatchAllVar(var) => PathPattern::catch_all_var(var.trim()),
        })
        .collect();

    Ok((input, indexed_patterns))
}

fn query_parser(input: &str) -> IResult<&str, Vec<QueryInfo>> {
    separated_list0(char('&'), query_param_parser)(input)
}

fn query_param_parser(input: &str) -> IResult<&str, QueryInfo> {
    map(parse_place_holder, |x| QueryInfo {
        key_name: x.trim().to_string(),
    })(input)
}

fn path_var_parser(input: &str) -> IResult<&str, ParsedPattern<'_>> {
    map_res(parse_place_holder, path_var_inner_parser)(input)
}

fn path_var_inner_parser(
    input: &str,
) -> Result<ParsedPattern<'_>, nom::Err<nom::error::Error<&str>>> {
    let (i, _) = peek(not(char('+')))(input)?;
    Ok(ParsedPattern::Var(i))
}

fn catch_all_path_var_parser(input: &str) -> IResult<&str, ParsedPattern<'_>> {
    map_res(parse_place_holder, catch_all_path_var_inner_parser)(input)
}

fn catch_all_path_var_inner_parser(
    input: &str,
) -> Result<ParsedPattern<'_>, nom::Err<nom::error::Error<&str>>> {
    let (i, _) = char('+')(input)?;
    Ok(ParsedPattern::CatchAllVar(i))
}

fn slash_parser(input: &str) -> IResult<&str, ()> {
    delimited(multispace0, char('/'), multispace0)
        .map(|_| ())
        .parse(input)
}

#[derive(Debug)]
enum ParsedPattern<'a> {
    Literal(&'a str),
    Var(&'a str),
    CatchAllVar(&'a str),
}

fn literal_parser(input: &str) -> IResult<&str, ParsedPattern<'_>> {
    map(take_while1(|c| !"/{}?&".contains(c)), |x| {
        ParsedPattern::Literal(x)
    })(input)
}

fn parse_place_holder(input: &str) -> IResult<&str, &str> {
    delimited(
        complete::char('{'),
        take_until_unbalanced('{', '}'),
        complete::char('}'),
    )(input)
}

// https://stackoverflow.com/questions/70630556/parse-allowing-nested-parentheses-in-nom
fn take_until_unbalanced(
    opening_bracket: char,
    closing_bracket: char,
) -> impl Fn(&str) -> IResult<&str, &str> {
    move |i: &str| {
        let mut index = 0;
        let mut bracket_counter = 0;
        while let Some(n) = &i[index..].find(&[opening_bracket, closing_bracket, '\\'][..]) {
            index += n;
            let mut it = i[index..].chars();
            match it.next().unwrap_or_default() {
                '\\' => {
                    // Skip the escape char `\`.
                    index += '\\'.len_utf8();
                    // Skip also the following char.
                    let c = it.next().unwrap_or_default();
                    index += c.len_utf8();
                }
                c if c == opening_bracket => {
                    bracket_counter += 1;
                    index += opening_bracket.len_utf8();
                }
                c if c == closing_bracket => {
                    // Closing bracket.
                    bracket_counter -= 1;
                    index += closing_bracket.len_utf8();
                }
                // Can not happen.
                _ => unreachable!(),
            };
            // We found the unmatched closing bracket.
            if bracket_counter == -1 {
                // We do not consume it.
                index -= closing_bracket.len_utf8();
                return Ok((&i[index..], &i[0..index]));
            };
        }

        if bracket_counter == 0 {
            Ok(("", i))
        } else {
            let error = nom::error::Error::new(i, nom::error::ErrorKind::TakeUntil);
            let error = nom::Err::Error(error);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_path_pattern, parse_place_holder};
    use crate::custom_api::path_pattern::{AllPathPatterns, LiteralInfo, PathPattern, QueryInfo};
    use test_r::test;

    #[test]
    fn test_parse() {
        let result = parse_path_pattern("/api/{id}/test/{name}/test2?{query1}&{query2}");
        assert_eq!(
            AllPathPatterns {
                path_patterns: vec![
                    PathPattern::literal("api"),
                    PathPattern::var("id"),
                    PathPattern::literal("test"),
                    PathPattern::var("name"),
                    PathPattern::literal("test2"),
                ],
                query_params: vec![
                    QueryInfo {
                        key_name: "query1".to_string()
                    },
                    QueryInfo {
                        key_name: "query2".to_string()
                    }
                ]
            },
            result.unwrap().1
        );

        let result = parse_path_pattern("/api/{id}/test/{name}/test2");

        assert_eq!(
            AllPathPatterns {
                path_patterns: vec![
                    PathPattern::Literal(LiteralInfo("api".to_string())),
                    PathPattern::var("id"),
                    PathPattern::Literal(LiteralInfo("test".to_string())),
                    PathPattern::var("name"),
                    PathPattern::Literal(LiteralInfo("test2".to_string())),
                ],
                query_params: vec![]
            },
            result.unwrap().1
        );

        let result = parse_path_pattern("/api/{id}/{+others}");

        assert_eq!(
            AllPathPatterns {
                path_patterns: vec![
                    PathPattern::Literal(LiteralInfo("api".to_string())),
                    PathPattern::var("id"),
                    PathPattern::catch_all_var("others")
                ],
                query_params: vec![]
            },
            result.unwrap().1
        );

        // {+var} is not allowed in the middle of the path
        assert!(AllPathPatterns::parse("/api/{foo}/{+others}/{bar}").is_err());
    }

    #[test]
    fn test_parse_root_only() {
        assert_eq!(
            parse_path_pattern("/").unwrap().1,
            AllPathPatterns {
                path_patterns: vec![],
                query_params: vec![]
            },
        );
    }

    #[test]
    fn test_parse_root_with_query_params() {
        assert_eq!(
            parse_path_pattern("/?{query}").unwrap().1,
            AllPathPatterns {
                path_patterns: vec![],
                query_params: vec![QueryInfo {
                    key_name: "query".to_string()
                },]
            },
        );
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(
            parse_path_pattern("").unwrap().1,
            AllPathPatterns {
                path_patterns: vec![],
                query_params: vec![]
            },
        );
    }

    #[test]
    fn test_parse_empty_with_query_params() {
        assert_eq!(
            parse_path_pattern("?{query}").unwrap().1,
            AllPathPatterns {
                path_patterns: vec![],
                query_params: vec![QueryInfo {
                    key_name: "query".to_string()
                },]
            },
        );
    }

    #[test]
    fn test_parse_no_slash_with_path() {
        let result = parse_path_pattern("api/{id}/foo/{+others}").unwrap().1;
        assert_eq!(
            result,
            AllPathPatterns {
                path_patterns: vec![
                    PathPattern::Literal(LiteralInfo("api".to_string())),
                    PathPattern::var("id"),
                    PathPattern::Literal(LiteralInfo("foo".to_string())),
                    PathPattern::catch_all_var("others"),
                ],
                query_params: vec![]
            },
        );
    }

    #[test]
    fn test_parse_place_holder() {
        assert_eq!(("", "test"), parse_place_holder("{test}").unwrap());
        assert_eq!(
            ("", "test{test}"),
            parse_place_holder("{test{test}}").unwrap(),
        );
        assert!(parse_place_holder("{test").is_err());
        assert!(parse_place_holder("test}").is_err());
        assert!(parse_place_holder("}").is_err());

        assert_eq!(("}", "test"), parse_place_holder("{test}}").unwrap());
    }
}
