//! A generic recursive-descent visitor over every `Expr` node in a subtree — factored out once
//! several checks (phase 5's duplicate-`UseEquation`-target detection, phase 6's case-branch/
//! reduce-boundedness/undefined-variable checks) needed the identical traversal to find every
//! node of a particular kind (`Expr::Variable`, `Expr::Case`, `Expr::Reduce`, ...) anywhere
//! beneath an expression — mirrors the source system's `EcoreUtil.getAllContents`/
//! `EcoreUtil2.getAllContentsOfType` used for the same purpose.

use alpha_syntax::ast::Expr;

/// Calls `visit` on `expr` itself, then recursively on every `Expr` reachable beneath it.
pub fn walk_expr(expr: &Expr, visit: &mut impl FnMut(&Expr)) {
    visit(expr);
    match expr {
        Expr::If(e) => {
            for sub in [e.cond(), e.then_branch(), e.else_branch()]
                .into_iter()
                .flatten()
            {
                walk_expr(&sub, visit);
            }
        }
        Expr::Restrict(e) => {
            if let Some(sub) = e.expr() {
                walk_expr(&sub, visit);
            }
        }
        Expr::AutoRestrict(e) => {
            if let Some(sub) = e.expr() {
                walk_expr(&sub, visit);
            }
        }
        Expr::Case(e) => {
            for b in e.branches() {
                walk_expr(&b, visit);
            }
        }
        Expr::Dependence(e) => {
            if let Some(sub) = e.applied_expr() {
                walk_expr(&sub, visit);
            }
        }
        Expr::Reduce(e) => {
            if let Some(b) = e.body() {
                walk_expr(&b, visit);
            }
        }
        Expr::Convolution(e) => {
            for sub in [e.kernel_expr(), e.data_expr()].into_iter().flatten() {
                walk_expr(&sub, visit);
            }
        }
        Expr::Select(e) => {
            if let Some(sub) = e.expr() {
                walk_expr(&sub, visit);
            }
        }
        Expr::MultiArg(e) => {
            for a in e.args() {
                walk_expr(&a, visit);
            }
        }
        Expr::Binary(e) => {
            for sub in [e.lhs(), e.rhs()].into_iter().flatten() {
                walk_expr(&sub, visit);
            }
        }
        Expr::Unary(e) => {
            if let Some(sub) = e.operand() {
                walk_expr(&sub, visit);
            }
        }
        Expr::Paren(e) => {
            if let Some(sub) = e.inner() {
                walk_expr(&sub, visit);
            }
        }
        Expr::Variable(_) | Expr::Index(_) | Expr::Bool(_) | Expr::Int(_) | Expr::Real(_) => {}
    }
}
