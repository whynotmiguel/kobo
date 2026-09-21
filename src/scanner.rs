use crate::token::{keyword, Token, TokenType};

/// Cut `source` into tokens. Returns everything it managed to scan alongside every
/// error it found; the caller decides whether to go on.
pub fn scan(source: &str) -> (Vec<Token>, Vec<String>) {
    let mut s = Scanner {
        src: source.chars().collect(),
        start: 0,
        current: 0,
        line: 1,
        last_token_line: 1,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    s.run();
    (s.tokens, s.errors)
}

struct Scanner {
    src: Vec<char>,
    start: usize,
    current: usize,
    line: usize,
    last_token_line: usize,
    tokens: Vec<Token>,
    errors: Vec<String>,
}

impl Scanner {
    fn run(&mut self) {
        while !self.at_end() {
            self.skip_whitespace();
            if self.at_end() {
                break;
            }
            self.start = self.current;
            self.scan_token();
        }
        self.start = self.current;
        self.add_eof();
    }

    fn scan_token(&mut self) {
        let c = self.advance();
        match c {
            '(' => self.add(TokenType::LParen),
            ')' => self.add(TokenType::RParen),
            '{' => self.add(TokenType::LBrace),
            '}' => self.add(TokenType::RBrace),
            ',' => self.add(TokenType::Comma),
            ';' => self.add(TokenType::Semicolon),
            '+' => self.add(TokenType::Plus),
            '-' => self.add(TokenType::Minus),
            '*' => self.add(TokenType::Star),
            '/' => {
                if self.matches('/') {
                    // Comment goes until end of line.
                    while self.peek() != '\n' && !self.at_end() {
                        self.advance();
                    }
                } else {
                    self.add(TokenType::Slash);
                }
            }
            '!' => {
                if self.matches('=') {
                    self.add(TokenType::BangEqual);
                } else {
                    self.add(TokenType::Bang);
                }
            }
            '=' => {
                if self.matches('=') {
                    self.add(TokenType::EqualEqual);
                } else {
                    self.add(TokenType::Equal);
                }
            }
            '<' => {
                if self.matches('=') {
                    self.add(TokenType::LessEqual);
                } else {
                    self.add(TokenType::Less);
                }
            }
            '>' => {
                if self.matches('=') {
                    self.add(TokenType::GreaterEqual);
                } else {
                    self.add(TokenType::Greater);
                }
            }
            '"' => self.string(),
            _ => {
                if c.is_ascii_digit() {
                    self.number();
                } else if is_alpha(c) {
                    self.identifier();
                } else if !c.is_ascii_whitespace() {
                    self.error(self.line, "Character is not part of any token.");
                }
                // Whitespace handled by skip_whitespace at start of loop.
            }
        }
    }

    fn string(&mut self) {
        while self.peek() != '"' && !self.at_end() {
            if self.peek() == '\n' {
                self.line += 1;
            }
            self.advance();
        }

        if self.at_end() {
            // Unterminated string.
            self.error(self.line, "String is never closed.");
            return;
        }

        // Consume the closing quote.
        self.advance();
        self.add(TokenType::Str);
    }

    fn number(&mut self) {
        while self.peek().is_ascii_digit() {
            self.advance();
        }

        // Look for a fractional part.
        if self.peek() == '.' && self.peek_next().is_ascii_digit() {
            // Consume the "."
            self.advance();

            while self.peek().is_ascii_digit() {
                self.advance();
            }
        }

        self.add(TokenType::Number);
    }

    fn identifier(&mut self) {
        while is_alpha_numeric(self.peek()) {
            self.advance();
        }

        let text: String = self.src[self.start..self.current].iter().collect();
        if let Some(kind) = keyword(&text) {
            self.add(kind);
        } else {
            self.add(TokenType::Identifier);
        }
    }

    fn add(&mut self, kind: TokenType) {
        self.tokens.push(Token {
            kind,
            lexeme: self.src[self.start..self.current].iter().collect(),
            line: self.line,
        });
        self.last_token_line = self.line;
    }

    fn add_eof(&mut self) {
        self.tokens.push(Token {
            kind: TokenType::Eof,
            lexeme: String::new(),
            line: self.last_token_line,
        });
    }

    fn error(&mut self, line: usize, message: &str) {
        self.errors.push(format!("[line {}] Error: {}", line, message));
    }

    // --- primitives ---------------------------------------------------------------

    fn at_end(&self) -> bool {
        self.current >= self.src.len()
    }

    fn advance(&mut self) -> char {
        let c = self.src[self.current];
        self.current += 1;
        c
    }

    fn matches(&mut self, expected: char) -> bool {
        if self.at_end() || self.src[self.current] != expected {
            return false;
        }
        self.current += 1;
        true
    }

    fn peek(&self) -> char {
        if self.at_end() {
            '\0'
        } else {
            self.src[self.current]
        }
    }

    fn peek_next(&self) -> char {
        if self.current + 1 >= self.src.len() {
            '\0'
        } else {
            self.src[self.current + 1]
        }
    }

    fn skip_whitespace(&mut self) {
        loop {
            match self.peek() {
                ' ' | '\r' | '\t' => {
                    self.advance();
                }
                '\n' => {
                    self.line += 1;
                    self.advance();
                }
                _ => { break; }
            }
        }
    }
}

fn is_alpha(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_alpha_numeric(c: char) -> bool {
    is_alpha(c) || c.is_ascii_digit()
}