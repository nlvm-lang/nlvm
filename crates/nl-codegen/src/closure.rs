//! Free-variable collection for closure literals — vm.md § Closures and
//! anonymous functions. `referenced_names` walks a closure body's AST and
//! returns every bare name it references (`Expr::Ident`, assignment
//! targets, `++`/`--` operands); `Emitter::compile_closure` then checks
//! each candidate against the *enclosing* method's locals to decide which
//! ones are actual captures (a name that isn't an outer local is something
//! else entirely — a class reference, or a name declared inside the
//! closure body itself — and is simply left alone).

use std::collections::HashSet;

use nl_syntax::ast::{Block, ClosureBody, Expr, LValue, Stmt};

pub(crate) fn referenced_names(body: &ClosureBody) -> HashSet<String> {
    let mut names = HashSet::new();
    match body {
        ClosureBody::Block(block) => collect_block(block, &mut names),
        ClosureBody::Expr(e) => collect_expr(e, &mut names),
    }
    names
}

fn collect_block(block: &Block, names: &mut HashSet<String>) {
    for stmt in block {
        collect_stmt(stmt, names);
    }
}

fn collect_stmt(stmt: &Stmt, names: &mut HashSet<String>) {
    match stmt {
        Stmt::Return(Some(e)) | Stmt::Throw(e) => collect_expr(e, names),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        Stmt::Expr(e) => collect_expr(e, names),
        Stmt::VarDecl { init, .. } => {
            if let Some(e) = init {
                collect_expr(e, names);
            }
        }
        Stmt::If { cond, then_branch, else_branch } => {
            collect_expr(cond, names);
            collect_block(then_branch, names);
            if let Some(b) = else_branch {
                collect_block(b, names);
            }
        }
        Stmt::While { cond, body } => {
            collect_expr(cond, names);
            collect_block(body, names);
        }
        Stmt::ForEach { iterable, body, .. } => {
            collect_expr(iterable, names);
            collect_block(body, names);
        }
        Stmt::For { init, cond, step, body } => {
            for s in init {
                collect_stmt(s, names);
            }
            if let Some(c) = cond {
                collect_expr(c, names);
            }
            for e in step {
                collect_expr(e, names);
            }
            collect_block(body, names);
        }
        Stmt::Block(b) => collect_block(b, names),
        Stmt::ThisCall(args) | Stmt::SuperCall(args) => {
            for a in args {
                collect_expr(a, names);
            }
        }
        Stmt::Try { body, catches, finally } => {
            collect_block(body, names);
            for c in catches {
                collect_block(&c.body, names);
            }
            if let Some(f) = finally {
                collect_block(f, names);
            }
        }
    }
}

fn collect_expr(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::IntLit(_)
        | Expr::FloatLit(_)
        | Expr::BoolLit(_)
        | Expr::StringLit(_)
        | Expr::NullLit
        | Expr::This
        | Expr::Super => {}
        Expr::Ident(name) | Expr::PostIncr(name) | Expr::PostDecr(name) => {
            names.insert(name.clone());
        }
        Expr::Assign(target, value) => {
            collect_lvalue(target, names);
            collect_expr(value, names);
        }
        Expr::Call(_, args) | Expr::New(_, _, args) => {
            for a in args {
                collect_expr(a, names);
            }
        }
        Expr::NewArray(_, size) => collect_expr(size, names),
        Expr::FieldAccess(target, _) | Expr::InstanceOf(target, _) => collect_expr(target, names),
        Expr::MethodCall(target, _, args) => {
            collect_expr(target, names);
            for a in args {
                collect_expr(a, names);
            }
        }
        Expr::Index(target, index) => {
            collect_expr(target, names);
            collect_expr(index, names);
        }
        Expr::Unary(_, inner) => collect_expr(inner, names),
        Expr::Binary(_, lhs, rhs) => {
            collect_expr(lhs, names);
            collect_expr(rhs, names);
        }
        Expr::Match(subject, arms) => {
            collect_expr(subject, names);
            for arm in arms {
                if let Some(p) = &arm.pattern {
                    collect_expr(p, names);
                }
                collect_expr(&arm.value, names);
            }
        }
        Expr::Ternary(cond, then_e, else_e) => {
            collect_expr(cond, names);
            collect_expr(then_e, names);
            collect_expr(else_e, names);
        }
        Expr::Closure { params, body, .. } => {
            // A nested closure may itself reference a variable from this
            // (outer) closure's enclosing scope — recurse, but drop its own
            // parameter names first so they aren't mistaken for captures.
            let mut inner = HashSet::new();
            match body {
                ClosureBody::Block(b) => collect_block(b, &mut inner),
                ClosureBody::Expr(e) => collect_expr(e, &mut inner),
            }
            for p in params {
                inner.remove(&p.name);
            }
            names.extend(inner);
        }
    }
}

fn collect_lvalue(lvalue: &LValue, names: &mut HashSet<String>) {
    match lvalue {
        LValue::Local(name) => {
            names.insert(name.clone());
        }
        LValue::Field(target, _) => collect_expr(target, names),
        LValue::Index(target, index) => {
            collect_expr(target, names);
            collect_expr(index, names);
        }
    }
}
