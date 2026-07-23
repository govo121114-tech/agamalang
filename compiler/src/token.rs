use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    Fn,         // fn
    Let,        // let
    If,         // if
    Else,       // else
    While,      // while
    For,        // for
    Return,     // return
    True,       // true
    False,      // false
    Null,       // null
    Struct,     // struct
    Impl,       // impl
    Enum,       // enum
    Break,      // break
    Continue,   // continue
    SizeOf,     // sizeof
    Eat,        // eat

    // Types
    IntType,    // int
    CharType,   // char
    BoolType,   // bool
    VoidType,   // void
    UpIntType,  // upint
    UnIntType,  // unint
    FixedType,  // fixed

    // Literals
    Integer(i64),
    Fixed(i64),     // Q16.16 fixed-point
    String(String),
    CharLiteral(u8),
    Identifier(String),

    // Punctuation
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LBracket,   // [
    RBracket,   // ]
    Semicolon,  // ;
    Colon,      // :
    Comma,      // ,
    Dot,        // .
    Arrow,      // ->

    // Operators
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    Percent,    // %
    Equal,      // =
    EqualEqual, // ==
    NotEqual,   // !=
    Less,       // <
    Greater,    // >
    LessEqual,  // <=
    GreaterEqual, // >=
    And,        // &&
    Or,         // ||
    Not,        // !
    Amp,        // &

    // Special
    Newline,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub column: usize,
}

impl Token {
    pub fn new(kind: TokenKind, line: usize, column: usize) -> Self {
        Token { kind, line, column }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TokenKind::Fn => write!(f, "fn"),
            TokenKind::Let => write!(f, "let"),
            TokenKind::If => write!(f, "if"),
            TokenKind::Else => write!(f, "else"),
            TokenKind::While => write!(f, "while"),
            TokenKind::For => write!(f, "for"),
            TokenKind::Return => write!(f, "return"),
            TokenKind::True => write!(f, "true"),
            TokenKind::False => write!(f, "false"),
            TokenKind::Null => write!(f, "null"),
            TokenKind::Struct => write!(f, "struct"),
            TokenKind::Impl => write!(f, "impl"),
            TokenKind::Enum => write!(f, "enum"),
            TokenKind::Break => write!(f, "break"),
            TokenKind::Continue => write!(f, "continue"),
            TokenKind::SizeOf => write!(f, "sizeof"),
            TokenKind::Eat => write!(f, "eat"),
            TokenKind::IntType => write!(f, "int"),
            TokenKind::CharType => write!(f, "char"),
            TokenKind::BoolType => write!(f, "bool"),
            TokenKind::VoidType => write!(f, "void"),
            TokenKind::UpIntType => write!(f, "upint"),
            TokenKind::UnIntType => write!(f, "unint"),
            TokenKind::FixedType => write!(f, "fixed"),
            TokenKind::Integer(n) => write!(f, "{}", n),
            TokenKind::Fixed(n) => {
                let int_part = *n >> 16;
                let frac_part = (*n as u64 & 0xFFFF) * 100000 / 65536;
                write!(f, "{}.{:05}", int_part, frac_part)
            }
            TokenKind::String(s) => write!(f, "\"{}\"", s),
            TokenKind::CharLiteral(c) => write!(f, "'{}'", *c as char),
            TokenKind::Identifier(s) => write!(f, "{}", s),
            TokenKind::LParen => write!(f, "("),
            TokenKind::RParen => write!(f, ")"),
            TokenKind::LBrace => write!(f, "{{"),
            TokenKind::RBrace => write!(f, "}}"),
            TokenKind::LBracket => write!(f, "["),
            TokenKind::RBracket => write!(f, "]"),
            TokenKind::Semicolon => write!(f, ";"),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Comma => write!(f, ","),
            TokenKind::Dot => write!(f, "."),
            TokenKind::Arrow => write!(f, "->"),
            TokenKind::Plus => write!(f, "+"),
            TokenKind::Minus => write!(f, "-"),
            TokenKind::Star => write!(f, "*"),
            TokenKind::Slash => write!(f, "/"),
            TokenKind::Percent => write!(f, "%"),
            TokenKind::Equal => write!(f, "="),
            TokenKind::EqualEqual => write!(f, "=="),
            TokenKind::NotEqual => write!(f, "!="),
            TokenKind::Less => write!(f, "<"),
            TokenKind::Greater => write!(f, ">"),
            TokenKind::LessEqual => write!(f, "<="),
            TokenKind::GreaterEqual => write!(f, ">="),
            TokenKind::And => write!(f, "&&"),
            TokenKind::Or => write!(f, "||"),
            TokenKind::Not => write!(f, "!"),
            TokenKind::Amp => write!(f, "&"),
            TokenKind::Newline => write!(f, "\\n"),
            TokenKind::Eof => write!(f, "<eof>"),
        }
    }
}
