use std::fmt;

#[derive(Debug, Clone)]
pub enum Type {
    Int,
    Char,
    Bool,
    Void,
    UpInt,
    UnInt,
    Fixed,
    Ptr(Box<Type>),
    Array(Box<Type>),            // int[] — dynamic array
    StaticArray(Box<Type>, usize), // int[10] — static array (fixed size)
    Named(String),               // user-defined type (struct)
}

#[derive(Debug, Clone)]
pub enum Stmt {
    VariableDecl {
        name: String,
        var_type: Option<Type>,
        init: Option<Expr>,
    },
    If {
        condition: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    For {
        init: Box<Stmt>,
        condition: Option<Expr>,
        post: Option<Expr>,
        body: Vec<Stmt>,
    },
    Return {
        value: Option<Expr>,
    },
    Expr(Expr),
    Block(Vec<Stmt>),
    Break,
    Continue,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Integer(i64),
    Fixed(i64),  // Q16.16 fixed-point
    String(String),
    Char(u8),
    Bool(bool),
    Null,
    Identifier(String),
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnOp,
        operand: Box<Expr>,
    },
    Call {
        callee: String,
        args: Vec<Expr>,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    Member {
        object: Box<Expr>,
        member: String,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    ArrayInit(Vec<Expr>),
    StructInit {
        type_name: String,
        fields: Vec<(String, Expr)>,
    },
    SizeOf(Type),
}

#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Equal,
    NotEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
    And,
    Or,
}

#[derive(Debug, Clone, Copy)]
pub enum UnOp {
    Negate,
    Not,
    Deref,
    AddrOf,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub param_type: Type,
}

#[derive(Debug, Clone)]
pub struct StructDefinition {
    pub name: String,
    pub fields: Vec<(String, Type)>,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub functions: Vec<Function>,
    pub structs: Vec<StructDefinition>,
    pub imports: Vec<String>,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Type::Int => write!(f, "int"),
            Type::Char => write!(f, "char"),
            Type::Bool => write!(f, "bool"),
            Type::Void => write!(f, "void"),
            Type::UpInt => write!(f, "upint"),
            Type::UnInt => write!(f, "unint"),
            Type::Fixed => write!(f, "fixed"),
            Type::Ptr(t) => write!(f, "*{}", t),
            Type::Array(t) => write!(f, "[]{}", t),
            Type::StaticArray(t, n) => write!(f, "[{}]{}", n, t),
            Type::Named(s) => write!(f, "{}", s),
        }
    }
}

impl Program {
    pub fn new() -> Self {
        Program { functions: Vec::new(), structs: Vec::new(), imports: Vec::new() }
    }
}
