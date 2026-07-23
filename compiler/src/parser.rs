use crate::ast::*;
use crate::token::{Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn expect(&mut self, kind: &TokenKind) -> &Token {
        if self.peek_kind() == kind {
            self.advance()
        } else {
            eprintln!(
                "Error: expected '{}' at line {}, got '{}'",
                kind,
                self.peek().line,
                self.peek().kind
            );
            self.advance()
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }
    }

    pub fn parse(&mut self) -> Program {
        let mut program = Program::new();

        self.skip_newlines();

        while !matches!(self.peek_kind(), TokenKind::Eof) {
            match self.peek_kind() {
                TokenKind::Fn => {
                    let func = self.parse_function();
                    program.functions.push(func);
                }
                TokenKind::Struct => {
                    let sd = self.parse_struct_def();
                    program.structs.push(sd);
                }
                TokenKind::Let => {
                    // Top-level let declarations (global constants)
                    let _ = self.parse_variable_decl();
                }
                TokenKind::Eat => {
                    self.advance(); // consume eat
                    let lib = match self.advance().kind.clone() {
                        TokenKind::Identifier(n) => n,
                        _ => {
                            eprintln!("Error: expected library name after 'eat' at line {}", self.peek().line);
                            String::new()
                        }
                    };
                    program.imports.push(lib);
                }
                _ => {
                    eprintln!(
                        "Error: unexpected token '{}' at line {}",
                        self.peek().kind,
                        self.peek().line
                    );
                    self.advance();
                }
            }
            self.skip_newlines();
        }

        program
    }

    fn parse_struct_def(&mut self) -> StructDefinition {
        self.expect(&TokenKind::Struct);
        let name = match self.advance().kind.clone() {
            TokenKind::Identifier(n) => n,
            _ => {
                eprintln!("Error: expected struct name at line {}", self.peek().line);
                String::new()
            }
        };
        self.skip_newlines();
        self.expect(&TokenKind::LBrace);
        self.skip_newlines();
        let mut fields = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace) && !matches!(self.peek_kind(), TokenKind::Eof) {
            let fname = match self.advance().kind.clone() {
                TokenKind::Identifier(n) => n,
                _ => {
                    eprintln!("Error: expected field name at line {}", self.peek().line);
                    String::new()
                }
            };
            self.expect(&TokenKind::Colon);
            let ftype = self.parse_type();
            self.expect(&TokenKind::Semicolon);
            self.skip_newlines();
            fields.push((fname, ftype));
        }
        self.expect(&TokenKind::RBrace);
        StructDefinition { name, fields }
    }

    fn parse_function(&mut self) -> Function {
        self.expect(&TokenKind::Fn); // fn
        self.skip_newlines();

        let name = match self.advance().kind.clone() {
            TokenKind::Identifier(n) => n,
            _ => {
                eprintln!("Error: expected function name at line {}", self.peek().line);
                String::new()
            }
        };
        self.skip_newlines();

        self.expect(&TokenKind::LParen);
        self.skip_newlines();

        let mut params = Vec::new();
        if !matches!(self.peek_kind(), TokenKind::RParen) {
            loop {
                let param = self.parse_param();
                params.push(param);
                self.skip_newlines();
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.advance();
                    self.skip_newlines();
                } else {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen);
        self.skip_newlines();

        let return_type = if matches!(self.peek_kind(), TokenKind::Arrow) {
            self.advance();
            self.skip_newlines();
            self.parse_type()
        } else {
            Type::Void
        };
        self.skip_newlines();

        let body = self.parse_block();

        Function { name, params, return_type, body }
    }

    fn parse_param(&mut self) -> Param {
        let name = match self.advance().kind.clone() {
            TokenKind::Identifier(n) => n,
            _ => {
                eprintln!("Error: expected parameter name at line {}", self.peek().line);
                String::new()
            }
        };
        self.expect(&TokenKind::Colon);
        let param_type = self.parse_type();
        Param { name, param_type }
    }

    fn parse_type(&mut self) -> Type {
        let base = if matches!(self.peek_kind(), TokenKind::Star) {
            self.advance();
            Type::Ptr(Box::new(self.parse_type()))
        } else {
            match self.advance().kind.clone() {
                TokenKind::IntType => Type::Int,
                TokenKind::CharType => Type::Char,
                TokenKind::BoolType => Type::Bool,
                TokenKind::VoidType => Type::Void,
                TokenKind::UpIntType => Type::UpInt,
                TokenKind::UnIntType => Type::UnInt,
                TokenKind::FixedType => Type::Fixed,
                TokenKind::Identifier(name) => Type::Named(name),
                _ => {
                    eprintln!("Error: expected type at line {}", self.peek().line);
                    Type::Int
                }
            }
        };
        // Postfix: array types [size] or []
        self.parse_array_suffix(base)
    }

    fn parse_array_suffix(&mut self, elem_type: Type) -> Type {
        let mut t = elem_type;
        while matches!(self.peek_kind(), TokenKind::LBracket) {
            self.advance();
            if matches!(self.peek_kind(), TokenKind::RBracket) {
                // [] — dynamic array
                self.advance();
                t = Type::Array(Box::new(t));
            } else {
                // [size] — static array
                let size = match self.advance().kind.clone() {
                    TokenKind::Integer(n) => n as usize,
                    _ => {
                        eprintln!("Error: expected array size at line {}", self.peek().line);
                        0
                    }
                };
                self.expect(&TokenKind::RBracket);
                t = Type::StaticArray(Box::new(t), size);
            }
        }
        t
    }

    fn parse_block(&mut self) -> Vec<Stmt> {
        self.expect(&TokenKind::LBrace);

        // Handle newlines inside block (treat as whitespace)
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }

        let mut stmts = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace) && !matches!(self.peek_kind(), TokenKind::Eof) {
            stmts.push(self.parse_stmt());

            // Skip newlines between statements
            while matches!(self.peek_kind(), TokenKind::Newline) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBrace);
        stmts
    }

    fn parse_stmt(&mut self) -> Stmt {
        match self.peek_kind() {
            TokenKind::Let => self.parse_variable_decl(),
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Return => self.parse_return(),
            TokenKind::LBrace => Stmt::Block(self.parse_block()),
            TokenKind::Break => {
                self.advance();
                self.expect(&TokenKind::Semicolon);
                Stmt::Break
            }
            TokenKind::Continue => {
                self.advance();
                self.expect(&TokenKind::Semicolon);
                Stmt::Continue
            }
            _ => {
                let expr = self.parse_expr();
                self.expect(&TokenKind::Semicolon);
                Stmt::Expr(expr)
            }
        }
    }

    fn parse_variable_decl(&mut self) -> Stmt {
        self.expect(&TokenKind::Let); // let
        let name = match self.advance().kind.clone() {
            TokenKind::Identifier(n) => n,
            _ => {
                eprintln!("Error: expected variable name at line {}", self.peek().line);
                String::new()
            }
        };

        let var_type = if matches!(self.peek_kind(), TokenKind::Colon) {
            self.advance();
            Some(self.parse_type())
        } else {
            None
        };

        let init = if matches!(self.peek_kind(), TokenKind::Equal) {
            self.advance();
            Some(self.parse_expr())
        } else {
            None
        };

        self.expect(&TokenKind::Semicolon);
        Stmt::VariableDecl { name, var_type, init }
    }

    fn parse_if(&mut self) -> Stmt {
        self.expect(&TokenKind::If);
        self.expect(&TokenKind::LParen);
        let condition = self.parse_expr();
        self.expect(&TokenKind::RParen);

        // Skip newlines after condition
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }

        let then_branch = self.parse_block();

        // Skip newlines after then block
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }

        let else_branch = if matches!(self.peek_kind(), TokenKind::Else) {
            self.advance();
            // Skip newlines after else
            while matches!(self.peek_kind(), TokenKind::Newline) {
                self.advance();
            }
            if matches!(self.peek_kind(), TokenKind::If) {
                // else if — preserve entire chain including its own else_branch
                Some(vec![self.parse_if()])
            } else {
                Some(self.parse_block())
            }
        } else {
            None
        };

        Stmt::If { condition, then_branch, else_branch }
    }

    fn parse_while(&mut self) -> Stmt {
        self.expect(&TokenKind::While);
        self.expect(&TokenKind::LParen);
        let condition = self.parse_expr();
        self.expect(&TokenKind::RParen);

        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }

        let body = self.parse_block();
        Stmt::While { condition, body }
    }

    fn parse_for(&mut self) -> Stmt {
        self.expect(&TokenKind::For);
        self.expect(&TokenKind::LParen);

        let init = if matches!(self.peek_kind(), TokenKind::Let) {
            self.parse_variable_decl()
        } else {
            let expr = self.parse_expr();
            self.expect(&TokenKind::Semicolon);
            Stmt::Expr(expr)
        };

        let condition = if !matches!(self.peek_kind(), TokenKind::Semicolon) {
            Some(self.parse_expr())
        } else {
            None
        };
        self.expect(&TokenKind::Semicolon);

        let post = if !matches!(self.peek_kind(), TokenKind::RParen) {
            Some(self.parse_expr())
        } else {
            None
        };
        self.expect(&TokenKind::RParen);

        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }

        let body = self.parse_block();
        Stmt::For { init: Box::new(init), condition, post, body }
    }

    fn parse_return(&mut self) -> Stmt {
        self.expect(&TokenKind::Return);
        if matches!(self.peek_kind(), TokenKind::Semicolon) {
            self.advance();
            Stmt::Return { value: None }
        } else {
            let value = self.parse_expr();
            self.expect(&TokenKind::Semicolon);
            Stmt::Return { value: Some(value) }
        }
    }

    // Expression parsing (precedence climbing)
    fn parse_expr(&mut self) -> Expr {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Expr {
        let mut expr = self.parse_logical_or();
        if matches!(self.peek_kind(), TokenKind::Equal) {
            self.advance();
            let value = self.parse_assignment();
            expr = Expr::Assign {
                target: Box::new(expr),
                value: Box::new(value),
            };
        }
        expr
    }

    fn parse_logical_or(&mut self) -> Expr {
        let mut expr = self.parse_logical_and();
        while matches!(self.peek_kind(), TokenKind::Or) {
            self.advance();
            let right = self.parse_logical_and();
            expr = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        expr
    }

    fn parse_logical_and(&mut self) -> Expr {
        let mut expr = self.parse_equality();
        while matches!(self.peek_kind(), TokenKind::And) {
            self.advance();
            let right = self.parse_equality();
            expr = Expr::Binary {
                op: BinOp::And,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        expr
    }

    fn parse_equality(&mut self) -> Expr {
        let mut expr = self.parse_comparison();
        loop {
            match self.peek_kind() {
                TokenKind::EqualEqual => {
                    self.advance();
                    let right = self.parse_comparison();
                    expr = Expr::Binary {
                        op: BinOp::Equal,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                TokenKind::NotEqual => {
                    self.advance();
                    let right = self.parse_comparison();
                    expr = Expr::Binary {
                        op: BinOp::NotEqual,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        expr
    }

    fn parse_comparison(&mut self) -> Expr {
        let mut expr = self.parse_term();
        loop {
            match self.peek_kind() {
                TokenKind::Less => {
                    self.advance();
                    let right = self.parse_term();
                    expr = Expr::Binary {
                        op: BinOp::Less,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                TokenKind::Greater => {
                    self.advance();
                    let right = self.parse_term();
                    expr = Expr::Binary {
                        op: BinOp::Greater,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                TokenKind::LessEqual => {
                    self.advance();
                    let right = self.parse_term();
                    expr = Expr::Binary {
                        op: BinOp::LessEqual,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                TokenKind::GreaterEqual => {
                    self.advance();
                    let right = self.parse_term();
                    expr = Expr::Binary {
                        op: BinOp::GreaterEqual,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        expr
    }

    fn parse_term(&mut self) -> Expr {
        let mut expr = self.parse_factor();
        loop {
            match self.peek_kind() {
                TokenKind::Plus => {
                    self.advance();
                    let right = self.parse_factor();
                    expr = Expr::Binary {
                        op: BinOp::Add,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                TokenKind::Minus => {
                    self.advance();
                    let right = self.parse_factor();
                    expr = Expr::Binary {
                        op: BinOp::Sub,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        expr
    }

    fn parse_factor(&mut self) -> Expr {
        let mut expr = self.parse_unary();
        loop {
            match self.peek_kind() {
                TokenKind::Star => {
                    self.advance();
                    let right = self.parse_unary();
                    expr = Expr::Binary {
                        op: BinOp::Mul,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                TokenKind::Slash => {
                    self.advance();
                    let right = self.parse_unary();
                    expr = Expr::Binary {
                        op: BinOp::Div,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                TokenKind::Percent => {
                    self.advance();
                    let right = self.parse_unary();
                    expr = Expr::Binary {
                        op: BinOp::Mod,
                        left: Box::new(expr),
                        right: Box::new(right),
                    };
                }
                _ => break,
            }
        }
        expr
    }

    fn parse_unary(&mut self) -> Expr {
        match self.peek_kind() {
            TokenKind::Minus => {
                self.advance();
                let operand = self.parse_unary();
                Expr::Unary {
                    op: UnOp::Negate,
                    operand: Box::new(operand),
                }
            }
            TokenKind::Not => {
                self.advance();
                let operand = self.parse_unary();
                Expr::Unary {
                    op: UnOp::Not,
                    operand: Box::new(operand),
                }
            }
            TokenKind::Star => {
                self.advance();
                let operand = self.parse_unary();
                Expr::Unary {
                    op: UnOp::Deref,
                    operand: Box::new(operand),
                }
            }
            TokenKind::Amp => {
                self.advance();
                let operand = self.parse_unary();
                Expr::Unary {
                    op: UnOp::AddrOf,
                    operand: Box::new(operand),
                }
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Expr {
        let tok = self.advance().clone();
        let mut expr = match tok.kind {
            TokenKind::Integer(n) => Expr::Integer(n),
            TokenKind::Fixed(n) => Expr::Fixed(n),
            TokenKind::String(s) => Expr::String(s),
            TokenKind::CharLiteral(c) => Expr::Char(c),
            TokenKind::True => Expr::Bool(true),
            TokenKind::False => Expr::Bool(false),
            TokenKind::Null => Expr::Null,
            TokenKind::SizeOf => {
                self.expect(&TokenKind::LParen);
                let ty = self.parse_type();
                self.expect(&TokenKind::RParen);
                Expr::SizeOf(ty)
            }
            TokenKind::LBracket => {
                let mut elems = Vec::new();
                if !matches!(self.peek_kind(), TokenKind::RBracket) {
                    loop {
                        elems.push(self.parse_expr());
                        if matches!(self.peek_kind(), TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RBracket);
                Expr::ArrayInit(elems)
            }
            TokenKind::Identifier(name) => {
                if matches!(self.peek_kind(), TokenKind::LBrace) {
                    return self.parse_struct_init(name);
                }
                if matches!(self.peek_kind(), TokenKind::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    if !matches!(self.peek_kind(), TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr());
                            if matches!(self.peek_kind(), TokenKind::Comma) {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(&TokenKind::RParen);
                    return Expr::Call { callee: name, args };
                }
                Expr::Identifier(name)
            }
            TokenKind::LParen => {
                let e = self.parse_expr();
                self.expect(&TokenKind::RParen);
                e
            }
            _ => {
                eprintln!(
                    "Error: unexpected token '{}' at line {}",
                    tok.kind, tok.line
                );
                Expr::Integer(0)
            }
        };
        // Chain postfix ops: .field and [index] for any expression
        loop {
            if matches!(self.peek_kind(), TokenKind::Dot) {
                self.advance();
                let member = match self.advance().kind.clone() {
                    TokenKind::Identifier(m) => m,
                    _ => {
                        eprintln!("Error: expected member name at line {}", self.peek().line);
                        String::new()
                    }
                };
                expr = Expr::Member { object: Box::new(expr), member };
            } else if matches!(self.peek_kind(), TokenKind::LBracket) {
                self.advance();
                let index = self.parse_expr();
                self.expect(&TokenKind::RBracket);
                expr = Expr::Index { object: Box::new(expr), index: Box::new(index) };
            } else {
                break;
            }
        }
        expr
    }

    fn parse_struct_init(&mut self, type_name: String) -> Expr {
        // type_name is already consumed, now we see LBrace
        self.expect(&TokenKind::LBrace);
        self.skip_newlines();
        let mut fields = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace) && !matches!(self.peek_kind(), TokenKind::Eof) {
            let fname = match self.advance().kind.clone() {
                TokenKind::Identifier(n) => n,
                _ => {
                    eprintln!("Error: expected field name at line {}", self.peek().line);
                    String::new()
                }
            };
            self.expect(&TokenKind::Colon);
            let fvalue = self.parse_expr();
            // Allow either ; or , as separator
            if matches!(self.peek_kind(), TokenKind::Semicolon) || matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
            fields.push((fname, fvalue));
        }
        self.expect(&TokenKind::RBrace);
        Expr::StructInit { type_name, fields }
    }
}
