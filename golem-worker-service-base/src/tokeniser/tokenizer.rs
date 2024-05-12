use crate::tokeniser::token_start_end_rules::Rules;
use std::fmt::Display;
use std::str::Chars;

#[derive(Clone, PartialEq, Debug, Eq, Hash)]
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
    LetEqual,
    SemiColon,
    WildCard,
    At,
}

#[derive(Clone, PartialEq, Debug, Eq, Hash)]
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
    Let,
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

    pub fn let_equal() -> Token {
        Token::LetEqual
    }

    // If a token needs to be considered as only a raw string
    pub fn as_raw_string_token(&self) -> Token {
        match self {
            Token::MultiChar(MultiCharTokens::InterpolationStart) => self.clone(), /* We disallow any normalisation to string if the token is interpolation! */
            Token::RCurly => self.clone(),
            token => Token::MultiChar(MultiCharTokens::Other(token.to_string())),
        }
    }

    pub fn length(&self) -> usize {
        self.to_string().len()
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
                Token::LetEqual => "=",
                Token::SemiColon => ";",
                Token::WildCard => "_",
                Token::At => "@",
                Token::MultiChar(multi_char) => match multi_char {
                    MultiCharTokens::Else => "else",
                    MultiCharTokens::EqualTo => "==",
                    MultiCharTokens::InterpolationStart => "${",
                    MultiCharTokens::GreaterThanOrEqualTo => ">=",
                    MultiCharTokens::LessThanOrEqualTo => "<=",
                    MultiCharTokens::If => "if",
                    MultiCharTokens::Then => "then",
                    MultiCharTokens::Worker => "worker_response",
                    MultiCharTokens::Request => "request",
                    MultiCharTokens::Ok => "ok",
                    MultiCharTokens::Err => "err",
                    MultiCharTokens::Some => "some",
                    MultiCharTokens::None => "none",
                    MultiCharTokens::Match => "match",
                    MultiCharTokens::Arrow => "=>",
                    MultiCharTokens::Other(string) => string.as_str(),
                    MultiCharTokens::Number(number) => number.as_str(),
                    MultiCharTokens::Let => "let",
                },
            }
        )
    }
}

impl Token {
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
    pub fn pos(&self) -> usize {
        self.state.pos
    }

    pub fn peek_at(&mut self, index: usize) -> Option<Token> {
        let original_state = self.state.pos;
        self.state.pos = index;

        let token = self.next_token();
        self.state.pos = original_state;
        token
    }

    pub fn next_non_empty_token(&mut self) -> Option<Token> {
        self.skip_whitespace()
    }

    // Tokenizer progresses, use `peek_next_non_empty_token_is` if only to peek if only to peek
    pub fn next_non_empty_token_is(&mut self, token: &Token) -> bool {
        self.next_non_empty_token() == Some(token.clone())
    }

    pub fn peek_next_non_empty_token_is(&mut self, token: &Token) -> bool {
        self.peek_next_non_empty_token() == Some(token.clone())
    }

    pub fn skip_next_non_empty_token(&mut self) {
        self.next_non_empty_token();
    }

    pub fn skip_if_next_non_empty_token_is(&mut self, token: &Token) {
        if self.peek_next_non_empty_token() == Some(token.clone()) {
            self.next_non_empty_token();
        }
    }

    // Captures the string upto the end token, and advance the cursor further skipping the end token
    pub fn capture_string_until_and_skip_end(&mut self, end: &Token) -> Option<String> {
        let captured_string = self.capture_string_until(end);
        match captured_string {
            Some(captured_string) => {
                self.next_non_empty_token();
                Some(captured_string)
            }
            None => None,
        }
    }

    pub fn capture_string_until_either(
        &mut self,
        token1: &'t Token,
        token2: &'t Token,
    ) -> Option<(&'t Token, String)> {
        let left_index = self.index_of_end_token(token1);
        let right_index = self.index_of_end_token(token2);

        match (left_index, right_index) {
            (Some(x), Some(y)) if x > y => self
                .capture_string_until(token2)
                .map(|string| (token2, string)),
            (Some(_), Some(_)) => self
                .capture_string_until(token1)
                .map(|string| (token1, string)),
            (Some(_), None) => self
                .capture_string_until(token1)
                .map(|string| (token1, string)),
            (None, Some(_)) => self
                .capture_string_until(token2)
                .map(|string| (token2, string)),
            (None, None) => None,
        }
    }

    // Consider this function to be low level function and use it carefully. Example: use expr::util module functions
    // if you are calling this as part of `Expr` language parsing.
    // Captures the string upto the end token, leaving the cursor at the end token (leaving it to the user)
    // It will pick the end token that doesn't correspond to nested_starts.
    // Example: For an input "{a: {a1, a2}, b: {b1, b2}}", if we want to capture the string between "a" and last `}`,
    // then nested_starts is ["{"] and end is `}`. This will make sure that it skips the nested values in between.
    pub fn capture_string_until(&mut self, end: &Token) -> Option<String> {
        let capture_until = self.index_of_end_token(end)?;
        let tokens = self.all_tokens_until(capture_until);

        Some(
            tokens
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<String>>()
                .join(""),
        )
    }

    pub fn capture_tail(&mut self) -> Option<String> {
        // Skip head
        self.next_token();

        let str = self.consume_rest().to_string();

        if str.is_empty() {
            None
        } else {
            Some(str)
        }
    }

    // Low level function, to peek ahead and see the position of the end token
    // Assumes the first token is already consumed. Example:
    // It handles nested situation. Example: After consumes `{`, this function helps to the position of corresponding `}`.
    // It skips all `}` that are part of any another nested `{}` after the first consumed token.
    // The rules of nesting are defined in `TokenStartEnds`.
    // Another example: To find the position of `,`, it will skip all `,` that are part of any another nested `{}` or `[]`, or `()`
    // after the first consumed token.
    pub fn index_of_end_token(&mut self, end_token: &Token) -> Option<usize> {
        let token_start_ends = Rules::of_token(end_token);
        let nested_starts_to_look_for = token_start_ends.all_token_starts();
        let nested_ends_to_look_for = token_start_ends.all_token_ends();
        let mut starts_identified = vec![];

        let mut index: usize = self.state.pos;
        let mut found: bool = false;

        while let Some(current_token) = self.peek_at(index) {
            let current_token_cloned = current_token.clone();

            if nested_starts_to_look_for.contains(&current_token_cloned) {
                starts_identified.push(current_token_cloned);
            } else if nested_ends_to_look_for.contains(&current_token_cloned) {
                let possible_starts =
                    token_start_ends.find_starts_of_a_token(&current_token_cloned);

                // If end_tokens already contain the end token
                if starts_identified.is_empty() && current_token_cloned == end_token.clone() {
                    // Found a matching end token
                    found = true;
                    break;
                }

                // Remove the first possible_start from the starts_identified
                for possible_start in possible_starts {
                    if let Some(index) = starts_identified.iter().position(|x| x == &possible_start)
                    {
                        starts_identified.remove(index);
                        break; // Remove only one element from vec1
                    }
                }
                // If end_tokens doesn't contain the future_token, then we need to find the next one
            } else if current_token_cloned == end_token.clone() && starts_identified.is_empty() {
                // Found a matching end token
                found = true;
                break;
            }

            index += current_token.length();
        }

        if found {
            Some(index)
        } else {
            None
        }
    }

    pub fn skip_whitespace(&mut self) -> Option<Token> {
        let mut non_empty_token: Option<Token> = None;
        while let Some(token) = self.next_token() {
            if token.is_empty() {
            } else {
                non_empty_token = Some(token);
                break;
            }
        }

        non_empty_token
    }

    pub fn next_chars(&self) -> Chars<'t> {
        self.text.get(self.state.pos..).unwrap().chars()
    }

    pub fn all_tokens_until(&mut self, index: usize) -> Vec<Token> {
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

    // Peek ahead the rest without traversal
    pub fn rest(&self) -> &str {
        &self.text[self.state.pos..]
    }

    pub fn rest_at(&self, index: usize) -> &str {
        &self.text[self.state.pos + index..]
    }

    pub fn consume_rest(&mut self) -> &str {
        let str = &self.text[self.state.pos..];
        self.progress_by_n(str.len());
        str
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

    pub fn next_token(&mut self) -> Option<Token> {
        self.get_single_char_token()
            .or_else(|| self.get_multi_char_token())
    }

    pub fn peek_next_token(&mut self) -> Option<Token> {
        let original_state = self.state.clone();
        let token = self.next_token();
        self.state = original_state;
        token
    }

    pub fn peek_next_non_empty_token(&mut self) -> Option<Token> {
        let original_state = self.state.clone();
        let token = self.next_non_empty_token();
        self.state = original_state;
        token
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
            '_' => Some(Token::WildCard),
            ':' => Some(Token::Colon),
            ';' => Some(Token::SemiColon),
            '@' => Some(Token::At),
            '=' => self
                .rest()
                .chars()
                .nth(1)
                .map_or(Some(Token::LetEqual), |c| match c {
                    '=' | '>' => None,
                    _ => Some(Token::LetEqual),
                }),
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
                    "let" => Some(Token::MultiChar(MultiCharTokens::Let)),
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
        let tokens: Vec<Token> = Tokenizer::new("foo bar").collect();
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
        let tokens: Vec<Token> = Tokenizer::new("(foo bar)").collect();
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
        let tokens: Vec<Token> = Tokenizer::new("foo . bar").collect();
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
        let tokens: Vec<Token> = Tokenizer::new("request .").collect();
        assert_eq!(tokens, vec![Token::request(), Token::Space, Token::Dot,]);
    }

    #[test]
    fn test_worker_response() {
        let tokens: Vec<Token> = Tokenizer::new("worker.").collect();
        assert_eq!(tokens, vec![Token::worker(), Token::Dot]);
    }

    #[test]
    fn test_open_close_square_bracket() {
        let tokens: Vec<Token> = Tokenizer::new("[foo bar]").collect();
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
        let tokens: Vec<Token> = Tokenizer::new("if x").collect();

        assert_eq!(
            tokens,
            vec![Token::if_token(), Token::Space, Token::raw_string("x"),]
        );
    }

    #[test]
    fn test_false_ifs() {
        let tokens: Vec<Token> = Tokenizer::new("asif x").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("ifis x").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("if ${x > 1} then 1 else 0").collect();

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

        let tokens: Vec<Token> = Tokenizer::new(string).collect();

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
        let tokens: Vec<Token> = Tokenizer::new("ifxthenyelsez").collect();

        assert_eq!(tokens, vec![Token::raw_string("ifxthenyelsez"),]);
    }

    #[test]
    fn test_greater_than_partial() {
        let tokens: Vec<Token> = Tokenizer::new("f >").collect();

        assert_eq!(
            tokens,
            vec![Token::raw_string("f"), Token::Space, Token::GreaterThan,]
        );
    }

    #[test]
    fn test_greater_than_with_space() {
        let tokens: Vec<Token> = Tokenizer::new("f  > g").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("${foo}>${bar}").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("f <").collect();

        assert_eq!(
            tokens,
            vec![Token::raw_string("f"), Token::Space, Token::LessThan,]
        );
    }

    #[test]
    fn test_less_than_with_space() {
        let tokens: Vec<Token> = Tokenizer::new("f < g").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("f<g").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("${foo} > ${bar}").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("${foo} < ${bar}").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("${foo} == ${bar}").collect();

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
        let tokens: Vec<Token> = Tokenizer::new("${foo}-raw_${bar}").collect();
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
        let tokens: Vec<Token> = Tokenizer::new("${foo}-^raw").collect();
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
        let tokens: Vec<Token> = Tokenizer::new("raw ${foo}").collect();
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
        let tokens: Vec<Token> = Tokenizer::new("foo ${foo} raw ${bar} bar").collect();
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
        let tokens: Vec<Token> = Tokenizer::new("${foo} raw${hi} bar").collect();
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
            "${match worker_response { some(value) => worker_response, none => 'some_value' } }",
        )
        .collect();

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
    fn capture_string_test() {
        let tokens = "(afsal)";

        let mut tokeniser = Tokenizer::new(tokens);
        tokeniser.next_token();
        let result = tokeniser.capture_string_until(&Token::RParen).unwrap();

        assert_eq!(result, "afsal".to_string())
    }

    #[test]
    fn capture_string_test_nested() {
        let tokens = "((afsal))";

        let mut tokeniser = Tokenizer::new(tokens);
        tokeniser.next_token();
        let result = tokeniser.capture_string_until(&Token::RParen).unwrap();

        assert_eq!(result, "(afsal)")
    }

    #[test]
    fn capture_character_test() {
        let tokens = ")";

        let mut tokeniser = Tokenizer::new(tokens);
        let result = tokeniser.capture_string_until(&Token::RParen).unwrap();
        assert_eq!(result, "".to_string())
    }

    #[test]
    fn capture_empty_test() {
        let tokens = "";

        let mut tokeniser = Tokenizer::new(tokens);
        let result = tokeniser.capture_string_until(&Token::RParen);

        assert_eq!(result, None)
    }

    #[test]
    fn test_capture_string_from() {
        let tokens = "else foo";

        let mut tokeniser = Tokenizer::new(tokens);
        let result = tokeniser.capture_tail();

        assert_eq!(result, Some(" foo".to_string()))
    }

    #[test]
    fn test_index_of_last_end_token() {
        let tokens = "else foo }";

        let mut tokeniser = Tokenizer::new(tokens);

        let result = tokeniser.index_of_end_token(&Token::RCurly);

        let unchanged_current_toknen = tokeniser.next_non_empty_token().clone();

        assert_eq!(
            (result, unchanged_current_toknen),
            (Some(9), Some(Token::else_token()))
        )
    }

    #[test]
    fn test_index_of_last_end_token_negative() {
        let tokens = "'not found' }";

        let mut tokeniser = Tokenizer::new(tokens);
        let result = tokeniser.index_of_end_token(&Token::Comma);
        let unchanged_current_toknen = tokeniser.next_non_empty_token().clone();

        assert_eq!(
            (result, unchanged_current_toknen),
            (None, Some(Token::Quote))
        )
    }

    #[test]
    fn test_capture_string_between_quotes() {
        let tokens = "foo' == 'bar'";

        let mut tokeniser = Tokenizer::new(tokens);
        let result = tokeniser.capture_string_until_and_skip_end(&Token::Quote);
        assert_eq!(result, Some("foo".to_string()))
    }
}
