#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Token<'a> {
    // Keywords & Symbols
    Fn,
    If,
    Else,
    Return,
    While,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Dot,

    // ; or newline (both \n and \r\n)
    Separator,

    // Operators
    Assign, // =
    Equals, // ==
    Lt,     // <
    Gt,     // >
    Lte,    // <=
    Gte,    // >=

    Plus,
    Minus, // unary or binary
    Star,
    Slash,
    Bang, // unary not

    // Data
    Identifier(&'a [u8]),
    IntegerLit(i32),

    // Note: string content is still escaped, as that would require allocation
    StringLit {
        content: &'a [u8],
        has_escape_characters: bool,
    },

    // Special
    Eof,
    Error,
}

pub(crate) struct Tokenizer<'a> {
    input: &'a [u8],
    cursor: usize,
    last_token: Option<Token<'a>>,
}

impl<'a> Tokenizer<'a> {
    pub const fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            cursor: 0,
            last_token: None,
        }
    }

    // TODO: maybe implement a reset(&'a [u8]) function to reuse the same tokenizer for multiple scripts
    // Would allow allocating a single engine once containing it

    pub fn set_cursor(&mut self, pos: usize) {
        self.cursor = pos;
        self.last_token = None; // Reset last token since we're jumping
    }

    pub fn cursor_pos(&self) -> usize {
        self.cursor
    }

    /// Returns the next token and advances the tokenizer state.
    pub fn advance(&mut self) -> Token<'a> {
        let (tok, cursor, last_token) =
            Self::next_token_inner(self.input, self.cursor, self.last_token);
        self.cursor = cursor;
        self.last_token = last_token;
        tok
    }

    /// Returns the next token without advancing the tokenizer state.
    pub fn peek(&self) -> Token<'a> {
        let (tok, _, _) = Self::next_token_inner(self.input, self.cursor, self.last_token);
        tok
    }

    pub fn peek2(&self) -> (Token<'a>, Token<'a>) {
        let (tok1, cursor, lt) = Self::next_token_inner(self.input, self.cursor, self.last_token);
        let (tok2, _, _) = Self::next_token_inner(self.input, cursor, lt);

        return (tok1, tok2);
    }

    /// The actual tokenization logic, stateless.
    fn next_token_inner(
        input: &'a [u8],
        mut cursor: usize,
        last_token: Option<Token<'a>>,
    ) -> (Token<'a>, usize, Option<Token<'a>>) {
        loop {
            // 1. Skip spaces and tabs ONLY
            while cursor < input.len() {
                let b = input[cursor];
                if b == b' ' || b == b'\t' || b == b'\r' {
                    cursor += 1;
                } else {
                    break;
                }
            }

            // 2. Check EOF
            if cursor >= input.len() {
                return (Token::Eof, cursor, last_token);
            }

            // 3. Read current byte & advance
            let start = cursor;
            let c = input[cursor];
            cursor += 1;

            match c {
                b'(' => return (Token::LParen, cursor, Some(Token::LParen)),
                b')' => return (Token::RParen, cursor, Some(Token::RParen)),
                b'{' => return (Token::LBrace, cursor, Some(Token::LBrace)),
                b'}' => return (Token::RBrace, cursor, Some(Token::RBrace)),
                b',' => return (Token::Comma, cursor, Some(Token::Comma)),

                b';' | b'\n' => {
                    // greedily consume any other separators / whitespace
                    while cursor < input.len() {
                        let b = input[cursor];
                        match b {
                            b';' | b'\n' | b' ' | b'\t' | b'\r' => cursor += 1,
                            _ => break,
                        }
                    }

                    // Check if we are effectively at the start of the file
                    let is_at_start = input[..start].iter().all(|b| b.is_ascii_whitespace());

                    // Only emit separator if not after LBrace or at start
                    if is_at_start || matches!(last_token, Some(Token::LBrace)) {
                        continue; // Skip this separator
                    }

                    return (Token::Separator, cursor, Some(Token::Separator));
                }

                b'!' => return (Token::Bang, cursor, Some(Token::Bang)),
                b'.' => return (Token::Dot, cursor, Some(Token::Dot)),
                b'+' => return (Token::Plus, cursor, Some(Token::Plus)),
                b'-' => return (Token::Minus, cursor, Some(Token::Minus)),
                b'*' => return (Token::Star, cursor, Some(Token::Star)),
                b'/' => return (Token::Slash, cursor, Some(Token::Slash)),

                b'=' => {
                    if cursor < input.len() && input[cursor] == b'=' {
                        cursor += 1; // Consume second '='
                        return (Token::Equals, cursor, Some(Token::Equals));
                    } else {
                        return (Token::Assign, cursor, Some(Token::Assign));
                    }
                }
                b'<' => {
                    if cursor < input.len() && input[cursor] == b'=' {
                        cursor += 1; // Consume '='
                        return (Token::Lte, cursor, Some(Token::Lte));
                    } else {
                        return (Token::Lt, cursor, Some(Token::Lt));
                    }
                }
                b'>' => {
                    if cursor < input.len() && input[cursor] == b'=' {
                        cursor += 1; // Consume '='
                        return (Token::Gte, cursor, Some(Token::Gte));
                    } else {
                        return (Token::Gt, cursor, Some(Token::Gt));
                    }
                }

                // --- String Literals ---
                b'"' => {
                    let content_start = cursor;
                    let mut has_escape_characters = false;
                    while cursor < input.len() && input[cursor] != b'"' {
                        if input[cursor] == b'\\' {
                            has_escape_characters = true;
                        }
                        cursor += 1;
                    }

                    if cursor >= input.len() {
                        return (Token::Error, cursor, Some(Token::Error)); // Unclosed string
                    }

                    let s = &input[content_start..cursor];
                    cursor += 1; // Skip closing quote
                    // Store escape info in a tuple (slice, bool)
                    return (
                        Token::StringLit {
                            content: s,
                            has_escape_characters,
                        },
                        cursor,
                        Some(Token::StringLit {
                            content: s,
                            has_escape_characters,
                        }),
                    );
                }

                // --- Numbers (Integers) ---
                b'0'..=b'9' => {
                    let mut value = (c - b'0') as i32;
                    while cursor < input.len() && input[cursor].is_ascii_digit() {
                        let digit = input[cursor] - b'0';
                        value = value * 10 + digit as i32;
                        cursor += 1;
                    }
                    return (
                        Token::IntegerLit(value),
                        cursor,
                        Some(Token::IntegerLit(value)),
                    );
                }

                // --- Identifiers & Keywords ---
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    while cursor < input.len() && Self::is_ident_char(input[cursor]) {
                        cursor += 1;
                    }

                    let text = &input[start..cursor];
                    let token = match text {
                        b"fn" => Token::Fn,
                        b"if" => Token::If,
                        b"ret" => Token::Return,
                        b"else" => Token::Else,
                        b"while" => Token::While,
                        _ => Token::Identifier(text),
                    };
                    return (token, cursor, Some(token));
                }

                _ => return (Token::Error, cursor, Some(Token::Error)),
            }
        }
    }

    fn is_ident_char(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use similar_asserts::assert_eq;
    use std::vec;

    fn tokenize_all<'a>(tok: &'a mut Tokenizer) -> Vec<Token<'a>> {
        let mut tokens = Vec::new();
        loop {
            let next = tok.advance();
            if next == Token::Eof {
                break;
            }
            tokens.push(next);
        }

        tokens
    }

    fn assert_tokenized(input: &str, expected: Vec<Token>) {
        let mut tok = Tokenizer::new(input.as_bytes());
        assert_eq!(tokenize_all(&mut tok), expected);
    }

    #[test]
    fn basic_parse() {
        assert_tokenized(
            r#"
            a = 5 + 3;
            b = a + 5
            print(5);
            "#,
            vec![
                Token::Identifier(b"a"),
                Token::Assign,
                Token::IntegerLit(5),
                Token::Plus,
                Token::IntegerLit(3),
                Token::Separator,
                Token::Identifier(b"b"),
                Token::Assign,
                Token::Identifier(b"a"),
                Token::Plus,
                Token::IntegerLit(5),
                Token::Separator,
                Token::Identifier(b"print"),
                Token::LParen,
                Token::IntegerLit(5),
                Token::RParen,
                Token::Separator,
            ],
        );
    }

    #[test]
    fn separator_at_start() {
        assert_tokenized(
            r#"
            a = 5;
            while a < 10 {
                a = a + 1;
            }
            print(a + 1);
            "#,
            vec![
                Token::Identifier(b"a"),
                Token::Assign,
                Token::IntegerLit(5),
                Token::Separator,
                Token::While,
                Token::Identifier(b"a"),
                Token::Lt,
                Token::IntegerLit(10),
                Token::LBrace,
                Token::Identifier(b"a"),
                Token::Assign,
                Token::Identifier(b"a"),
                Token::Plus,
                Token::IntegerLit(1),
                Token::Separator,
                Token::RBrace,
                Token::Separator,
                Token::Identifier(b"print"),
                Token::LParen,
                Token::Identifier(b"a"),
                Token::Plus,
                Token::IntegerLit(1),
                Token::RParen,
                Token::Separator,
            ],
        );
    }

    #[test]
    fn simple_unary() {
        assert_tokenized(
            "a = !5",
            vec![
                Token::Identifier(b"a"),
                Token::Assign,
                Token::Bang,
                Token::IntegerLit(5),
            ],
        );
    }
}
