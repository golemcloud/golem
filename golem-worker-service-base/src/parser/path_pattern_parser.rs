use nom::branch::alt;
use nom::IResult;

use super::*;
use crate::http_api_definition::{PathPattern, QueryInfo, VarInfo};
use crate::parser::{literal_parser, place_holder_parser, ParseError};

pub struct PathPatternParser;

impl GolemParser<PathPattern> for PathPatternParser {
    fn parse(&self, input: &str) -> Result<PathPattern, ParseError> {
        get_path_pattern(input)
    }
}

fn get_path_pattern(input: &str) -> Result<PathPattern, ParseError> {
    let split_path_and_query: Vec<&str> = input.split('?').collect();

    let path_side = split_path_and_query
        .first()
        .ok_or(ParseError::Message("Path cannot be empty".to_string()))?;

    // initial `/` is excluded to not break indexes
    let path = if path_side.starts_with('/') {
        &path_side[1..path_side.len()]
    } else {
        path_side
    };

    let query_side = split_path_and_query.get(1);

    let mut path_patterns: Vec<PathPattern> = vec![];

    for (index, path_component) in path.split('/').enumerate().filter(|x| !x.1.is_empty()) {
        let (_, pattern) = alt((
            |input| get_path_var_parser(index, input),
            get_literal_parser,
        ))(path_component.trim())
        .map_err(|err| ParseError::Message(err.to_string()))?;
        path_patterns.push(pattern);
    }

    if let Some(query_side) = query_side {
        for query_component in query_side.split('&') {
            let (_, pattern) = alt((get_query_parser, get_literal_parser))(query_component.trim())
                .map_err(|err| ParseError::Message(err.to_string()))?;
            path_patterns.push(pattern);
        }
    }

    Ok(PathPattern::Zip(path_patterns))
}

fn get_query_parser(input: &str) -> IResult<&str, PathPattern> {
    place_holder_parser::parse_place_holder(input)
        .map(|x| (x.0, PathPattern::Query(QueryInfo { key_name: x.1 .0 })))
}

fn get_path_var_parser(index: usize, input: &str) -> IResult<&str, PathPattern> {
    place_holder_parser::parse_place_holder(input).map(|x| {
        (
            x.0,
            PathPattern::Var(VarInfo {
                key_name: x.1 .0,
                index,
            }),
        )
    })
}

fn get_literal_parser(input: &str) -> IResult<&str, PathPattern> {
    literal_parser::parse_literal_pattern(input).map(|x| (x.0, PathPattern::Literal(x.1)))
}
