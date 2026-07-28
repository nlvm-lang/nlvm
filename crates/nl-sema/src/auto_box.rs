//! Issue #15 — applies `checker::AutoBoxFix`es onto the caller's own
//! (pre-expansion) `SourceFile`s: for each fix, finds the `auto` declaration
//! it names (by source path + line + variable name — see `AutoBoxFix`'s doc
//! comment for why those three, not a checker-internal id, identify it) and
//! fills in its `ty` field, turning e.g. `auto n = 0;` into the equivalent
//! of `int n = 0;` before `nl_codegen::compile_program` ever runs its own
//! (independent) `nl_syntax::monomorphize::expand`. From nl-codegen's side
//! this is indistinguishable from the programmer having written the
//! explicit type themselves, so no nl-codegen change is needed at all.

use std::collections::HashMap;

use nl_syntax::ast::{ClosureBody, Expr, LValue, SourceFile, SourceItem, Stmt, StmtKind, Type};

use crate::checker::AutoBoxFix;

/// `(path, line, name)` — see `AutoBoxFix`'s doc comment for why this
/// triple, rather than a checker-internal id, identifies a declaration.
type FixKey = (String, u32, String);

pub(crate) fn apply(files: &mut [SourceFile], fixes: &[AutoBoxFix]) {
    if fixes.is_empty() {
        return;
    }
    // A template method's body is checked once per monomorphized
    // instantiation, but all of them share the exact same pre-expansion
    // source location (`generate_instantiation` copies the template's own
    // `path`) — so the same `FixKey` can arrive more than once. When two
    // instantiations disagree on the resolved type (`Holder<int>` vs.
    // `Holder<string>` both declaring `auto n = seed;`), patching the
    // shared template source with either one arbitrarily would silently
    // miscompile the other; such a key is dropped entirely (`None` here),
    // so that declaration deterministically falls back to the pre-issue-#15
    // behavior instead of a coin flip between the two concrete types.
    let mut by_key: HashMap<FixKey, Option<Type>> = HashMap::new();
    for fix in fixes {
        let key = (fix.path.clone(), fix.line, fix.name.clone());
        by_key
            .entry(key)
            .and_modify(|existing| {
                if existing.as_ref().is_some_and(|ty| *ty != fix.ty) {
                    *existing = None;
                }
            })
            .or_insert_with(|| Some(fix.ty.clone()));
    }
    let resolved: HashMap<FixKey, Type> = by_key
        .into_iter()
        .filter_map(|(key, ty)| ty.map(|ty| (key, ty)))
        .collect();
    if resolved.is_empty() {
        return;
    }

    for file in files.iter_mut() {
        let SourceItem::Class(class) = &mut file.item else {
            continue;
        };
        for method in &mut class.methods {
            for stmt in &mut method.body {
                apply_stmt(&file.path, stmt, &resolved);
            }
        }
    }
}

fn apply_block(path: &str, block: &mut [Stmt], fixes: &HashMap<FixKey, Type>) {
    for stmt in block {
        apply_stmt(path, stmt, fixes);
    }
}

fn apply_stmt(path: &str, stmt: &mut Stmt, fixes: &HashMap<FixKey, Type>) {
    let line = stmt.line;
    match &mut stmt.kind {
        StmtKind::VarDecl { ty, name, init, .. } => {
            if ty.is_none() {
                let key = (path.to_string(), line, name.clone());
                if let Some(resolved_ty) = fixes.get(&key) {
                    *ty = Some(resolved_ty.clone());
                }
            }
            if let Some(e) = init {
                apply_expr(path, e, fixes);
            }
        }
        StmtKind::Return(Some(e)) | StmtKind::Throw(e) => apply_expr(path, e, fixes),
        StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Expr(e) => apply_expr(path, e, fixes),
        StmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            apply_expr(path, cond, fixes);
            apply_block(path, then_branch, fixes);
            if let Some(b) = else_branch {
                apply_block(path, b, fixes);
            }
        }
        StmtKind::While { cond, body } => {
            apply_expr(path, cond, fixes);
            apply_block(path, body, fixes);
        }
        StmtKind::ForEach { iterable, body, .. } => {
            apply_expr(path, iterable, fixes);
            apply_block(path, body, fixes);
        }
        StmtKind::For {
            init,
            cond,
            step,
            body,
        } => {
            for s in init {
                apply_stmt(path, s, fixes);
            }
            if let Some(c) = cond {
                apply_expr(path, c, fixes);
            }
            for e in step {
                apply_expr(path, e, fixes);
            }
            apply_block(path, body, fixes);
        }
        StmtKind::Block(b) => apply_block(path, b, fixes),
        StmtKind::ThisCall(args) | StmtKind::SuperCall(args) => {
            for a in args {
                apply_expr(path, &mut a.value, fixes);
            }
        }
        StmtKind::Try {
            body,
            catches,
            finally,
        } => {
            apply_block(path, body, fixes);
            for c in catches {
                apply_block(path, &mut c.body, fixes);
            }
            if let Some(f) = finally {
                apply_block(path, f, fixes);
            }
        }
        StmtKind::Switch { subject, cases } => {
            apply_expr(path, subject, fixes);
            for case in cases {
                if let Some(v) = &mut case.value {
                    apply_expr(path, v, fixes);
                }
                apply_block(path, &mut case.body, fixes);
            }
        }
    }
}

fn apply_expr(path: &str, expr: &mut Expr, fixes: &HashMap<FixKey, Type>) {
    match expr {
        Expr::IntLit(_)
        | Expr::FloatLit(_)
        | Expr::BoolLit(_)
        | Expr::StringLit(_)
        | Expr::NullLit
        | Expr::This
        | Expr::Super
        | Expr::Ident(_) => {}
        Expr::PostIncr(target)
        | Expr::PostDecr(target)
        | Expr::PreIncr(target)
        | Expr::PreDecr(target) => apply_lvalue(path, target, fixes),
        Expr::Assign(target, value) => {
            apply_lvalue(path, target, fixes);
            apply_expr(path, value, fixes);
        }
        Expr::Call(_, args) | Expr::New(_, _, args) => {
            for a in args {
                apply_expr(path, &mut a.value, fixes);
            }
        }
        Expr::NewArray(_, dims) => {
            for size in dims.iter_mut().flatten() {
                apply_expr(path, size, fixes);
            }
        }
        Expr::NewArrayInit(_, elements) => {
            for e in elements {
                apply_expr(path, e, fixes);
            }
        }
        Expr::FieldAccess(target, _) | Expr::InstanceOf(target, _) => {
            apply_expr(path, target, fixes)
        }
        Expr::Cast(_, inner) => apply_expr(path, inner, fixes),
        Expr::MethodCall(target, _, args) => {
            apply_expr(path, target, fixes);
            for a in args {
                apply_expr(path, &mut a.value, fixes);
            }
        }
        Expr::Index(target, index) => {
            apply_expr(path, target, fixes);
            apply_expr(path, index, fixes);
        }
        Expr::Unary(_, inner) => apply_expr(path, inner, fixes),
        Expr::Binary(_, lhs, rhs) => {
            apply_expr(path, lhs, fixes);
            apply_expr(path, rhs, fixes);
        }
        Expr::Match(subject, arms) => {
            apply_expr(path, subject, fixes);
            for arm in arms {
                if let Some(p) = &mut arm.pattern {
                    apply_expr(path, p, fixes);
                }
                apply_expr(path, &mut arm.value, fixes);
            }
        }
        Expr::Ternary(cond, then_e, else_e) => {
            apply_expr(path, cond, fixes);
            apply_expr(path, then_e, fixes);
            apply_expr(path, else_e, fixes);
        }
        Expr::Coalesce(lhs, rhs) | Expr::Elvis(lhs, rhs) => {
            apply_expr(path, lhs, fixes);
            apply_expr(path, rhs, fixes);
        }
        Expr::Closure { body, .. } => match body {
            ClosureBody::Block(b) => apply_block(path, b, fixes),
            ClosureBody::Expr(e) => apply_expr(path, e, fixes),
        },
    }
}

fn apply_lvalue(path: &str, lvalue: &mut LValue, fixes: &HashMap<FixKey, Type>) {
    match lvalue {
        LValue::Local(_) => {}
        LValue::Field(target, _) => apply_expr(path, target, fixes),
        LValue::Index(target, index) => {
            apply_expr(path, target, fixes);
            apply_expr(path, index, fixes);
        }
    }
}
