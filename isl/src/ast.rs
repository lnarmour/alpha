//! `isl_ast_build`/`isl_ast_node`/`isl_ast_expr`: isl's loop-generation entry point. This is the
//! single highest-value piece of isl to bind well — it's where isl's own
//! loop-generation/simplification algorithm (deciding loop order, collapsing bounds, ...) earns
//! its keep, and exactly what
//! `alpha-codegen`'s `WriteC` demand-driven generator uses instead of hand-rolling one.
use crate::ctx::{take_c_string, Context, Result};
use crate::set::{Format, Set};
use crate::union_map::UnionMap;
use std::ffi::CString;

pub struct AstBuild {
    ctx: Context,
    ptr: *mut isl_sys::isl_ast_build,
}

impl AstBuild {
    /// `context` bounds the build's parameter space (e.g. a system's parameter domain) — see
    /// `LoopGenerator.generateLoops` in the source Java for the equivalent call.
    pub fn from_context(context: Set) -> Result<AstBuild> {
        let ctx = context.ctx.clone();
        let ptr = unsafe { isl_sys::isl_ast_build_from_context(context.into_raw()) };
        Ok(AstBuild {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    /// Walks every point in `schedule`'s domain in the order `schedule` maps them to (identity
    /// schedule = plain lexicographic order, as in the demand-driven `WriteC` generator),
    /// producing the loop/conditional AST isl's own build algorithm computes.
    pub fn generate(&self, schedule: UnionMap) -> Result<AstNode> {
        let ptr =
            unsafe { isl_sys::isl_ast_build_node_from_schedule_map(self.ptr, schedule.into_raw()) };
        Ok(unsafe { AstNode::from_raw(self.ctx.clone(), self.ctx.check(ptr)?) })
    }

    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_ast_build {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    /// Names the loop iterators `generate` introduces, in dimension order — `alpha-codegen`'s
    /// `WriteC` generator uses this so the C identifiers isl's own AST builder emits for a loop
    /// match the equation's own index names (or, for a `Reduce`'s ambient dims held fixed while
    /// generating its own reduction loop, that dim's "primed" parameter name).
    pub fn set_iterators(self, names: &[&str]) -> Result<AstBuild> {
        let ctx = self.ctx.clone();
        let mut list_ptr = unsafe { isl_sys::isl_id_list_alloc(ctx.as_ptr(), names.len() as i32) };
        list_ptr = ctx.check(list_ptr)?;
        for name in names {
            let cname = CString::new(*name).expect("iterator name must not contain NUL bytes");
            let id_ptr = unsafe {
                isl_sys::isl_id_alloc(ctx.as_ptr(), cname.as_ptr(), std::ptr::null_mut())
            };
            let id_ptr = ctx.check(id_ptr)?;
            list_ptr = ctx.check(unsafe { isl_sys::isl_id_list_add(list_ptr, id_ptr) })?;
        }
        let ptr = unsafe { isl_sys::isl_ast_build_set_iterators(self.into_raw(), list_ptr) };
        Ok(AstBuild {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    /// Renders `set` (a Presburger condition sharing `self`'s "current" dims — see this method's
    /// callers in `alpha-codegen` for how they line those up) as a boolean isl AST expression,
    /// e.g. a `case` branch's guard domain into the C condition of a ternary — `isl_ast_build_expr_from_set`.
    /// Unlike [`Self::generate`], this doesn't consume `self`: it's meant to be called repeatedly
    /// against the same build (one per `case` branch of the same equation).
    pub fn expr_from_set(&self, set: Set) -> Result<AstExpr> {
        let ptr = unsafe { isl_sys::isl_ast_build_expr_from_set(self.ptr, set.into_raw()) };
        Ok(unsafe { AstExpr::from_raw(self.ctx.clone(), self.ctx.check(ptr)?) })
    }
}

impl Clone for AstBuild {
    fn clone(&self) -> AstBuild {
        let ptr = unsafe { isl_sys::isl_ast_build_copy(self.ptr) };
        AstBuild {
            ctx: self.ctx.clone(),
            ptr,
        }
    }
}

impl Drop for AstBuild {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_ast_build_free(self.ptr) };
    }
}

pub struct AstNode {
    ctx: Context,
    ptr: *mut isl_sys::isl_ast_node,
}

/// `isl_ast_node_type`, minus the `mark`/`error` variants — `mark` nodes (schedule-tree
/// annotations) are scheduling-tree territory, out of scope for this port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstNodeKind {
    For {
        degenerate: bool,
    },
    If,
    Block,
    User,
    /// A schedule-tree `mark` node, or a type isl introduces in a future version this crate
    /// doesn't know about yet — round-trippable but not decomposed.
    Other,
}

impl AstNode {
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_ast_node) -> AstNode {
        debug_assert!(!ptr.is_null());
        AstNode { ctx, ptr }
    }

    pub fn kind(&self) -> AstNodeKind {
        use isl_sys::isl_ast_node_type as T;
        match unsafe { isl_sys::isl_ast_node_get_type(self.ptr) } {
            T::isl_ast_node_for => {
                let degenerate = self
                    .ctx
                    .check_bool(unsafe { isl_sys::isl_ast_node_for_is_degenerate(self.ptr) })
                    .unwrap_or(false);
                AstNodeKind::For { degenerate }
            }
            T::isl_ast_node_if => AstNodeKind::If,
            T::isl_ast_node_block => AstNodeKind::Block,
            T::isl_ast_node_user => AstNodeKind::User,
            _ => AstNodeKind::Other,
        }
    }

    // --- `for` nodes ---
    pub fn for_iterator(&self) -> Result<AstExpr> {
        self.expr_from(unsafe { isl_sys::isl_ast_node_for_get_iterator(self.ptr) })
    }
    pub fn for_init(&self) -> Result<AstExpr> {
        self.expr_from(unsafe { isl_sys::isl_ast_node_for_get_init(self.ptr) })
    }
    pub fn for_cond(&self) -> Result<AstExpr> {
        self.expr_from(unsafe { isl_sys::isl_ast_node_for_get_cond(self.ptr) })
    }
    pub fn for_inc(&self) -> Result<AstExpr> {
        self.expr_from(unsafe { isl_sys::isl_ast_node_for_get_inc(self.ptr) })
    }
    pub fn for_body(&self) -> Result<AstNode> {
        let ptr = unsafe { isl_sys::isl_ast_node_for_get_body(self.ptr) };
        Ok(unsafe { AstNode::from_raw(self.ctx.clone(), self.ctx.check(ptr)?) })
    }

    // --- `if` nodes ---
    pub fn if_cond(&self) -> Result<AstExpr> {
        self.expr_from(unsafe { isl_sys::isl_ast_node_if_get_cond(self.ptr) })
    }
    pub fn if_then(&self) -> Result<AstNode> {
        let ptr = unsafe { isl_sys::isl_ast_node_if_get_then(self.ptr) };
        Ok(unsafe { AstNode::from_raw(self.ctx.clone(), self.ctx.check(ptr)?) })
    }
    /// `None` if this `if` has no `else` branch. isl's own "else is another if-node" chaining
    /// (which the source Java's `ASTConverter` unrolls into C `else if`) is left as-is here —
    /// callers walk it themselves via recursive `if_cond`/`if_then`/`if_else` on the returned
    /// node, same shape as the C API.
    pub fn if_else(&self) -> Option<AstNode> {
        let ptr = unsafe { isl_sys::isl_ast_node_if_get_else(self.ptr) };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { AstNode::from_raw(self.ctx.clone(), ptr) })
        }
    }

    // --- `block` nodes ---
    pub fn block_children(&self) -> Result<Vec<AstNode>> {
        let list_ptr = unsafe { isl_sys::isl_ast_node_block_get_children(self.ptr) };
        let list_ptr = self.ctx.check(list_ptr)?;
        let n = unsafe { isl_sys::isl_ast_node_list_size(list_ptr) }.max(0) as u32;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let node_ptr = unsafe { isl_sys::isl_ast_node_list_get_at(list_ptr, i as i32) };
            out.push(unsafe { AstNode::from_raw(self.ctx.clone(), self.ctx.check(node_ptr)?) });
        }
        unsafe { isl_sys::isl_ast_node_list_free(list_ptr) };
        Ok(out)
    }

    // --- `user` nodes ---
    pub fn user_expr(&self) -> Result<AstExpr> {
        self.expr_from(unsafe { isl_sys::isl_ast_node_user_get_expr(self.ptr) })
    }

    fn expr_from(&self, ptr: *mut isl_sys::isl_ast_expr) -> Result<AstExpr> {
        Ok(unsafe { AstExpr::from_raw(self.ctx.clone(), self.ctx.check(ptr)?) })
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_ast_node(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for AstNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::C))
    }
}

impl Clone for AstNode {
    fn clone(&self) -> AstNode {
        let ptr = unsafe { isl_sys::isl_ast_node_copy(self.ptr) };
        unsafe { AstNode::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for AstNode {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_ast_node_free(self.ptr) };
    }
}

pub struct AstExpr {
    ctx: Context,
    ptr: *mut isl_sys::isl_ast_expr,
}

/// `isl_ast_expr_type`. `Op`'s specific operator (`+`, `min`, ternary select, ...) is exposed
/// separately via [`AstExpr::op_type`]/[`AstExpr::op_args`] rather than folded in here — isl has
/// ~30 operator kinds and `alpha-codegen`'s converters only ever need to match on a handful of
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AstExprKind {
    Op,
    Id,
    Int,
}

impl AstExpr {
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_ast_expr) -> AstExpr {
        debug_assert!(!ptr.is_null());
        AstExpr { ctx, ptr }
    }

    pub fn kind(&self) -> AstExprKind {
        use isl_sys::isl_ast_expr_type as T;
        match unsafe { isl_sys::isl_ast_expr_get_type(self.ptr) } {
            T::isl_ast_expr_op => AstExprKind::Op,
            T::isl_ast_expr_id => AstExprKind::Id,
            // isl_ast_expr_int, and the (unreachable in practice) error variant.
            _ => AstExprKind::Int,
        }
    }

    /// The identifier name, for [`AstExprKind::Id`] expressions (e.g. a loop iterator reference
    /// inside a `for`'s body).
    pub fn id_name(&self) -> Result<String> {
        let id_ptr = unsafe { isl_sys::isl_ast_expr_get_id(self.ptr) };
        let id_ptr = self.ctx.check(id_ptr)?;
        let name = unsafe {
            let name_ptr = isl_sys::isl_id_get_name(id_ptr);
            let name = if name_ptr.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(name_ptr)
                    .to_string_lossy()
                    .into_owned()
            };
            isl_sys::isl_id_free(id_ptr);
            name
        };
        Ok(name)
    }

    /// The integer value, for [`AstExprKind::Int`] expressions. Truncates to `i64` — isl's
    /// arbitrary-precision `isl_val` can in principle hold more, but no Alpha-level integer
    /// literal or loop bound in practice needs more than 64 bits.
    pub fn int_value(&self) -> Result<i64> {
        let val_ptr = unsafe { isl_sys::isl_ast_expr_get_val(self.ptr) };
        let val_ptr = self.ctx.check(val_ptr)?;
        let v = unsafe {
            let v = isl_sys::isl_val_get_num_si(val_ptr);
            isl_sys::isl_val_free(val_ptr);
            v
        };
        Ok(v as i64)
    }

    /// The operator kind, for [`AstExprKind::Op`] expressions.
    pub fn op_type(&self) -> isl_sys::isl_ast_expr_op_type::Type {
        unsafe { isl_sys::isl_ast_expr_get_op_type(self.ptr) }
    }

    pub fn op_args(&self) -> Result<Vec<AstExpr>> {
        let n = unsafe { isl_sys::isl_ast_expr_op_get_n_arg(self.ptr) }.max(0) as u32;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let ptr = unsafe { isl_sys::isl_ast_expr_op_get_arg(self.ptr, i as i32) };
            out.push(unsafe { AstExpr::from_raw(self.ctx.clone(), self.ctx.check(ptr)?) });
        }
        Ok(out)
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_ast_expr(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for AstExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::C))
    }
}

impl Clone for AstExpr {
    fn clone(&self) -> AstExpr {
        let ptr = unsafe { isl_sys::isl_ast_expr_copy(self.ptr) };
        unsafe { AstExpr::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for AstExpr {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_ast_expr_free(self.ptr) };
    }
}
