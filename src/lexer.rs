use crate::token::{Token, TokenKind};

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.source.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.source.get(self.pos).copied();
        if ch.is_some() {
            self.pos += 1;
            if ch == Some('\n') {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        ch
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == ' ' || ch == '\t' || ch == '\r' {
                self.advance();
            } else if ch == '/' && self.peek_next() == Some('/') {
                // Line comment
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.advance();
                }
            } else if ch == '/' && self.peek_next() == Some('*') {
                // Block comment
                self.advance(); // skip /
                self.advance(); // skip *
                while let Some(c) = self.peek() {
                    if c == '*' && self.peek_next() == Some('/') {
                        self.advance(); // skip *
                        self.advance(); // skip /
                        break;
                    }
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn read_string(&mut self) -> TokenKind {
        let line = self.line;
        let column = self.column;
        // NOTE: opening " was already consumed by next_token's advance()
        let mut s = String::new();
        while let Some(ch) = self.peek() {
            if ch == '"' {
                self.advance();
                return TokenKind::String(s);
            }
            if ch == '\\' {
                self.advance();
                match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('0') => s.push('\0'),
                    Some(c) => {
                        eprintln!("Warning: unknown escape sequence \\{}", c);
                        s.push(c);
                    }
                    None => break,
                }
            } else {
                s.push(ch);
                self.advance();
            }
        }
        eprintln!("Error: unterminated string at line {}", line);
        TokenKind::String(s)
    }

    fn read_char(&mut self) -> TokenKind {
        let line = self.line;
        // NOTE: opening ' was already consumed by next_token's advance()
        let ch = if self.peek() == Some('\\') {
            self.advance();
            match self.advance() {
                Some('n') => b'\n',
                Some('t') => b'\t',
                Some('r') => b'\r',
                Some('\\') => b'\\',
                Some('\'') => b'\'',
                Some('0') => b'\0',
                Some(c) => {
                    eprintln!("Warning: unknown escape sequence \\{}", c);
                    c as u8
                }
                None => {
                    eprintln!("Error: unterminated char literal at line {}", line);
                    0
                }
            }
        } else {
            match self.advance() {
                Some(c) => c as u8,
                None => {
                    eprintln!("Error: unterminated char literal at line {}", line);
                    0
                }
            }
        };
        if self.peek() == Some('\'') {
            self.advance();
        } else {
            eprintln!("Error: expected closing ' at line {}", line);
        }
        TokenKind::CharLiteral(ch)
    }

    fn read_number(&mut self, first: char) -> TokenKind {
        let mut s = String::new();
        s.push(first);
        let mut is_hex = false;
        if first == '0' {
            if let Some(next) = self.peek() {
                if next == 'x' || next == 'X' {
                    is_hex = true;
                    s.push('x');
                    self.advance();
                }
            }
        }
        while let Some(ch) = self.peek() {
            if is_hex {
                if ch.is_ascii_hexdigit() {
                    s.push(ch);
                    self.advance();
                } else {
                    break;
                }
            } else {
                if ch.is_ascii_digit() {
                    s.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
        }
        let n: i64 = if is_hex {
            if s.len() > 2 {
                i64::from_str_radix(&s[2..], 16).unwrap_or(0)
            } else {
                0
            }
        } else {
            s.parse().unwrap_or(0)
        };
        TokenKind::Integer(n)
    }

    fn read_identifier(&mut self, first: char) -> TokenKind {
        let mut s = String::new();
        s.push(first);
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                s.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        match s.as_str() {
            "fn" => TokenKind::Fn,
            "let" => TokenKind::Let,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "return" => TokenKind::Return,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "null" => TokenKind::Null,
            "struct" => TokenKind::Struct,
            "impl" => TokenKind::Impl,
            "enum" => TokenKind::Enum,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "sizeof" => TokenKind::SizeOf,
            "int" => TokenKind::IntType,
            "char" => TokenKind::CharType,
            "bool" => TokenKind::BoolType,
            "void" => TokenKind::VoidType,
            _ => TokenKind::Identifier(s),
        }
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace();

        let line = self.line;
        let column = self.column;

        let ch = match self.advance() {
            Some(c) => c,
            None => return Token::new(TokenKind::Eof, line, column),
        };

        let kind = match ch {
            // Single-character tokens
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ';' => TokenKind::Semicolon,
            ':' => TokenKind::Colon,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            '+' => TokenKind::Plus,
            '-' => {
                if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::NotEqual
                } else {
                    TokenKind::Not
                }
            }
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::EqualEqual
                } else {
                    TokenKind::Equal
                }
            }
            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::LessEqual
                } else {
                    TokenKind::Less
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::GreaterEqual
                } else {
                    TokenKind::Greater
                }
            }
            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    TokenKind::And
                } else {
                    TokenKind::Amp
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    TokenKind::Or
                } else {
                    eprintln!("Error: unexpected character '|' at line {}", line);
                    TokenKind::Newline
                }
            }
            '\n' => TokenKind::Newline,

            // Literals
            '"' => self.read_string(),
            '\'' => self.read_char(),

            // Numbers and identifiers
            c if c.is_ascii_digit() => self.read_number(c),
            c if c.is_alphabetic() || c == '_' => self.read_identifier(c),

            c => {
                eprintln!("Error: unexpected character '{}' at line {}", c, line);
                TokenKind::Newline
            }
        };

        Token::new(kind, line, column)
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token();
            let is_eof = matches!(token.kind, TokenKind::Eof);
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        tokens
    }
}
