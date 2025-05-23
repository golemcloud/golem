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

use nom::character::complete;
use nom::sequence::delimited;
use nom::IResult;

pub fn parse_place_holder(input: &str) -> IResult<&str, &str> {
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
    use crate::gateway_api_definition::http::place_holder_parser::parse_place_holder;
    use test_r::test;

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
