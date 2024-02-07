use super::tokeniser::Token;

// A better management for traversing through token
// without worrying about white spaces
// It is decided that expression language is white space insensitive
pub struct TokenCursor {
    tokens: Vec<Token>,
    index: usize,
}

impl TokenCursor {
    pub fn new(tokens: Vec<Token>) -> TokenCursor {
        TokenCursor { tokens, index: 0 }
    }

    pub fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    pub fn next_token(&mut self) -> Option<Token> {
        let token = self.peek().cloned();
        self.advance();
        token
    }

    pub fn advance(&mut self) {
        self.index += 1;
    }

    pub fn next_non_empty_token(&mut self) -> Option<Token> {
        self.skip_whitespace();
        self.next_token()
    }

    // State of cursor doesn't change similar to peek
    pub fn next_non_empty_char_is(&mut self, token: Token) -> bool {
        let mut index: usize = self.index;
        let mut matches: bool = false;

        while let Some(s) = self.tokens.get(index).map(|x| x.to_string()) {
            if s.chars().all(char::is_whitespace) {
                index += 1;
            } else {
                matches = s == token.to_string();
                break;
            }
        }

        matches
    }

    pub fn capture_string_between(&mut self, start: &Token, end: &Token) -> Option<String> {
        let capture_until = self.index_of_last_end_token(start, end);

        let mut tokens = vec![];

        let result = match capture_until {
            Some(capture_until) => {
                for index in self.index..capture_until {
                    let token = self.tokens.get(index);

                    if let Some(token) = token {
                        tokens.push(token.clone())
                    }
                }

                self.index = capture_until + 1;

                Some(
                    tokens
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<String>>()
                        .join(""),
                )
            }

            None => None,
        };

        if self.index > 0 {
            self.index -= 1
        }; // shift to the end token index instead of forgetting it

        result
    }

    pub fn capture_tail(&mut self) -> Option<String> {
        self.index += 1;

        let mut tokens_after = Vec::new();

        while let Some(token) = self.tokens.get(self.index) {
            tokens_after.push(token.clone());

            self.index += 1;
        }

        if tokens_after.is_empty() {
            None
        } else {
            Some(
                tokens_after
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<String>>()
                    .join(""),
            )
        }
    }

    pub fn index_of_last_end_token(&mut self, start: &Token, end: &Token) -> Option<usize> {
        let mut index: usize = self.index;

        let mut start_token_count = 0;

        let mut found: bool = false;

        while let Some(current_token) = self.tokens.get(index).map(|x| x.to_string()) {
            if current_token == start.to_string() {
                // That it finds a start token again
                start_token_count += 1;
            } else if current_token == end.to_string() {
                // Making sure any nested start token was closed (making it always a zero) before break
                if start_token_count == 0 {
                    // Found a matching end token
                    found = true;
                    break;
                } else {
                    start_token_count -= 1;
                }
            }

            index += 1;
        }

        if found {
            Some(index)
        } else {
            None
        }
    }

    pub fn skip_whitespace(&mut self) {
        while let Some(token) = self.peek() {
            if token.is_white_space() {
                self.advance();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn capture_string_test() {
        let tokens = vec![
            Token::OpenParen,
            Token::RawString("afsal".to_string()),
            Token::CloseParen,
        ];

        let mut cursor = TokenCursor::new(tokens.clone());
        cursor.next_token();
        let result = cursor
            .capture_string_between(&Token::OpenParen, &Token::CloseParen)
            .unwrap();

        assert_eq!(result, "afsal".to_string())
    }

    #[test]
    fn capture_string_test_nested() {
        let tokens = vec![
            Token::OpenParen,
            Token::OpenParen,
            Token::RawString("afsal".to_string()),
            Token::CloseParen,
            Token::CloseParen,
        ];

        let mut cursor = TokenCursor::new(tokens.clone());
        cursor.next_token();
        let result = cursor
            .capture_string_between(&Token::OpenParen, &Token::CloseParen)
            .unwrap();

        assert_eq!(result, "(afsal)".to_string())
    }

    #[test]
    fn capture_character_test() {
        let tokens = vec![Token::CloseParen];

        let mut cursor = TokenCursor::new(tokens.clone());
        let result = cursor
            .capture_string_between(&Token::OpenParen, &Token::CloseParen)
            .unwrap();
        assert_eq!(result, "".to_string())
    }

    #[test]
    fn capture_empty_test() {
        let tokens = vec![];

        let mut cursor = TokenCursor::new(tokens.clone());
        let result = cursor.capture_string_between(&Token::OpenParen, &Token::CloseParen);

        assert_eq!(result, None)
    }

    #[test]
    fn test_next_non_empty_char() {
        let tokens = vec![
            Token::RawString(" ".to_string()),
            Token::RawString(" ".to_string()),
            Token::CloseParen,
        ];

        let mut cursor = TokenCursor::new(tokens.clone());
        let result = cursor.next_non_empty_char_is(Token::CloseParen);
        assert!(result)
    }

    #[test]
    fn test_capture_string_from() {
        let tokens = vec![Token::Else, Token::RawString("foo".to_string())];

        let mut cursor = TokenCursor::new(tokens.clone());
        let result = cursor.capture_tail();

        assert_eq!(result, Some("foo".to_string()))
    }
}
