use std::fmt::Display;

use regex::Regex;

use super::cursor::TokenCursor;

// Typical usage:
//
// let result = Tokenizer::new("input_value").run();
// let cursor = result.to_cursor();
//   while let Some(token) = cursor.next_token() {}
// }
#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    Worker,
    Request,
    Ok,
    Err,
    Some,
    None,
    Match,
    RawString(String),
    InterpolationStart,
    ClosedCurlyBrace,
    OpenCurlyBrace,
    OpenSquareBracket,
    ClosedSquareBracket,
    GreaterThan,
    Space,
    NewLine,
    GreaterThanOrEqualTo,
    LessThan,
    LessThanOrEqualTo,
    EqualTo,
    If,
    OpenParen,
    CloseParen,
    Then,
    Else,
    Dot,
    Arrow,
    Comma,
    Quote,
}

impl Token {
    pub fn is_white_space(&self) -> bool {
        match self {
            Token::Space => true,
            Token::NewLine => true,
            Token::Else => false,
            Token::EqualTo => false,
            Token::InterpolationStart => false,
            Token::ClosedCurlyBrace => false,
            Token::GreaterThan => false,
            Token::GreaterThanOrEqualTo => false,
            Token::LessThanOrEqualTo => false,
            Token::LessThan => false,
            Token::If => false,
            Token::Then => false,
            Token::OpenParen => false,
            Token::CloseParen => false,
            Token::RawString(_) => false,
            Token::OpenSquareBracket => false,
            Token::ClosedSquareBracket => false,
            Token::Dot => false,
            Token::Worker => false,
            Token::Request => false,
            Token::Ok => false,
            Token::Err => false,
            Token::Some => false,
            Token::None => false,
            Token::OpenCurlyBrace => false,
            Token::Match => false,
            Token::Arrow => false,
            Token::Comma => false,
            Token::Quote => false,
        }
    }

    // If a token needs to be considered as only a raw string
    pub fn as_raw_string_token(&self) -> Token {
        match self {
            Token::InterpolationStart => self.clone(), /* We disallow any normalisation to string if the token is interpolation! */
            Token::ClosedCurlyBrace => self.clone(),
            token => Token::RawString(token.to_string()),
        }
    }

    pub fn is_code(&self) -> bool {
        matches!(self, Token::InterpolationStart)
    }

    pub fn raw_string(input: &str) -> Token {
        Token::RawString(input.to_string())
    }
}

impl Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Token::Else => "else",
                Token::Space => " ",
                Token::EqualTo => "==",
                Token::InterpolationStart => "${",
                Token::ClosedCurlyBrace => "}",
                Token::GreaterThan => ">",
                Token::GreaterThanOrEqualTo => ">=",
                Token::LessThanOrEqualTo => "<=",
                Token::LessThan => "<",
                Token::If => "if",
                Token::Then => "then",
                Token::OpenParen => "(",
                Token::CloseParen => ")",
                Token::NewLine => "\n",
                Token::RawString(string) => string,
                Token::OpenSquareBracket => "[",
                Token::ClosedSquareBracket => "]",
                Token::Dot => ".",
                Token::Worker => "worker",
                Token::Request => "request",
                Token::Ok => "ok",
                Token::Err => "err",
                Token::Some => "some",
                Token::None => "none",
                Token::Match => "match",
                Token::OpenCurlyBrace => "{",
                Token::Arrow => "=>",
                Token::Comma => ",",
                Token::Quote => "'",
            }
        )
    }
}

impl Token {
    pub fn is_non_empty_constructor(&self) -> bool {
        matches!(self, Token::Ok | Token::Err | Token::Some | Token::Match)
    }

    pub fn is_empty_constructor(&self) -> bool {
        matches!(self, Token::None)
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::RawString(string) => string.is_empty(),
            _ => false,
        }
    }

    pub fn trim(&self) -> Token {
        match self {
            Self::RawString(string) => Self::RawString(string.trim().to_string()),
            anything => anything.clone(),
        }
    }
}

// Vec<Token>
// Vec<PlaceHolder> Space Vec<PlaceHolder>

#[derive(Clone)]
enum TokenizerState {
    Beginning,
    Text,
    Static(Token),
    End,
}

impl TokenizerState {
    fn is_end(&self) -> bool {
        match self {
            Self::Beginning => false,
            Self::Text => false,
            Self::End => true,
            Self::Static(_) => false,
        }
    }
}

pub struct Tokenizer {
    text: String,
    state: TokenizerState,
}

impl<'t> Tokenizer {
    pub fn new(text: &'t str) -> Self {
        Self {
            text: text.to_string(),
            state: TokenizerState::Beginning,
        }
    }

    // Collect tokens gets rid of empty spaces and make sure everything is trimmed
    // If needed raw versions as it is, use `.collect()` instead of `.collect_tokens`
    pub fn run(self) -> TokeniserResult {
        let all_tokens: Vec<Token> = self.collect();

        TokeniserResult {
            value: all_tokens
                .into_iter()
                .flat_map(|x: Token| if x.is_empty() { vec![] } else { vec![x] })
                .collect(),
        }
    }

    fn get_token(&mut self) -> Token {
        let mut token: Option<Token> = None;

        for (character_index, c) in tokenise_string_with_index(self.text.as_str()) {
            if c == "<" {
                token = Some(Token::RawString(self.text[..character_index].to_string())); // Example: we token out the string just before <, and marks the state as <
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::LessThan);
                break;
            } else if c == "<=" {
                token = Some(Token::RawString(self.text[..character_index].to_string())); // Example: we token out the string just before <, and marks the state as <
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::LessThanOrEqualTo);
                break;
            } else if c == ">" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::GreaterThan);
                break;
            } else if c == ">=" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::GreaterThanOrEqualTo);
                break;
            } else if c == "==" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::EqualTo);
                break;
            } else if c == "if" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::If);
                break;
            } else if c == "then" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 4..].to_string();
                self.state = TokenizerState::Static(Token::Then);
                break;
            } else if c == "else" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 4..].to_string();
                self.state = TokenizerState::Static(Token::Else);
                break;
            } else if c == "(" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::OpenParen);
                break;
            } else if c == ")" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::CloseParen);
                break;
            } else if c == "[" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::OpenSquareBracket);
                break;
            } else if c == "]" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::ClosedSquareBracket);
                break;
            } else if c == "${" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::InterpolationStart);
                break;
            } else if c == "=>" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::Arrow);
                break;
            } else if c == "{" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::OpenCurlyBrace);
                break;
            } else if c == "}" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::ClosedCurlyBrace);
                break;
            } else if c == " " {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::Space);
                break;
            } else if c == "\n" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::NewLine);
                break;
            } else if c == "." {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::Dot);
                break;
            } else if c == "worker" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Worker.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Worker);
                break;
            } else if c == "request" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Request.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Request);
                break;
            }  else if c == "ok" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + Token::Ok.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Ok);
                break;
            } else if c == "err" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + Token::Err.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Err);
                break;
            } else if c == "some" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Some.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Some);
                break;
            } else if c == "none" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::None.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::None);
                break;
            } else if c == "null" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + "null".to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::None);
                break;
            } else if c == "match" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Match.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Match);
                break;
            } else if c == "," {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Comma.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Comma);
                break;
            } else if c == "'" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Quote.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Quote);
                break;
            }
        }

        match token {
            Some(token) => token,
            None => {
                token = Some(Token::RawString(self.text.clone()));
                self.text = "".to_string();
                self.state = TokenizerState::End;
                token.unwrap()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TokeniserResult {
    pub value: Vec<Token>,
}

impl TokeniserResult {
    pub fn to_cursor(&self) -> TokenCursor {
        TokenCursor::new(self.value.clone())
    }

    pub fn filter_spaces(&self) -> TokeniserResult {
        TokeniserResult {
            value: self
                .value
                .iter()
                .filter(|token| !token.trim().is_empty())
                .cloned()
                .collect(),
        }
    }
}

fn tokenise_string_with_index(input_string: &str) -> Vec<(usize, &str)> {
    let mut result: Vec<(usize, &str)> = Vec::new();
    let mut current_index = 0;
    let token_regex_pattern = Regex::new(
        r"(worker|request|,|\.|'|<=|\$\{|}|>=|\n| |==|<|>|\bif\b|\bthen\b|\belse\b|=>|\{|\bsome\b|\bnone\b|\bmatch\b|\bok\b|\berr\b|[ -]|[^\s])|[\(\)]|\[|\]|(\w+)",
    )
        .unwrap();

    for capture in token_regex_pattern.captures_iter(input_string) {
        if let Some(matched) = capture.get(1) {
            let matched_str = matched.as_str();
            result.push((current_index, matched_str));
            current_index += matched_str.len();
        } else if let Some(matched) = capture.get(2) {
            let matched_str = matched.as_str();
            result.push((current_index, matched_str));
            current_index += matched_str.len();
        }
    }

    result
}

impl Iterator for Tokenizer {
    type Item = Token;

    fn next(&mut self) -> Option<Self::Item> {
        if self.state.is_end() {
            return None;
        }

        match self.state.clone() {
            TokenizerState::Beginning => Some(self.get_token()),
            TokenizerState::Text => Some(self.get_token()),
            TokenizerState::Static(inner) => {
                self.state = TokenizerState::Text;
                Some(inner)
            }
            TokenizerState::End => None,
        }
    }
}

#[cfg(test)]
mod tests {

    use super::{Token, Tokenizer};
    extern crate alloc;
    use alloc::vec::Vec;

    #[test]
    fn test_raw() {
        let tokens: Vec<Token> = Tokenizer::new("foo bar").run().value;
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
        let tokens: Vec<Token> = Tokenizer::new("(foo bar)").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::OpenParen,
                Token::raw_string("foo"),
                Token::Space,
                Token::raw_string("bar"),
                Token::CloseParen
            ]
        );
    }

    #[test]
    fn test_dot() {
        let tokens: Vec<Token> = Tokenizer::new("foo . bar").run().value;
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
        let tokens: Vec<Token> = Tokenizer::new("request .").run().value;
        assert_eq!(tokens, vec![Token::Request, Token::Space, Token::Dot,]);
    }

    #[test]
    fn test_worker_response() {
        let tokens: Vec<Token> = Tokenizer::new("worker.").run().value;
        assert_eq!(tokens, vec![Token::Worker, Token::Dot]);
    }


    #[test]
    fn test_open_close_square_bracket() {
        let tokens: Vec<Token> = Tokenizer::new("[foo bar]").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::OpenSquareBracket,
                Token::raw_string("foo"),
                Token::Space,
                Token::raw_string("bar"),
                Token::ClosedSquareBracket
            ]
        );
    }

    #[test]
    fn test_if_start() {
        let tokens: Vec<Token> = Tokenizer::new("if x").run().value;

        assert_eq!(
            tokens,
            vec![Token::If, Token::Space, Token::raw_string("x"),]
        );
    }

    #[test]
    fn test_false_ifs() {
        let tokens: Vec<Token> = Tokenizer::new("asif x").run().value;

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
        let tokens: Vec<Token> = Tokenizer::new("ifis x").run().value;

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
        let tokens: Vec<Token> = Tokenizer::new("if ${x > 1} then 1 else 0").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::If,
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("x"),
                Token::Space,
                Token::GreaterThan,
                Token::Space,
                Token::raw_string("1"),
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::Then,
                Token::Space,
                Token::raw_string("1"),
                Token::Space,
                Token::Else,
                Token::Space,
                Token::raw_string("0"),
            ]
        );
    }

    #[test]
    fn test_if_then_else_multi_line() {
        let string = r#"
if ${x} then ${y}
else${z}
"#;

        let tokens: Vec<Token> = Tokenizer::new(string).run().value;

        assert_eq!(
            tokens,
            vec![
                Token::NewLine,
                Token::If,
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("x"),
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::Then,
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("y"),
                Token::ClosedCurlyBrace,
                Token::NewLine,
                Token::Else,
                Token::InterpolationStart,
                Token::raw_string("z"),
                Token::ClosedCurlyBrace,
                Token::NewLine,
            ]
        );
    }

    #[test]
    fn test_if_then_else_false_expr() {
        let tokens: Vec<Token> = Tokenizer::new("ifxthenyelsez").run().value;

        assert_eq!(tokens, vec![Token::raw_string("ifxthenyelsez"),]);
    }

    #[test]
    fn test_greater_than_partial() {
        let tokens: Vec<Token> = Tokenizer::new("f >").run().value;

        assert_eq!(
            tokens,
            vec![Token::raw_string("f"), Token::Space, Token::GreaterThan,]
        );
    }

    #[test]
    fn test_greater_than_with_space() {
        let tokens: Vec<Token> = Tokenizer::new("f  > g").run().value;

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
        let tokens: Vec<Token> = Tokenizer::new("${foo}>${bar}").run().value;

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
                Token::GreaterThan,
                Token::InterpolationStart,
                Token::raw_string("bar"),
                Token::ClosedCurlyBrace,
            ]
        );
    }

    #[test]
    fn test_lessthan_partial() {
        let tokens: Vec<Token> = Tokenizer::new("f <").run().value;

        assert_eq!(
            tokens,
            vec![Token::raw_string("f"), Token::Space, Token::LessThan,]
        );
    }

    #[test]
    fn test_less_than_with_space() {
        let tokens: Vec<Token> = Tokenizer::new("f < g").run().value;

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
        let tokens: Vec<Token> = Tokenizer::new("f<g").run().value;

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
        let tokens: Vec<Token> = Tokenizer::new("${foo} > ${bar}").run().value;

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::GreaterThan,
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("bar"),
                Token::ClosedCurlyBrace,
            ]
        );
    }

    #[test]
    fn test_less_than_with_exprs() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} < ${bar}").run().value;

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::LessThan,
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("bar"),
                Token::ClosedCurlyBrace,
            ]
        );
    }

    #[test]
    fn test_equal_to_with_exprs() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} == ${bar}").run().value;

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::EqualTo,
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("bar"),
                Token::ClosedCurlyBrace,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_beginning_and_end() {
        let tokens: Vec<Token> = Tokenizer::new("${foo}-raw_${bar}").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
                Token::raw_string("-raw_"),
                Token::InterpolationStart,
                Token::raw_string("bar"),
                Token::ClosedCurlyBrace,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_beginning() {
        let tokens: Vec<Token> = Tokenizer::new("${foo}-^raw").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
                Token::raw_string("-^raw")
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_end() {
        let tokens: Vec<Token> = Tokenizer::new("raw ${foo}").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::raw_string("raw"),
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_anywhere() {
        let tokens: Vec<Token> = Tokenizer::new("foo ${foo} raw ${bar} bar").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::raw_string("foo"),
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::raw_string("raw"),
                Token::Space,
                Token::InterpolationStart,
                Token::raw_string("bar"),
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::raw_string("bar")
            ]
        );
    }

    #[test]
    fn test_token_processing_with_dollar() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} raw${hi} bar").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::raw_string("foo"),
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::raw_string("raw"),
                Token::InterpolationStart,
                Token::raw_string("hi"),
                Token::ClosedCurlyBrace,
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
        .run()
        .value;
        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::Match,
                Token::Space,
                Token::Worker,
                Token::Dot,
                Token::raw_string("response"),
                Token::Space,
                Token::OpenCurlyBrace,
                Token::Space,
                Token::Some,
                Token::OpenParen,
                Token::raw_string("value"),
                Token::CloseParen,
                Token::Space,
                Token::Arrow,
                Token::Space,
                Token::Worker,
                Token::Dot,
                Token::raw_string("response"),
                Token::Comma,
                Token::Space,
                Token::None,
                Token::Space,
                Token::Arrow,
                Token::Space,
                Token::Quote,
                Token::raw_string("some_value"),
                Token::Quote,
                Token::Space,
                Token::ClosedCurlyBrace,
                Token::Space,
                Token::ClosedCurlyBrace,
            ]
        );
    }
}
