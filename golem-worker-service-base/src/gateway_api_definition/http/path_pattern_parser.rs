// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use nom::branch::alt;
use nom::bytes::complete::take_while1;
use nom::character::complete::{char, multispace0};
use nom::combinator::{map, map_res, not, opt, peek};

use nom::multi::{many0, separated_list0};
use nom::sequence::{delimited, preceded, tuple};
use nom::IResult;

use crate::gateway_api_definition::http::{
    place_holder_parser, AllPathPatterns, PathPattern, QueryInfo,
};

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
    let item_parser = delimited(
        multispace0,
        alt((path_var_parser, literal_parser)),
        multispace0,
    );
    let final_item_parser = delimited(multispace0, catch_all_path_var_parser, multispace0);
    let (input, (mut patterns, final_pattern)) = tuple((
        many0(preceded(char('/'), item_parser)),
        opt(preceded(char('/'), final_item_parser)),
    ))(input)?;

    if let Some(final_pattern) = final_pattern {
        patterns.push(final_pattern);
    };

    let indexed_patterns = patterns
        .into_iter()
        .map(|pattern| match pattern {
            ParsedPattern::Literal(literal) => PathPattern::literal(literal),
            ParsedPattern::Var(var) => PathPattern::var(var),
            ParsedPattern::CatchAllVar(var) => PathPattern::catch_all_var(var),
        })
        .collect();

    Ok((input, indexed_patterns))
}

fn query_parser(input: &str) -> IResult<&str, Vec<QueryInfo>> {
    separated_list0(char('&'), query_param_parser)(input)
}

fn query_param_parser(input: &str) -> IResult<&str, QueryInfo> {
    map(place_holder_parser::parse_place_holder, |x| QueryInfo {
        key_name: x.to_string(),
    })(input)
}

fn path_var_parser(input: &str) -> IResult<&str, ParsedPattern<'_>> {
    map_res(
        place_holder_parser::parse_place_holder,
        path_var_inner_parser,
    )(input)
}

fn path_var_inner_parser(
    input: &str,
) -> Result<ParsedPattern<'_>, nom::Err<nom::error::Error<&str>>> {
    let (i, _) = peek(not(char('+')))(input)?;
    Ok(ParsedPattern::Var(i))
}

fn catch_all_path_var_parser(input: &str) -> IResult<&str, ParsedPattern<'_>> {
    map_res(
        place_holder_parser::parse_place_holder,
        catch_all_path_var_inner_parser,
    )(input)
}

fn catch_all_path_var_inner_parser(
    input: &str,
) -> Result<ParsedPattern<'_>, nom::Err<nom::error::Error<&str>>> {
    let (i, _) = char('+')(input)?;
    Ok(ParsedPattern::CatchAllVar(i))
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

#[cfg(test)]
mod tests {
    use crate::gateway_api_definition::http::path_pattern_parser::parse_path_pattern;
    use crate::gateway_api_definition::http::{AllPathPatterns, PathPattern, QueryInfo};
    use test_r::test;

    #[test]
    fn test_parse() {
        use crate::gateway_api_definition::http::LiteralInfo;

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
}
