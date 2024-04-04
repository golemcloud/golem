use std::fmt::Display;
use std::str::Chars;

use crate::tokeniser::cursor::TokenCursor;

// Typical usage:
//
// let result = Tokenizer::new("input_value").run();
// let cursor = result.to_cursor();
//   while let Some(token) = cursor.next_token() {}
// }
#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    MultiChar(MultiCharTokens),
    RCurly,
    LCurly,
    LSquare,
    RSquare,
    GreaterThan,
    LessThan,
    Space,
    NewLine,
    LParen,
    RParen,
    Dot,
    Comma,
    Quote,
    Colon,
}

#[derive(Clone, PartialEq, Debug)]
pub enum MultiCharTokens {
    Worker,
    Request,
    Ok,
    Err,
    Some,
    None,
    Match,
    InterpolationStart,
    GreaterThanOrEqualTo,
    LessThanOrEqualTo,
    EqualTo,
    If,
    Then,
    Else,
    Arrow,
    Number(String),
    Other(String),
}

impl Token {
    pub fn raw_string(string: &str) -> Token {
        Token::MultiChar(MultiCharTokens::Other(string.to_string()))
    }

    pub fn interpolation_start() -> Token {
        Token::MultiChar(MultiCharTokens::InterpolationStart)
    }

    pub fn worker() -> Token {
        Token::MultiChar(MultiCharTokens::Worker)
    }

    pub fn request() -> Token {
        Token::MultiChar(MultiCharTokens::Request)
    }

    pub fn if_token() -> Token {
        Token::MultiChar(MultiCharTokens::If)
    }

    pub fn then() -> Token {
        Token::MultiChar(MultiCharTokens::Then)
    }

    pub fn else_token() -> Token {
        Token::MultiChar(MultiCharTokens::Else)
    }

    pub fn match_token() -> Token {
        Token::MultiChar(MultiCharTokens::Match)
    }

    pub fn ok() -> Token {
        Token::MultiChar(MultiCharTokens::Ok)
    }

    pub fn err() -> Token {
        Token::MultiChar(MultiCharTokens::Err)
    }

    pub fn some() -> Token {
        Token::MultiChar(MultiCharTokens::Some)
    }

    pub fn none() -> Token {
        Token::MultiChar(MultiCharTokens::None)
    }

    pub fn arrow() -> Token {
        Token::MultiChar(MultiCharTokens::Arrow)
    }

    pub fn greater_than_or_equal_to() -> Token {
        Token::MultiChar(MultiCharTokens::GreaterThanOrEqualTo)
    }

    pub fn less_than_or_equal_to() -> Token {
        Token::MultiChar(MultiCharTokens::LessThanOrEqualTo)
    }

    pub fn equal_to() -> Token {
        Token::MultiChar(MultiCharTokens::EqualTo)
    }

    pub fn number(number: &str) -> Token {
        Token::MultiChar(MultiCharTokens::Number(number.to_string()))
    }

    // If a token needs to be considered as only a raw string
    pub fn as_raw_string_token(&self) -> Token {
        match self {
            Token::MultiChar(MultiCharTokens::InterpolationStart) => self.clone(), /* We disallow any normalisation to string if the token is interpolation! */
            Token::RCurly => self.clone(),
            token => Token::MultiChar(MultiCharTokens::Other(token.to_string())),
        }
    }
}

impl Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Token::Space => " ",
                Token::RCurly => "}",
                Token::GreaterThan => ">",
                Token::LParen => "(",
                Token::RParen => ")",
                Token::NewLine => "\n",
                Token::LSquare => "[",
                Token::RSquare => "]",
                Token::Dot => ".",
                Token::LCurly => "{",
                Token::Comma => ",",
                Token::Quote => "'",
                Token::LessThan => "<",
                Token::Colon => ":",
                Token::MultiChar(multi_char) => match multi_char {
                    MultiCharTokens::Else => "else",
                    MultiCharTokens::EqualTo => "==",
                    MultiCharTokens::InterpolationStart => "${",
                    MultiCharTokens::GreaterThanOrEqualTo => ">=",
                    MultiCharTokens::LessThanOrEqualTo => "<=",
                    MultiCharTokens::If => "if",
                    MultiCharTokens::Then => "then",
                    MultiCharTokens::Worker => "worker",
                    MultiCharTokens::Request => "request",
                    MultiCharTokens::Ok => "ok",
                    MultiCharTokens::Err => "err",
                    MultiCharTokens::Some => "some",
                    MultiCharTokens::None => "none",
                    MultiCharTokens::Match => "match",
                    MultiCharTokens::Arrow => "=>",
                    MultiCharTokens::Other(string) => string.as_str(),
                    MultiCharTokens::Number(number) => number.as_str(),
                },
            }
        )
    }
}

impl Token {
    pub fn is_non_empty_constructor(&self) -> bool {
        matches!(
            self,
            Token::MultiChar(MultiCharTokens::Ok)
                | Token::MultiChar(MultiCharTokens::Err)
                | Token::MultiChar(MultiCharTokens::Some)
                | Token::MultiChar(MultiCharTokens::Match)
        )
    }

    pub fn is_empty_constructor(&self) -> bool {
        matches!(self, Token::MultiChar(MultiCharTokens::None))
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::MultiChar(MultiCharTokens::Other(string)) => string.is_empty(),
            Self::Space => true,
            _ => false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct State {
    pub(crate) pos: usize,
}

#[derive(Clone)]
pub struct Tokenizer<'a> {
    pub(crate) text: &'a str,
    pub(crate) state: State,
}

impl<'t> Tokenizer<'t> {
    pub fn to_cursor(&self) -> TokenCursor<'t> {
        TokenCursor {
            current_token: None,
            tokenizer: self.clone(),
        }
    }

    pub fn next_chars(&self) -> Chars<'t> {
        self.text.get(self.state.pos..).unwrap().chars()
    }

    pub fn all_tokens_until(&mut self, index: usize)  -> Vec<Token> {
       let mut tokens = vec![];
            while self.state.pos < index {
                if let Some(token) = self.next_token() {
                    tokens.push(token);
                } else {
                    break;
                }
            }
            tokens
    }

    pub fn eat_while(&mut self, f: impl Fn(char) -> bool) -> Option<&str> {
        let beginning = self.state.pos;

        self.state.pos += self
            .next_chars()
            .map_while(|ch| f(ch).then(|| ch.len_utf8()))
            .sum::<usize>();

        self.text.get(beginning..self.state.pos)
    }

    pub fn peek(&self, by: usize) -> Option<&str> {
        self.text.get(self.state.pos..self.state.pos + by)
    }

    pub fn peek_next_char(&self) -> Option<char> {
        self.text.chars().nth(self.state.pos)
    }

    pub fn rest(&self) -> &str {
        &self.text[self.state.pos..]
    }

    pub fn progress(&mut self) {
        self.state.pos += 1;
    }

    pub fn progress_by(&mut self, ch: &char) {
        self.state.pos += ch.len_utf8();
    }

    pub fn progress_by_n(&mut self, n: usize) {
        self.state.pos += n;
    }

    pub fn new(text: &'t str) -> Self {
        Self {
            text,
            state: State { pos: 0 },
        }
    }

    pub fn run(self) -> Vec<Token> {
        self.collect()
    }

    pub fn next_token(&mut self) -> Option<Token> {
        self.get_single_char_token()
            .or_else(|| self.get_multi_char_token())
    }

    fn get_single_char_token(&mut self) -> Option<Token> {
        let ch = self.rest().chars().next()?;
        if let Some(token) = match ch {
            ',' => Some(Token::Comma),
            '{' => Some(Token::LCurly),
            '}' => Some(Token::RCurly),
            '(' => Some(Token::LParen),
            ')' => Some(Token::RParen),
            '[' => Some(Token::LSquare),
            ']' => Some(Token::RSquare),
            '.' => Some(Token::Dot),
            '\'' => Some(Token::Quote),
            '\n' => Some(Token::NewLine),
            ' ' => Some(Token::Space),
            '>' => Some(Token::GreaterThan),
            '<' => Some(Token::LessThan),
            ':' => Some(Token::Colon),
            _ => None,
        } {
            self.progress();
            Some(token)
        } else {
            None
        }
    }

    fn get_multi_char_token(&mut self) -> Option<Token> {
        let ch = self.rest().chars().next()?;
        match ch {
            'a'..='z' | 'A'..='Z' | '-' | '_' => {
                let str =
                    self.eat_while(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')?;
                match str {
                    "worker" => Some(Token::MultiChar(MultiCharTokens::Worker)),
                    "request" => Some(Token::MultiChar(MultiCharTokens::Request)),
                    "ok" => Some(Token::MultiChar(MultiCharTokens::Ok)),
                    "err" => Some(Token::MultiChar(MultiCharTokens::Err)),
                    "some" => Some(Token::MultiChar(MultiCharTokens::Some)),
                    "none" => Some(Token::MultiChar(MultiCharTokens::None)),
                    "match" => Some(Token::MultiChar(MultiCharTokens::Match)),
                    "if" => Some(Token::MultiChar(MultiCharTokens::If)),
                    "then" => Some(Token::MultiChar(MultiCharTokens::Then)),
                    "else" => Some(Token::MultiChar(MultiCharTokens::Else)),
                    random => Some(Token::MultiChar(MultiCharTokens::Other(random.to_string()))),
                }
            }
            '0'..='9' => {
                let str =
                    self.eat_while(|ch| matches!(ch, '0'..='9' | '-' | '.' | 'e' | 'E' | '+'))?;
                Some(Token::MultiChar(MultiCharTokens::Number(str.to_string())))
            }
            _ => self
                .find_double_char_token()
                .or_else(|| self.find_next_char()),
        }
    }

    fn find_next_char(&mut self) -> Option<Token> {
        let final_char = self.peek_next_char()?;
        self.progress_by(&final_char);
        Some(Token::MultiChar(MultiCharTokens::Other(
            final_char.to_string(),
        )))
    }
    fn find_double_char_token(&mut self) -> Option<Token> {
        let peeked = self.peek(2)?;

        match peeked {
            "=>" => {
                self.progress_by(&'=');
                self.progress_by(&'>');
                Some(Token::MultiChar(MultiCharTokens::Arrow))
            }
            ">=" => {
                self.progress_by(&'>');
                self.progress_by(&'=');
                Some(Token::MultiChar(MultiCharTokens::GreaterThanOrEqualTo))
            }
            "<=" => {
                self.progress_by(&'<');
                self.progress_by(&'=');
                Some(Token::MultiChar(MultiCharTokens::LessThanOrEqualTo))
            }
            "==" => {
                self.progress_by(&'=');
                self.progress_by(&'=');
                Some(Token::MultiChar(MultiCharTokens::EqualTo))
            }
            "${" => {
                self.progress_by(&'$');
                self.progress_by(&'{');
                Some(Token::MultiChar(MultiCharTokens::InterpolationStart))
            }
            _ => None,
        }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_token()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{Token, Tokenizer};

    extern crate alloc;

    #[test]
    fn test_raw() {
        let tokens: Vec<Token> = Tokenizer::new("foo bar").run();
        assert_eq!(
            tokens,
            vec![
                Token::raw_string("foo"),
                Token::Space,
                Token::raw_string("bar")
            ]
        );
    }

    #[test]
    fn test_open_close_braces() {
        let tokens: Vec<Token> = Tokenizer::new("(foo bar)").run();
        assert_eq!(
            tokens,
            vec![
                Token::LParen,
                Token::raw_string("foo"),
                Token::Space,
                Token::raw_string("bar"),
                Token::RParen
            ]
        );
    }

    #[test]
    fn test_dot() {
        let tokens: Vec<Token> = Tokenizer::new("foo . bar").run();
        assert_eq!(
            tokens,
            vec![
                Token::raw_string("foo"),
                Token::Space,
                Token::Dot,
                Token::Space,
                Token::raw_string("bar"),
            ]
        );
    }

    #[test]
    fn test_request() {
        let tokens: Vec<Token> = Tokenizer::new("request .").run();
        assert_eq!(tokens, vec![Token::request(), Token::Space, Token::Dot,]);
    }

    #[test]
    fn test_worker_response() {
        let tokens: Vec<Token> = Tokenizer::new("worker.").run();
        assert_eq!(tokens, vec![Token::worker(), Token::Dot]);
    }

    #[test]
    fn test_open_close_square_bracket() {
        let tokens: Vec<Token> = Tokenizer::new("[foo bar]").run();
        assert_eq!(
            tokens,
            vec![
                Token::LSquare,
                Token::raw_string("foo"),
                Token::Space,
                Token::raw_string("bar"),
                Token::RSquare
            ]
        );
    }

    #[test]
    fn test_if_start() {
        let tokens: Vec<Token> = Tokenizer::new("if x").run();

        assert_eq!(
            tokens,
            vec![Token::if_token(), Token::Space, Token::raw_string("x"),]
        );
    }

    #[test]
    fn test_false_ifs() {
        let tokens: Vec<Token> = Tokenizer::new("asif x").run();

        assert_eq!(
            tokens,
            vec![
                Token::raw_string("asif"),
                Token::Space,
                Token::raw_string("x")
            ]
        );
    }

    #[test]
    fn test_false_ifs2() {
        let tokens: Vec<Token> = Tokenizer::new("ifis x").run();

        assert_eq!(
            tokens,
            vec![
                Token::raw_string("ifis"),
                Token::Space,
                Token::raw_string("x")
            ]
        );
    }

    #[test]
    fn test_if_then_else_predicate() {
        let tokens: Vec<Token> = Tokenizer::new("if ${x > 1} then 1 else 0").run();

        assert_eq!(
            tokens,
            vec![
                Token::if_token(),
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("x"),
                Token::Space,
                Token::GreaterThan,
                Token::Space,
                Token::number("1"),
                Token::RCurly,
                Token::Space,
                Token::then(),
                Token::Space,
                Token::number("1"),
                Token::Space,
                Token::else_token(),
                Token::Space,
                Token::number("0"),
            ]
        );
    }

    #[test]
    fn test_if_then_else_multi_line() {
        let string = r#"
if ${x} then ${y}
else${z}
"#;

        let tokens: Vec<Token> = Tokenizer::new(string).run();

        assert_eq!(
            tokens,
            vec![
                Token::NewLine,
                Token::if_token(),
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("x"),
                Token::RCurly,
                Token::Space,
                Token::then(),
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("y"),
                Token::RCurly,
                Token::NewLine,
                Token::else_token(),
                Token::interpolation_start(),
                Token::raw_string("z"),
                Token::RCurly,
                Token::NewLine,
            ]
        );
    }

    #[test]
    fn test_if_then_else_false_expr() {
        let tokens: Vec<Token> = Tokenizer::new("ifxthenyelsez").run();

        assert_eq!(tokens, vec![Token::raw_string("ifxthenyelsez"),]);
    }

    #[test]
    fn test_greater_than_partial() {
        let tokens: Vec<Token> = Tokenizer::new("f >").run();

        assert_eq!(
            tokens,
            vec![Token::raw_string("f"), Token::Space, Token::GreaterThan,]
        );
    }

    #[test]
    fn test_greater_than_with_space() {
        let tokens: Vec<Token> = Tokenizer::new("f  > g").run();

        assert_eq!(
            tokens,
            vec![
                Token::raw_string("f"),
                Token::Space,
                Token::Space,
                Token::GreaterThan,
                Token::Space,
                Token::raw_string("g")
            ]
        );
    }

    #[test]
    fn test_greater_than_no_spaces() {
        let tokens: Vec<Token> = Tokenizer::new("${foo}>${bar}").run();

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
                Token::GreaterThan,
                Token::interpolation_start(),
                Token::raw_string("bar"),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_lessthan_partial() {
        let tokens: Vec<Token> = Tokenizer::new("f <").run();

        assert_eq!(
            tokens,
            vec![Token::raw_string("f"), Token::Space, Token::LessThan,]
        );
    }

    #[test]
    fn test_less_than_with_space() {
        let tokens: Vec<Token> = Tokenizer::new("f < g").run();

        assert_eq!(
            tokens,
            vec![
                Token::raw_string("f"),
                Token::Space,
                Token::LessThan,
                Token::Space,
                Token::raw_string("g")
            ]
        );
    }

    #[test]
    fn test_less_than_with_no_space() {
        let tokens: Vec<Token> = Tokenizer::new("f<g").run();

        assert_eq!(
            tokens,
            vec![
                Token::raw_string("f"),
                Token::LessThan,
                Token::raw_string("g")
            ]
        );
    }

    #[test]
    fn test_greater_than_with_exprs() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} > ${bar}").run();

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
                Token::Space,
                Token::GreaterThan,
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("bar"),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_less_than_with_exprs() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} < ${bar}").run();

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
                Token::Space,
                Token::LessThan,
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("bar"),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_equal_to_with_exprs() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} == ${bar}").run();

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
                Token::Space,
                Token::equal_to(),
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("bar"),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_beginning_and_end() {
        let tokens: Vec<Token> = Tokenizer::new("${foo}-raw_${bar}").run();
        assert_eq!(
            tokens,
            vec![
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
                Token::raw_string("-raw_"),
                Token::interpolation_start(),
                Token::raw_string("bar"),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_beginning() {
        let tokens: Vec<Token> = Tokenizer::new("${foo}-^raw").run();
        assert_eq!(
            tokens,
            vec![
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
                Token::raw_string("-"),
                Token::raw_string("^"),
                Token::raw_string("raw"),
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_end() {
        let tokens: Vec<Token> = Tokenizer::new("raw ${foo}").run();
        assert_eq!(
            tokens,
            vec![
                Token::raw_string("raw"),
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_anywhere() {
        let tokens: Vec<Token> = Tokenizer::new("foo ${foo} raw ${bar} bar").run();
        assert_eq!(
            tokens,
            vec![
                Token::raw_string("foo"),
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
                Token::Space,
                Token::raw_string("raw"),
                Token::Space,
                Token::interpolation_start(),
                Token::raw_string("bar"),
                Token::RCurly,
                Token::Space,
                Token::raw_string("bar")
            ]
        );
    }

    #[test]
    fn test_token_processing_with_dollar() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} raw${hi} bar").run();
        assert_eq!(
            tokens,
            vec![
                Token::interpolation_start(),
                Token::raw_string("foo"),
                Token::RCurly,
                Token::Space,
                Token::raw_string("raw"),
                Token::interpolation_start(),
                Token::raw_string("hi"),
                Token::RCurly,
                Token::Space,
                Token::raw_string("bar"),
            ]
        );
    }

    #[test]
    fn test_token_processing_with_match_expr() {
        let tokens: Vec<Token> = Tokenizer::new(
            "${match worker.response { some(value) => worker.response, none => 'some_value' } }",
        )
        .run();

        assert_eq!(
            tokens,
            vec![
                Token::interpolation_start(),
                Token::match_token(),
                Token::Space,
                Token::worker(),
                Token::Dot,
                Token::raw_string("response"),
                Token::Space,
                Token::LCurly,
                Token::Space,
                Token::some(),
                Token::LParen,
                Token::raw_string("value"),
                Token::RParen,
                Token::Space,
                Token::arrow(),
                Token::Space,
                Token::worker(),
                Token::Dot,
                Token::raw_string("response"),
                Token::Comma,
                Token::Space,
                Token::none(),
                Token::Space,
                Token::arrow(),
                Token::Space,
                Token::Quote,
                Token::raw_string("some_value"),
                Token::Quote,
                Token::Space,
                Token::RCurly,
                Token::Space,
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_path_pattern() {
        let mut cursor = Tokenizer::new("{variable{}}").to_cursor();

        match  cursor.next_token() {
            Some(Token::LCurly) => {
                let result = cursor.capture_string_until(vec![&Token::LCurly], &Token::RCurly).unwrap();
                assert_eq!(result, "variable{}");
            }
            _ => panic!("Expected LCurly"),
        }
    }
}
