use qbx_lua_syntax::ast::*;
use qbx_lua_syntax::{Comment, CommentKind, Span};

use crate::{FormatOptions, QuoteStyle};

pub fn print(source: &str, chunk: &Chunk, options: &FormatOptions) -> String {
    let mut printer = Printer { src: source, opts: options, comments: &chunk.comments, next: 0 };
    let shebang = if source.starts_with("#!") { source.lines().next() } else { None };
    let lines = printer.block(&chunk.block, 0, source.len() as u32);
    let mut out = String::with_capacity(source.len());
    if let Some(line) = shebang {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&lines.join("\n"));
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

struct Printer<'a> {
    src: &'a str,
    opts: &'a FormatOptions,
    comments: &'a [Comment],
    /// Index of the first comment that has not been written yet.
    next: usize,
}

fn width(text: &str) -> usize {
    text.chars().count()
}

fn is_multiline(text: &str) -> bool {
    text.contains('\n')
}

impl Printer<'_> {
    fn indent(&self, level: usize) -> String {
        if self.opts.use_tabs {
            "\t".repeat(level)
        } else {
            " ".repeat(level * self.opts.indent_width)
        }
    }

    fn col(&self, level: usize) -> usize {
        level * self.opts.indent_width
    }

    fn text(&self, span: Span) -> &str {
        span.text(self.src)
    }

    fn newlines_between(&self, from: u32, to: u32) -> usize {
        self.src.get(from as usize..to as usize).map_or(0, |gap| gap.matches('\n').count())
    }

    fn has_comment_in(&self, span: Span) -> bool {
        let first = self.comments.partition_point(|c| c.span.start < span.start);
        self.comments.get(first).is_some_and(|c| c.span.start < span.end)
    }

    fn fits(&self, text: &str, col: usize) -> bool {
        !is_multiline(text) && col + width(text) <= self.opts.line_width
    }

    /// Writes the comments that end before `limit`, each on its own line.
    fn leading_comments(&mut self, limit: u32, level: usize, out: &mut Vec<String>, prev_end: &mut Option<u32>) {
        while let Some(comment) = self.comments.get(self.next).filter(|c| c.span.end <= limit) {
            if prev_end.is_some_and(|end| self.newlines_between(end, comment.span.start) >= 2) {
                out.push(String::new());
            }
            out.push(format!("{}{}", self.indent(level), self.text(comment.span).trim_end()));
            *prev_end = Some(comment.span.end);
            self.next += 1;
        }
    }

    /// Appends a comment that sits on the same source line right after `end`.
    fn trailing_comment(&mut self, end: u32, line: &mut String) -> u32 {
        let Some(comment) = self.comments.get(self.next) else { return end };
        // Stops at the first newline: the next comment may lie megabytes ahead of every statement before it.
        let gap = self.src.get(end as usize..comment.span.start as usize);
        let same_line = comment.span.start >= end && gap.is_none_or(|gap| !gap.contains('\n'));
        if !same_line || (comment.kind != CommentKind::Line && is_multiline(self.text(comment.span))) {
            return end;
        }
        line.push(' ');
        line.push_str(self.text(comment.span).trim_end());
        self.next += 1;
        comment.span.end
    }

    fn block(&mut self, block: &Block, level: usize, end_limit: u32) -> Vec<String> {
        let mut out = Vec::new();
        let mut prev_end: Option<u32> = None;
        for stmt in &block.stmts {
            self.leading_comments(stmt.span.start, level, &mut out, &mut prev_end);
            if prev_end.is_some_and(|end| self.newlines_between(end, stmt.span.start) >= 2) {
                out.push(String::new());
            }
            let mut text = self.stmt(stmt, level);
            let unsupported = self.comments.get(self.next).is_some_and(|c| c.span.start < stmt.span.end);
            if unsupported {
                // A comment sits somewhere inside the statement that the printer has no slot for;
                // keeping the author's text is better than moving or losing it.
                text = format!("{}{}", self.indent(level), self.text(stmt.span));
                self.next = self.comments.partition_point(|c| c.span.start < stmt.span.end);
            }
            let needs_separator = text.trim_start().starts_with('(') && !out.is_empty();
            if needs_separator {
                text = format!("{};{}", self.indent(level), text.trim_start());
            }
            let end = self.trailing_comment(stmt.span.end, &mut text);
            out.push(text);
            prev_end = Some(end);
        }
        self.leading_comments(end_limit, level, &mut out, &mut prev_end);
        out
    }

    /// Prints a block below its header, keeping a comment written after the header keyword on
    /// that line: `-- luacheck: ignore` or `---@diagnostic disable-line` only apply there.
    fn body(&mut self, block: &Block, level: usize, end_limit: u32, out: &mut String, header_end: u32) {
        self.trailing_comment(header_end, out);
        for line in self.block(block, level + 1, end_limit) {
            out.push('\n');
            out.push_str(&line);
        }
        out.push('\n');
    }

    fn stmt(&mut self, stmt: &Stmt, level: usize) -> String {
        let pad = self.indent(level);
        let col = self.col(level);
        let end_keyword = stmt.span.end.saturating_sub(3);
        match &stmt.kind {
            StmtKind::Local { names, exprs, in_unpack } => {
                let names: Vec<String> = names
                    .iter()
                    .map(|n| match n.attrib {
                        Some((_, span)) => format!("{} {}", n.name.text, self.text(span)),
                        None => n.name.text.to_string(),
                    })
                    .collect();
                let mut out = format!("{pad}local {}", names.join(", "));
                if !exprs.is_empty() {
                    out.push_str(if *in_unpack { " in " } else { " = " });
                    let start = width(&out);
                    out.push_str(&self.expr_list(exprs, level, start));
                }
                out
            }
            StmtKind::LocalFunction { name, func } => {
                format!("{pad}local function {}{}", name.text, self.func_rest(func, level))
            }
            StmtKind::Function { name, func } => {
                let mut full = name.base.text.to_string();
                for segment in &name.path {
                    full.push('.');
                    full.push_str(&segment.text);
                }
                if let Some(method) = &name.method {
                    full.push(':');
                    full.push_str(&method.text);
                }
                format!("{pad}function {full}{}", self.func_rest(func, level))
            }
            StmtKind::Assign { targets, exprs } => {
                let mut out = format!("{pad}{} = ", self.expr_list(targets, level, col));
                let start = width(&out);
                out.push_str(&self.expr_list(exprs, level, start));
                out
            }
            StmtKind::CompoundAssign { target, op, expr, .. } => {
                let mut out = format!("{pad}{} {}= ", self.expr(target, level, col), op.symbol());
                let start = width(&out);
                out.push_str(&self.expr(expr, level, start));
                out
            }
            StmtKind::Expr(expr) => format!("{pad}{}", self.expr(expr, level, col)),
            StmtKind::Do(body) => {
                let mut out = format!("{pad}do");
                self.body(body, level, end_keyword, &mut out, stmt.span.start);
                out + &pad + "end"
            }
            StmtKind::Defer(body) => {
                let mut out = format!("{pad}defer");
                self.body(body, level, end_keyword, &mut out, stmt.span.start);
                out + &pad + "end"
            }
            StmtKind::While { cond, body } => {
                let mut out = format!("{pad}while {} do", self.expr(cond, level, col + 6));
                self.body(body, level, end_keyword, &mut out, cond.span.end);
                out + &pad + "end"
            }
            StmtKind::Repeat { body, cond } => {
                let mut out = format!("{pad}repeat");
                let until = self.src[..cond.span.start as usize].rfind("until").map_or(cond.span.start, |i| i as u32);
                self.body(body, level, until, &mut out, stmt.span.start);
                out + &pad + "until " + &self.expr(cond, level, col + 6)
            }
            StmtKind::If { branches, else_block } => {
                if let ([branch], None) = (branches.as_slice(), else_block) {
                    if let Some(inline) = self.inline_block(&branch.block, stmt.span) {
                        let line = format!("{pad}if {} then {inline} end", self.expr(&branch.cond, level, col + 3));
                        if self.fits(&line, 0) {
                            return line;
                        }
                    }
                }
                let mut out = String::new();
                for (i, branch) in branches.iter().enumerate() {
                    let keyword = if i == 0 { "if" } else { "elseif" };
                    let cond = self.expr(&branch.cond, level, col + keyword.len() + 1);
                    out.push_str(&format!("{pad}{keyword} {cond} then"));
                    let limit = branches
                        .get(i + 1)
                        .map(|next| next.keyword_span.start)
                        .or_else(|| else_block.as_ref().map(|b| self.else_keyword(branch, b)))
                        .unwrap_or(end_keyword);
                    self.body(&branch.block, level, limit, &mut out, branch.cond.span.end);
                }
                if let Some(block) = else_block {
                    let keyword = branches.last().map_or(stmt.span.start, |branch| self.else_keyword(branch, block));
                    out.push_str(&format!("{pad}else"));
                    self.body(block, level, end_keyword, &mut out, keyword);
                }
                out + &pad + "end"
            }
            StmtKind::NumericFor { var, start, limit, step, body } => {
                let mut head = format!(
                    "{pad}for {} = {}, {}",
                    var.text,
                    self.expr(start, level, col),
                    self.expr(limit, level, col)
                );
                if let Some(step) = step {
                    head.push_str(&format!(", {}", self.expr(step, level, col)));
                }
                let mut out = head + " do";
                let header_end = step.as_ref().map_or(limit.span.end, |step| step.span.end);
                self.body(body, level, end_keyword, &mut out, header_end);
                out + &pad + "end"
            }
            StmtKind::GenericFor { names, exprs, body } => {
                let names: Vec<&str> = names.iter().map(|n| n.text.as_str()).collect();
                let mut out = format!("{pad}for {} in ", names.join(", "));
                let start = width(&out);
                out.push_str(&self.expr_list(exprs, level, start));
                out.push_str(" do");
                let header_end = exprs.last().map_or(stmt.span.start, |expr| expr.span.end);
                self.body(body, level, end_keyword, &mut out, header_end);
                out + &pad + "end"
            }
            StmtKind::Return(exprs) if exprs.is_empty() => format!("{pad}return"),
            StmtKind::Return(exprs) => format!("{pad}return {}", self.expr_list(exprs, level, col + 7)),
            StmtKind::Break => format!("{pad}break"),
            StmtKind::Goto(name) => format!("{pad}goto {}", name.text),
            StmtKind::Label(name) => format!("{pad}::{}::", name.text),
            StmtKind::Error => format!("{pad}{}", self.text(stmt.span)),
        }
    }

    /// A block the author wrote on one line (`if x then return end`) rendered as that one statement.
    fn inline_block(&mut self, block: &Block, construct: Span) -> Option<String> {
        let [only] = block.stmts.as_slice() else { return None };
        let simple = matches!(
            only.kind,
            StmtKind::Return(_) | StmtKind::Break | StmtKind::Goto(_) | StmtKind::Expr(_) | StmtKind::Assign { .. }
        );
        if !simple || self.newlines_between(construct.start, construct.end) > 0 || self.has_comment_in(construct) {
            return None;
        }
        let text = self.stmt(only, 0);
        (!is_multiline(&text)).then_some(text)
    }

    /// Offset of the `else` keyword that separates the last branch from the else block.
    fn else_keyword(&self, branch: &IfBranch, else_block: &Block) -> u32 {
        let from = branch.block.span.end.max(branch.cond.span.end) as usize;
        let to = (else_block.span.start as usize).max(from);
        let mut search = from;
        while let Some(found) = self.src[search..to].find("else") {
            let at = (search + found) as u32;
            let in_comment = self.comments.iter().any(|c| c.span.contains(at));
            if !in_comment {
                return at;
            }
            search += found + 4;
        }
        else_block.span.start
    }

    fn func_rest(&mut self, func: &FuncBody, level: usize) -> String {
        let mut params: Vec<&str> = func.params.iter().map(|p| p.text.as_str()).collect();
        if func.vararg.is_some() {
            params.push("...");
        }
        let mut out = format!("({})", params.join(", "));
        let body_span = Span::new(func.params_span.end, func.end_span.start);
        if func.body.stmts.is_empty() && !self.has_comment_in(body_span) {
            return out + " end";
        }
        if let Some(inline) = self.inline_block(&func.body, func.span) {
            let line = format!("{out} {inline} end");
            if self.col(level) + width(&line) + 16 <= self.opts.line_width {
                return line;
            }
        }
        self.body(&func.body, level, func.end_span.start, &mut out, func.params_span.end);
        out + &self.indent(level) + "end"
    }

    fn expr_list(&mut self, exprs: &[Expr], level: usize, col: usize) -> String {
        let mut out = String::new();
        for (i, expr) in exprs.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let at = if is_multiline(&out) { self.col(level) } else { col + width(&out) };
            out.push_str(&self.expr(expr, level, at));
        }
        out
    }

    fn string(&self, span: Span) -> String {
        let raw = self.text(span);
        let target = match self.opts.quote_style {
            QuoteStyle::Preserve => return raw.to_string(),
            QuoteStyle::Single => '\'',
            QuoteStyle::Double => '"',
        };
        let is_short = raw.len() >= 2 && (raw.starts_with('\'') || raw.starts_with('"'));
        let inner = raw.get(1..raw.len().saturating_sub(1)).unwrap_or("");
        if !is_short || raw.starts_with(target) || inner.contains(['\'', '"', '\\']) {
            return raw.to_string();
        }
        format!("{target}{inner}{target}")
    }

    fn expr(&mut self, expr: &Expr, level: usize, col: usize) -> String {
        let mut out = self.expr_inner(expr, level, col);
        // Inline block comments such as `value --[[@as number]]` stay attached to their expression.
        while let Some(comment) = self.comments.get(self.next) {
            let gap = self.src.get(expr.span.end as usize..comment.span.start as usize);
            let attached = comment.kind != CommentKind::Line
                && gap.is_some_and(|gap| gap.chars().all(|c| c == ' ' || c == '\t'))
                && !is_multiline(self.text(comment.span));
            if !attached {
                break;
            }
            out.push(' ');
            out.push_str(self.text(comment.span));
            self.next += 1;
        }
        out
    }

    fn expr_inner(&mut self, expr: &Expr, level: usize, col: usize) -> String {
        match &expr.kind {
            ExprKind::Nil => "nil".into(),
            ExprKind::True => "true".into(),
            ExprKind::False => "false".into(),
            ExprKind::Vararg => "...".into(),
            ExprKind::Number(_) | ExprKind::JenkinsHash(_) | ExprKind::Error => self.text(expr.span).to_string(),
            ExprKind::String(_) => self.string(expr.span),
            ExprKind::Name(name) => name.text.to_string(),
            ExprKind::Function(func) => format!("function{}", self.func_rest(func, level)),
            ExprKind::Paren(inner) => format!("({})", self.expr(inner, level, col + 1)),
            ExprKind::Field { base, name, safe } => {
                let base = self.expr(base, level, col);
                format!("{base}{}.{}", if *safe { "?" } else { "" }, name.text)
            }
            ExprKind::Index { base, index, safe } => {
                let base = self.expr(base, level, col);
                format!("{base}{}[{}]", if *safe { "?" } else { "" }, self.expr(index, level, col))
            }
            ExprKind::Call { callee, args, style, .. } => {
                let callee = self.expr(callee, level, col);
                let at = if is_multiline(&callee) { self.col(level) } else { col + width(&callee) };
                callee + &self.call_args(args, *style, level, at)
            }
            ExprKind::MethodCall { base, method, args, style, safe, .. } => {
                let head = format!("{}{}:{}", self.expr(base, level, col), if *safe { "?" } else { "" }, method.text);
                let at = if is_multiline(&head) { self.col(level) } else { col + width(&head) };
                head + &self.call_args(args, *style, level, at)
            }
            ExprKind::Unary { op, expr: operand } => {
                let symbol = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "not ",
                    UnOp::Len => "#",
                    UnOp::BNot => "~",
                };
                let inner = self.expr(operand, level, col + symbol.len());
                let clash = *op == UnOp::Neg && inner.starts_with('-');
                format!("{symbol}{}{inner}", if clash { " " } else { "" })
            }
            ExprKind::Binary { .. } => self.binary(expr, level, col),
            ExprKind::Table(fields) => self.table(expr, fields, level, col),
        }
    }

    fn call_args(&mut self, args: &[Expr], style: CallStyle, level: usize, col: usize) -> String {
        match (style, args) {
            (CallStyle::String, [arg]) => return format!(" {}", self.expr(arg, level, col + 1)),
            (CallStyle::Table, [arg]) => return format!(" {}", self.expr(arg, level, col + 1)),
            _ => {}
        }
        if args.is_empty() {
            return "()".into();
        }
        let saved = self.next;
        let mut parts: Vec<String> = Vec::with_capacity(args.len());
        let mut at = col + 1;
        for arg in args {
            let text = self.expr(arg, level, at);
            at += width(text.lines().next().unwrap_or("")) + 2;
            parts.push(text);
        }
        // Tables and functions may span lines inside the parentheses; anything else that wraps
        // means the call itself has to break.
        let only_last_breaks = parts.iter().zip(args).all(|(part, arg)| {
            !is_multiline(part) || matches!(arg.unparen().kind, ExprKind::Table(_) | ExprKind::Function(_))
        });
        let hugged = format!("({})", parts.join(", "));
        let first_line = hugged.lines().next().unwrap_or("");
        if only_last_breaks && col + width(first_line) <= self.opts.line_width {
            return hugged;
        }
        self.next = saved;
        let inner = self.indent(level + 1);
        let mut out = String::from("(");
        for (i, arg) in args.iter().enumerate() {
            let text = self.expr(arg, level + 1, self.col(level + 1));
            out.push_str(&format!("\n{inner}{text}{}", if i + 1 < args.len() { "," } else { "" }));
        }
        out + "\n" + &self.indent(level) + ")"
    }

    fn binary(&mut self, expr: &Expr, level: usize, col: usize) -> String {
        let mut operands: Vec<&Expr> = Vec::new();
        let mut ops: Vec<BinOp> = Vec::new();
        let mut current = expr;
        while let ExprKind::Binary { op, lhs, rhs, .. } = &current.kind {
            operands.push(rhs);
            ops.push(*op);
            current = lhs;
        }
        operands.push(current);
        operands.reverse();
        ops.reverse();

        let saved = self.next;
        let breakable = ops.iter().any(|op| matches!(op, BinOp::And | BinOp::Or | BinOp::Concat));
        // First see what the chain looks like with unlimited room: if it is one line that is merely
        // too long, breaking at the operators reads better than wrapping a call buried inside it.
        let mut natural = self.expr(operands[0], level, 0);
        for (op, operand) in ops.iter().zip(&operands[1..]) {
            natural.push_str(&format!(" {} {}", op.symbol(), self.expr(operand, level, 0)));
        }
        let overflows = !is_multiline(&natural) && col + width(&natural) > self.opts.line_width;
        if !is_multiline(&natural) && !overflows {
            return natural;
        }
        self.next = saved;
        if !(overflows && breakable) {
            let mut flat = self.expr(operands[0], level, col);
            for (op, operand) in ops.iter().zip(&operands[1..]) {
                flat.push_str(&format!(" {} ", op.symbol()));
                let at = col + width(flat.lines().last().unwrap_or(""));
                flat.push_str(&self.expr(operand, level, at));
            }
            let first_line = flat.lines().next().unwrap_or("");
            if !breakable || col + width(first_line) <= self.opts.line_width {
                return flat;
            }
            self.next = saved;
        }
        let inner = self.indent(level + 1);
        let mut out = self.expr(operands[0], level, col);
        for (op, operand) in ops.iter().zip(&operands[1..]) {
            let symbol = op.symbol();
            if matches!(op, BinOp::And | BinOp::Or | BinOp::Concat) {
                let text = self.expr(operand, level + 1, self.col(level + 1) + symbol.len() + 1);
                out.push_str(&format!("\n{inner}{symbol} {text}"));
            } else {
                let at = width(out.lines().last().unwrap_or(""));
                out.push_str(&format!(" {symbol} {}", self.expr(operand, level + 1, at)));
            }
        }
        out
    }

    fn field(&mut self, field: &TableField, level: usize) -> String {
        let col = self.col(level);
        match field {
            TableField::Positional(value) => self.expr(value, level, col),
            TableField::Named { name, value } => {
                format!("{} = {}", name.text, self.expr(value, level, col + width(&name.text) + 3))
            }
            TableField::Keyed { key, value } => {
                let key = self.expr(key, level, col + 1);
                let at = col + width(&key) + 5;
                format!("[{key}] = {}", self.expr(value, level, at))
            }
            TableField::SetMember(name) => format!(".{}", name.text),
        }
    }

    fn table(&mut self, expr: &Expr, fields: &[TableField], level: usize, col: usize) -> String {
        let inside = Span::new(expr.span.start + 1, expr.span.end.saturating_sub(1));
        let has_comments = self.has_comment_in(inside);
        if fields.is_empty() && !has_comments {
            return "{}".into();
        }
        let first_start = fields.first().map(field_start);
        let was_expanded = first_start.is_some_and(|start| self.newlines_between(expr.span.start, start) > 0);
        if !has_comments && !was_expanded {
            let saved = self.next;
            let parts: Vec<String> = fields.iter().map(|f| self.field(f, level)).collect();
            let flat = format!("{{ {} }}", parts.join(", "));
            if self.fits(&flat, col) {
                return flat;
            }
            self.next = saved;
        }

        let had_trailing_separator =
            fields.last().is_some_and(|last| self.src[field_end(last) as usize..].trim_start().starts_with([',', ';']));
        let mut lines: Vec<String> = Vec::new();
        let mut prev_end: Option<u32> = None;
        for (i, field) in fields.iter().enumerate() {
            let is_last = i + 1 == fields.len();
            let start = field_start(field);
            self.leading_comments(start, level + 1, &mut lines, &mut prev_end);
            if prev_end.is_some_and(|end| self.newlines_between(end, start) >= 2) {
                lines.push(String::new());
            }
            let separator = if is_last && !had_trailing_separator { "" } else { "," };
            let mut line = format!("{}{}{separator}", self.indent(level + 1), self.field(field, level + 1));
            let end = field_end(field);
            let after_comma = self.src[end as usize..].find([',', ';']).map_or(end, |i| end + i as u32 + 1);
            let separator_is_near =
                self.src[end as usize..after_comma as usize].trim_matches([',', ';', ' ', '\t']).is_empty();
            let comment_from = if separator_is_near { after_comma } else { end };
            let trailing_end = self.trailing_comment(comment_from, &mut line);
            lines.push(line);
            prev_end = Some(trailing_end.max(end));
        }
        self.leading_comments(inside.end, level + 1, &mut lines, &mut prev_end);
        format!("{{\n{}\n{}}}", lines.join("\n"), self.indent(level))
    }
}

fn field_start(field: &TableField) -> u32 {
    match field {
        TableField::Positional(value) => value.span.start,
        TableField::Named { name, .. } | TableField::SetMember(name) => name.span.start,
        TableField::Keyed { key, .. } => key.span.start.saturating_sub(1),
    }
}

fn field_end(field: &TableField) -> u32 {
    match field {
        TableField::Positional(value) | TableField::Named { value, .. } | TableField::Keyed { value, .. } => {
            value.span.end
        }
        TableField::SetMember(name) => name.span.end,
    }
}
