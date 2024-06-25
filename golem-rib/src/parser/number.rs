use combine::{many1, Parser};

use crate::expr::Expr;
use combine::parser::char::{char, spaces, digit, letter};

use combine::stream::easy;

use combine::error::StreamError;

pub fn number<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(many1(letter().or(char('-')).or(digit()).or(char('.')))
        .and_then(|s: Vec<char>| {
            let primitive = s.into_iter().collect::<String>();

            if let Ok(u64) = primitive.parse::<u64>() {
                Ok(Expr::unsigned_integer(u64))
            } else if let Ok(i64_value) = primitive.parse::<i64>() {
                Ok(Expr::signed_integer(i64_value))
            } else if let Ok(f64_value) = primitive.parse::<f64>() {
                Ok(Expr::float(f64_value))
            } else {
                Err(easy::Error::message_static_message(
                    "Unable to parse number",
                ))
            }
        })
        .message("Unable to parse number"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_number() {
        let input = "123";
        let result = number().easy_parse(input);
        assert_eq!(result, Ok((Expr::unsigned_integer(123), "")));
    }

    #[test]
    fn test_negative_number() {
        let input = "-123";
        let result = number().easy_parse(input);
        assert_eq!(result, Ok((Expr::signed_integer(-123), "")));
    }

    #[test]
    fn test_float_number() {
        let input = "123.456";
        let result = number().easy_parse(input);
        assert_eq!(result, Ok((Expr::float(123.456), "")));
    }
}
