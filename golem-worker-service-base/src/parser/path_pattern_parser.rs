use nom::branch::alt;
use nom::bytes::complete::take_while1;
use nom::character::complete::{char, multispace0};
use nom::combinator::{map, opt};

use nom::multi::{separated_list0, separated_list1};
use nom::sequence::{delimited, preceded, tuple};
use nom::IResult;

use crate::api_definition::http::{AllPathPatterns, LiteralInfo, PathPattern, QueryInfo, VarInfo};
use crate::parser::{place_holder_parser, ParseError};

use super::*;

pub struct PathPatternParser;

impl GolemParser<AllPathPatterns> for PathPatternParser {
    fn parse(&self, input: &str) -> Result<AllPathPatterns, ParseError> {
        parse_path_pattern(input)
            .map(|(_, result)| result)
            .map_err(|err| ParseError::Message(err.to_string()))
    }
}

fn parse_path_pattern(input: &str) -> IResult<&str, AllPathPatterns> {
    let (input, (path, query)) = tuple((
        delimited(opt(char('/')), path_parser, opt(char('/'))),
        opt(preceded(char('?'), query_parser)),
    ))(input)?;

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
    let (input, patterns) = separated_list1(char('/'), item_parser)(input)?;

    let indexed_patterns = patterns
        .into_iter()
        .enumerate()
        .map(|(index, pattern)| match pattern {
            ParsedPattern::Literal(literal) => {
                PathPattern::Literal(LiteralInfo(literal.to_string()))
            }
            ParsedPattern::Var(var) => PathPattern::Var(VarInfo {
                key_name: var.to_string(),
                index,
            }),
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

fn path_var_parser<'a>(input: &'a str) -> IResult<&'a str, ParsedPattern<'a>> {
    map(place_holder_parser::parse_place_holder, |x| {
        ParsedPattern::Var(x)
    })(input)
}

#[derive(Debug)]
enum ParsedPattern<'a> {
    Literal(&'a str),
    Var(&'a str),
}

fn literal_parser<'a>(input: &'a str) -> IResult<&'a str, ParsedPattern<'a>> {
    map(take_while1(|c| !"/{}?&".contains(c)), |x| {
        ParsedPattern::Literal(x)
    })(input)
}

#[test]
fn test_parse() {
    use crate::api_definition::http::LiteralInfo;

    let result = parse_path_pattern("/api/{id}/test/{name}/test2?{query1}&{query2}");
    assert_eq!(
        AllPathPatterns {
            path_patterns: vec![
                PathPattern::Literal(LiteralInfo("api".to_string())),
                PathPattern::Var(VarInfo {
                    key_name: "id".to_string(),
                    index: 1
                }),
                PathPattern::Literal(LiteralInfo("test".to_string())),
                PathPattern::Var(VarInfo {
                    key_name: "name".to_string(),
                    index: 3
                }),
                PathPattern::Literal(LiteralInfo("test2".to_string())),
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
                PathPattern::Var(VarInfo {
                    key_name: "id".to_string(),
                    index: 1
                }),
                PathPattern::Literal(LiteralInfo("test".to_string())),
                PathPattern::Var(VarInfo {
                    key_name: "name".to_string(),
                    index: 3
                }),
                PathPattern::Literal(LiteralInfo("test2".to_string())),
            ],
            query_params: vec![]
        },
        result.unwrap().1
    );
}
