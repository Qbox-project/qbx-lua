use smol_str::SmolStr;

use crate::lexer::{Comment, NumberValue, Token};
use crate::span::Span;
use crate::SyntaxError;

#[derive(Debug)]
pub struct Chunk {
    pub block: Block,
    pub comments: Vec<Comment>,
    pub tokens: Vec<Token>,
    pub errors: Vec<SyntaxError>,
}

#[derive(Debug, Default)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub text: SmolStr,
    pub span: Span,
}

impl Name {
    pub fn is_missing(&self) -> bool {
        self.text.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attrib {
    Const,
    Close,
    Unknown,
}

#[derive(Debug)]
pub struct AttribName {
    pub name: Name,
    pub attrib: Option<(Attrib, Span)>,
}

#[derive(Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum StmtKind {
    Local {
        names: Vec<AttribName>,
        exprs: Vec<Expr>,
        in_unpack: bool,
    },
    LocalFunction {
        name: Name,
        func: Box<FuncBody>,
    },
    Function {
        name: FuncName,
        func: Box<FuncBody>,
    },
    Assign {
        targets: Vec<Expr>,
        exprs: Vec<Expr>,
    },
    CompoundAssign {
        target: Expr,
        op: BinOp,
        op_span: Span,
        expr: Expr,
    },
    /// An expression in statement position. Anything other than a call is also reported as a syntax error.
    Expr(Expr),
    Do(Block),
    While {
        cond: Expr,
        body: Block,
    },
    Repeat {
        body: Block,
        cond: Expr,
    },
    If {
        branches: Vec<IfBranch>,
        else_block: Option<Block>,
    },
    NumericFor {
        var: Name,
        start: Expr,
        limit: Expr,
        step: Option<Expr>,
        body: Block,
    },
    GenericFor {
        names: Vec<Name>,
        exprs: Vec<Expr>,
        body: Block,
    },
    Return(Vec<Expr>),
    Break,
    Goto(Name),
    Label(Name),
    Defer(Block),
    Error,
}

#[derive(Debug)]
pub struct IfBranch {
    pub cond: Expr,
    pub block: Block,
    pub keyword_span: Span,
}

#[derive(Debug)]
pub struct FuncName {
    pub base: Name,
    pub path: Vec<Name>,
    pub method: Option<Name>,
    pub span: Span,
}

#[derive(Debug)]
pub struct FuncBody {
    pub params: Vec<Name>,
    pub vararg: Option<Span>,
    pub params_span: Span,
    pub body: Block,
    pub span: Span,
    pub end_span: Span,
}

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStyle {
    Paren,
    String,
    Table,
}

#[derive(Debug)]
pub enum ExprKind {
    Nil,
    True,
    False,
    Vararg,
    Number(NumberValue),
    String(SmolStr),
    JenkinsHash(SmolStr),
    Function(Box<FuncBody>),
    Name(Name),
    Index { base: Box<Expr>, index: Box<Expr>, safe: bool },
    Field { base: Box<Expr>, name: Name, safe: bool },
    Call { callee: Box<Expr>, args: Vec<Expr>, args_span: Span, style: CallStyle },
    MethodCall { base: Box<Expr>, method: Name, args: Vec<Expr>, args_span: Span, style: CallStyle, safe: bool },
    Binary { op: BinOp, op_span: Span, lhs: Box<Expr>, rhs: Box<Expr> },
    Unary { op: UnOp, expr: Box<Expr> },
    Paren(Box<Expr>),
    Table(Vec<TableField>),
    Error,
}

impl Expr {
    pub fn is_call(&self) -> bool {
        matches!(self.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. })
    }

    pub fn is_multi_value(&self) -> bool {
        self.is_call() || matches!(self.kind, ExprKind::Vararg)
    }

    pub fn unparen(&self) -> &Expr {
        let mut expr = self;
        while let ExprKind::Paren(inner) = &expr.kind {
            expr = inner;
        }
        expr
    }

    pub fn as_string(&self) -> Option<&SmolStr> {
        match &self.kind {
            ExprKind::String(s) => Some(s),
            _ => None,
        }
    }

    /// Dotted path for plain `a.b.c` chains (string indexes included), e.g. `Citizen.Wait`.
    pub fn dotted_path(&self) -> Option<String> {
        let mut path = String::new();
        self.write_dotted_path(&mut path).then_some(path)
    }

    fn write_dotted_path(&self, out: &mut String) -> bool {
        let (base, key) = match &self.kind {
            ExprKind::Name(name) => {
                out.push_str(&name.text);
                return true;
            }
            ExprKind::Field { base, name, .. } => (base, &name.text),
            ExprKind::Index { base, index, .. } => match index.as_string() {
                Some(key) => (base, key),
                None => return false,
            },
            _ => return false,
        };
        if !base.write_dotted_path(out) {
            return false;
        }
        out.push('.');
        out.push_str(key);
        true
    }
}

#[derive(Debug)]
pub enum TableField {
    Positional(Expr),
    Named { name: Name, value: Expr },
    Keyed { key: Expr, value: Expr },
    SetMember(Name),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::IDiv => "//",
            BinOp::Mod => "%",
            BinOp::Pow => "^",
            BinOp::Concat => "..",
            BinOp::Eq => "==",
            BinOp::Ne => "~=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::BAnd => "&",
            BinOp::BOr => "|",
            BinOp::BXor => "~",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
    Len,
    BNot,
}
