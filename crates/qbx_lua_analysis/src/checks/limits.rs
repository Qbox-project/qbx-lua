use qbx_lua_syntax::ast::*;
use qbx_lua_syntax::{LineIndex, NumberValue, SmolStr, Span};

use super::{FileInput, Sink};
use crate::rules;

// MAXVARS in lparser.c and MAXUPVAL in lfunc.h of CfxLua 5.4.8.
const MAX_VARS: usize = 200;
const MAX_UPVALUES: usize = 255;
const NUMERIC_FOR_HIDDEN: usize = 3;
const GENERIC_FOR_HIDDEN: usize = 4;
const ENV: &str = "_ENV";

pub(super) fn check(input: &FileInput, sink: &mut Sink) {
    if sink.enabled(rules::TOO_MANY_LOCALS).is_none() && sink.enabled(rules::TOO_MANY_UPVALUES).is_none() {
        return;
    }
    let mut compiler = Compiler::default();
    compiler.open(Owner::Main, vec![SmolStr::new_static(ENV)]);
    compiler.stmts(&input.chunk.block);
    compiler.close();
    if compiler.overflows.is_empty() {
        return;
    }
    let lines = LineIndex::new(input.source);
    for overflow in compiler.overflows {
        let owner = match overflow.owner {
            Owner::Main => "the main chunk".to_string(),
            Owner::Named(name) => format!("function '{name}'"),
            Owner::Anonymous(start) => format!("function at line {}", lines.line_of(start) + 1),
        };
        let (code, what, limit) = match overflow.kind {
            Limit::Locals => (rules::TOO_MANY_LOCALS, "local variables active at once", MAX_VARS),
            Limit::Upvalues => (rules::TOO_MANY_UPVALUES, "upvalues", MAX_UPVALUES),
        };
        let message = format!("{owner} has {} {what}, more than the {limit} that Lua allows", overflow.count);
        sink.report(code, overflow.span, message);
    }
}

#[derive(Clone)]
enum Owner {
    Main,
    Named(String),
    Anonymous(u32),
}

#[derive(Clone, Copy)]
enum Limit {
    Locals,
    Upvalues,
}

struct Overflow {
    owner: Owner,
    kind: Limit,
    span: Span,
    count: usize,
}

#[derive(Clone, Copy)]
enum Const {
    Nil,
    False,
    True,
    Str,
    Int(i64),
    Float(f64),
}

impl Const {
    fn is_truthy(self) -> bool {
        !matches!(self, Const::Nil | Const::False)
    }
}

struct Var {
    name: SmolStr,
    value: Option<Const>,
}

struct Func {
    owner: Owner,
    first: usize,
    active: usize,
    upvalues: Vec<SmolStr>,
    peak: usize,
    locals_overflow: Option<Span>,
    upvalues_overflow: Option<Span>,
}

enum Found {
    Var,
    Const(Const),
    Upvalue,
    Global,
}

#[derive(Default)]
struct Compiler {
    /// `dyd->actvar`: a variable takes its slot when declared, before the values of its statement.
    vars: Vec<Var>,
    funcs: Vec<Func>,
    overflows: Vec<Overflow>,
}

impl Compiler {
    fn func(&mut self) -> &mut Func {
        self.funcs.last_mut().expect("the main chunk stays open")
    }

    fn open(&mut self, owner: Owner, upvalues: Vec<SmolStr>) {
        let first = self.vars.len();
        self.funcs.push(Func {
            owner,
            first,
            active: 0,
            upvalues,
            peak: 0,
            locals_overflow: None,
            upvalues_overflow: None,
        });
    }

    fn close(&mut self) {
        let func = self.funcs.pop().expect("open and close are paired");
        self.vars.truncate(func.first);
        if let Some(span) = func.locals_overflow {
            let owner = func.owner.clone();
            self.overflows.push(Overflow { owner, kind: Limit::Locals, span, count: func.peak });
        }
        if let Some(span) = func.upvalues_overflow {
            let count = func.upvalues.len();
            self.overflows.push(Overflow { owner: func.owner, kind: Limit::Upvalues, span, count });
        }
    }

    fn declare(&mut self, name: &str, span: Span) {
        let count = self.vars.len() + 1 - self.func().first;
        let func = self.func();
        func.peak = func.peak.max(count);
        if count > MAX_VARS && func.locals_overflow.is_none() {
            func.locals_overflow = Some(span);
        }
        self.vars.push(Var { name: SmolStr::new(name), value: None });
    }

    fn activate(&mut self, count: usize) {
        self.func().active += count;
    }

    fn leave(&mut self, level: usize) {
        let func = self.func();
        func.active = level;
        let end = func.first + level;
        self.vars.truncate(end);
    }

    fn block(&mut self, block: &Block) {
        let level = self.func().active;
        self.stmts(block);
        self.leave(level);
    }

    fn stmts(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
    }

    fn function(&mut self, func: &FuncBody, owner: Owner, method: bool) {
        self.open(owner, Vec::new());
        if method {
            self.declare("self", Span::empty(func.params_span.start));
            self.activate(1);
        }
        for param in &func.params {
            self.declare(&param.text, param.span);
        }
        self.activate(func.params.len());
        self.stmts(&func.body);
        self.close();
    }

    /// `singlevaraux`: a compile-time constant of an enclosing function is no upvalue.
    fn find(&mut self, level: usize, name: &str, span: Span) -> Found {
        let func = &self.funcs[level];
        let active = &self.vars[func.first..func.first + func.active];
        if let Some(var) = active.iter().rev().find(|var| var.name == name) {
            return var.value.map_or(Found::Var, Found::Const);
        }
        if func.upvalues.iter().any(|upvalue| upvalue == name) {
            return Found::Upvalue;
        }
        if level == 0 {
            return Found::Global;
        }
        match self.find(level - 1, name, span) {
            Found::Var | Found::Upvalue => {
                let func = &mut self.funcs[level];
                if func.upvalues.len() + 1 > MAX_UPVALUES && func.upvalues_overflow.is_none() {
                    func.upvalues_overflow = Some(span);
                }
                func.upvalues.push(SmolStr::new(name));
                Found::Upvalue
            }
            other => other,
        }
    }

    fn name(&mut self, name: &Name) -> Option<Const> {
        if name.is_missing() {
            return None;
        }
        let level = self.funcs.len() - 1;
        match self.find(level, &name.text, name.span) {
            Found::Const(value) => Some(value),
            Found::Global => {
                self.find(level, ENV, name.span);
                None
            }
            Found::Var | Found::Upvalue => None,
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Local { names, exprs, in_unpack } => self.local(names, exprs, *in_unpack),
            StmtKind::LocalFunction { name, func } => {
                self.declare(&name.text, name.span);
                self.activate(1);
                self.function(func, Owner::Named(name.text.to_string()), false);
            }
            StmtKind::Function { name, func } => {
                self.name(&name.base);
                let mut path = name.base.text.to_string();
                for field in &name.path {
                    path.push('.');
                    path.push_str(&field.text);
                }
                if let Some(method) = &name.method {
                    path.push(':');
                    path.push_str(&method.text);
                }
                self.function(func, Owner::Named(path), name.method.is_some());
            }
            StmtKind::Assign { targets, exprs } => {
                for target in targets {
                    self.expr(target);
                }
                for (i, expr) in exprs.iter().enumerate() {
                    let owner = targets.get(i).and_then(Expr::dotted_path);
                    self.value(expr, owner);
                }
            }
            StmtKind::CompoundAssign { target, expr, .. } => {
                self.expr(target);
                self.expr(expr);
            }
            StmtKind::Expr(expr) => {
                self.expr(expr);
            }
            StmtKind::Do(body) | StmtKind::Defer(body) => self.block(body),
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            StmtKind::Repeat { body, cond } => {
                let level = self.func().active;
                self.stmts(body);
                self.expr(cond);
                self.leave(level);
            }
            StmtKind::If { branches, else_block } => {
                for branch in branches {
                    self.expr(&branch.cond);
                    self.block(&branch.block);
                }
                if let Some(block) = else_block {
                    self.block(block);
                }
            }
            StmtKind::NumericFor { var, start, limit, step, body } => {
                let level = self.func().active;
                for _ in 0..NUMERIC_FOR_HIDDEN {
                    self.declare("(for state)", var.span);
                }
                self.declare(&var.text, var.span);
                self.expr(start);
                self.expr(limit);
                if let Some(step) = step {
                    self.expr(step);
                }
                self.activate(NUMERIC_FOR_HIDDEN + 1);
                self.block(body);
                self.leave(level);
            }
            StmtKind::GenericFor { names, exprs, body } => {
                let level = self.func().active;
                let first = names.first().map_or(stmt.span, |name| name.span);
                for _ in 0..GENERIC_FOR_HIDDEN {
                    self.declare("(for state)", first);
                }
                for name in names {
                    self.declare(&name.text, name.span);
                }
                for expr in exprs {
                    self.expr(expr);
                }
                self.activate(GENERIC_FOR_HIDDEN + names.len());
                self.block(body);
                self.leave(level);
            }
            StmtKind::Return(exprs) => {
                for expr in exprs {
                    self.expr(expr);
                }
            }
            StmtKind::Break | StmtKind::Goto(_) | StmtKind::Label(_) | StmtKind::Error => {}
        }
    }

    /// `localstat`: a constant `<const>` value of the last name makes it a compile-time constant.
    fn local(&mut self, names: &[AttribName], exprs: &[Expr], in_unpack: bool) {
        for name in names {
            self.declare(&name.name.text, name.name.span);
        }
        if in_unpack {
            for expr in exprs {
                self.expr(expr);
            }
            self.activate(names.len());
            let span = exprs.first().map_or(Span::default(), |expr| expr.span);
            let level = self.func().active;
            self.declare("(in)", span);
            self.leave(level);
            return;
        }
        let mut last = None;
        for (i, expr) in exprs.iter().enumerate() {
            let owner = names.get(i).map(|name| name.name.text.to_string());
            last = self.value(expr, owner);
        }
        let is_const = names.last().is_some_and(|name| matches!(name.attrib, Some((Attrib::Const, _))));
        if is_const && names.len() == exprs.len() {
            if let (Some(value), Some(var)) = (last, self.vars.last_mut()) {
                var.value = Some(value);
            }
        }
        self.activate(names.len());
    }

    fn value(&mut self, expr: &Expr, owner: Option<String>) -> Option<Const> {
        match (&expr.kind, owner) {
            (ExprKind::Function(func), Some(owner)) => {
                self.function(func, Owner::Named(owner), false);
                None
            }
            _ => self.expr(expr),
        }
    }

    fn expr(&mut self, expr: &Expr) -> Option<Const> {
        match &expr.kind {
            ExprKind::Nil => Some(Const::Nil),
            ExprKind::True => Some(Const::True),
            ExprKind::False => Some(Const::False),
            ExprKind::String(_) => Some(Const::Str),
            ExprKind::Number(NumberValue::Int(value)) => Some(Const::Int(*value)),
            ExprKind::Number(NumberValue::Float(value)) => Some(Const::Float(*value)),
            ExprKind::JenkinsHash(text) => Some(Const::Int(joaat(text))),
            ExprKind::Name(name) => self.name(name),
            ExprKind::Paren(inner) => self.expr(inner),
            ExprKind::Function(func) => {
                self.function(func, Owner::Anonymous(func.span.start), false);
                None
            }
            ExprKind::Index { base, index, .. } => {
                self.expr(base);
                self.expr(index);
                None
            }
            ExprKind::Field { base, .. } => {
                self.expr(base);
                None
            }
            ExprKind::Call { callee, args, .. } => {
                self.expr(callee);
                for arg in args {
                    self.expr(arg);
                }
                None
            }
            ExprKind::MethodCall { base, args, .. } => {
                self.expr(base);
                for arg in args {
                    self.expr(arg);
                }
                None
            }
            ExprKind::Binary { op, lhs, rhs, .. } => {
                let lhs = self.expr(lhs);
                let rhs = self.expr(rhs);
                fold_binary(*op, lhs?, rhs?)
            }
            ExprKind::Unary { op, expr } => {
                let value = self.expr(expr)?;
                match op {
                    UnOp::Not if value.is_truthy() => Some(Const::False),
                    UnOp::Not => Some(Const::True),
                    UnOp::Neg | UnOp::BNot => fold_arith(Arith::from_unary(*op)?, value, Const::Int(0)),
                    UnOp::Len => None,
                }
            }
            ExprKind::Table(fields) => {
                for field in fields {
                    match field {
                        TableField::Positional(value) | TableField::Named { value, .. } => {
                            self.expr(value);
                        }
                        TableField::Keyed { key, value } => {
                            self.expr(key);
                            self.expr(value);
                        }
                        TableField::SetMember(_) => {}
                    }
                }
                None
            }
            ExprKind::Vararg | ExprKind::Error => None,
        }
    }
}

/// `and` and `or` stay constant only when the left side emits no jump (`luaK_goiftrue`).
fn fold_binary(op: BinOp, lhs: Const, rhs: Const) -> Option<Const> {
    match op {
        BinOp::And => matches!(lhs, Const::Str | Const::Int(_) | Const::Float(_) | Const::True).then_some(rhs),
        BinOp::Or => matches!(lhs, Const::Nil | Const::False).then_some(rhs),
        _ => fold_arith(Arith::from_binary(op)?, lhs, rhs),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
    Unm,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    BNot,
}

impl Arith {
    fn from_binary(op: BinOp) -> Option<Arith> {
        Some(match op {
            BinOp::Add => Arith::Add,
            BinOp::Sub => Arith::Sub,
            BinOp::Mul => Arith::Mul,
            BinOp::Div => Arith::Div,
            BinOp::IDiv => Arith::IDiv,
            BinOp::Mod => Arith::Mod,
            BinOp::Pow => Arith::Pow,
            BinOp::BAnd => Arith::BAnd,
            BinOp::BOr => Arith::BOr,
            BinOp::BXor => Arith::BXor,
            BinOp::Shl => Arith::Shl,
            BinOp::Shr => Arith::Shr,
            _ => return None,
        })
    }

    fn from_unary(op: UnOp) -> Option<Arith> {
        match op {
            UnOp::Neg => Some(Arith::Unm),
            UnOp::BNot => Some(Arith::BNot),
            UnOp::Not | UnOp::Len => None,
        }
    }

    fn is_bitwise(self) -> bool {
        matches!(self, Arith::BAnd | Arith::BOr | Arith::BXor | Arith::Shl | Arith::Shr | Arith::BNot)
    }
}

/// `constfolding` of lcode.c, which folds neither an operation that may raise nor NaN or 0.0.
fn fold_arith(op: Arith, lhs: Const, rhs: Const) -> Option<Const> {
    let (Const::Int(_) | Const::Float(_)) = lhs else { return None };
    let (Const::Int(_) | Const::Float(_)) = rhs else { return None };
    let result = if op.is_bitwise() {
        Const::Int(int_arith(op, to_integer(lhs)?, to_integer(rhs)?))
    } else {
        if matches!(op, Arith::Div | Arith::IDiv | Arith::Mod) && to_float(rhs) == 0.0 {
            return None;
        }
        match (lhs, rhs, op) {
            (Const::Int(a), Const::Int(b), op) if !matches!(op, Arith::Div | Arith::Pow) => {
                Const::Int(int_arith(op, a, b))
            }
            _ => Const::Float(float_arith(op, to_float(lhs), to_float(rhs))),
        }
    };
    match result {
        Const::Float(value) if value.is_nan() || value == 0.0 => None,
        result => Some(result),
    }
}

fn to_float(value: Const) -> f64 {
    match value {
        Const::Int(value) => value as f64,
        Const::Float(value) => value,
        _ => f64::NAN,
    }
}

fn to_integer(value: Const) -> Option<i64> {
    match value {
        Const::Int(value) => Some(value),
        Const::Float(value)
            if value.floor() == value && (-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&value) =>
        {
            Some(value as i64)
        }
        _ => None,
    }
}

fn int_arith(op: Arith, a: i64, b: i64) -> i64 {
    match op {
        Arith::Add => a.wrapping_add(b),
        Arith::Sub => a.wrapping_sub(b),
        Arith::Mul => a.wrapping_mul(b),
        Arith::IDiv if b == -1 => a.wrapping_neg(),
        Arith::IDiv => {
            let quotient = a / b;
            if (a ^ b) < 0 && a % b != 0 {
                quotient - 1
            } else {
                quotient
            }
        }
        Arith::Mod if b == -1 => 0,
        Arith::Mod => {
            let remainder = a % b;
            if remainder != 0 && (remainder ^ b) < 0 {
                remainder + b
            } else {
                remainder
            }
        }
        Arith::Unm => a.wrapping_neg(),
        Arith::BAnd => a & b,
        Arith::BOr => a | b,
        Arith::BXor => a ^ b,
        Arith::Shl => shift_left(a, b),
        Arith::Shr => shift_left(a, b.wrapping_neg()),
        Arith::BNot => !a,
        Arith::Div | Arith::Pow => unreachable!("division and powers are float operations"),
    }
}

fn shift_left(value: i64, by: i64) -> i64 {
    match by {
        ..=-64 | 64.. => 0,
        ..0 => ((value as u64) >> -by) as i64,
        _ => ((value as u64) << by) as i64,
    }
}

fn float_arith(op: Arith, a: f64, b: f64) -> f64 {
    match op {
        Arith::Add => a + b,
        Arith::Sub => a - b,
        Arith::Mul => a * b,
        Arith::Div => a / b,
        Arith::Pow if b == 2.0 => a * a,
        Arith::Pow => a.powf(b),
        Arith::IDiv => (a / b).floor(),
        Arith::Mod => {
            let remainder = a % b;
            if (remainder > 0.0 && b < 0.0) || (remainder < 0.0 && b > 0.0) {
                remainder + b
            } else {
                remainder
            }
        }
        Arith::Unm => -a,
        _ => unreachable!("bitwise operations are integer operations"),
    }
}

/// `luaO_HashString` of CfxLua, the value of a backtick literal.
fn joaat(text: &str) -> i64 {
    let mut hash: u32 = 0;
    for byte in text.bytes() {
        hash = hash.wrapping_add(byte.to_ascii_lowercase() as i8 as i32 as u32);
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash = hash.wrapping_add(hash << 15);
    hash as i32 as i64
}
