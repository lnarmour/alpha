//! `isl_set`: the core polyhedral domain type. See `docs/rust-port-design.md` §5/§6 in the
//! workspace root for the operation inventory this is built from and how it's used by Alpha's
//! semantic analysis (expression/context domain inference, the well-formedness checks) and
//! codegen (the AST builder's context set).
use crate::ctx::{take_c_string, Context, Result};
use crate::space::{DimType, Space};
use std::ffi::CString;

pub struct Set {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_set,
}

/// `ISL_FORMAT_C` vs `ISL_FORMAT_ISL` — see `docs/rust-port-design.md` §5: isl's own C
/// pretty-printer is what codegen leans on for affine/constraint/polynomial-to-C conversion,
/// rather than a hand-rolled printer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Isl,
    C,
}

impl Format {
    pub(crate) fn to_raw(self) -> i32 {
        match self {
            Format::Isl => isl_sys::ISL_FORMAT_ISL as i32,
            Format::C => isl_sys::ISL_FORMAT_C as i32,
        }
    }
}

impl Set {
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_set) -> Set {
        debug_assert!(!ptr.is_null());
        Set { ctx, ptr }
    }

    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_set {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    /// Parses isl's own set syntax (`"{ [i,j] : 0 <= i < N and 0 <= j < N }"`) — this is Alpha's
    /// actual domain-literal grammar; see `docs/rust-port-design.md` §1/§4.
    pub fn read_from_str(ctx: &Context, s: &str) -> Result<Set> {
        let cstr = CString::new(s).expect("isl set text must not contain NUL bytes");
        let ptr = unsafe { isl_sys::isl_set_read_from_str(ctx.as_ptr(), cstr.as_ptr()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn universe(space: Space) -> Set {
        let ctx = space.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_universe(space.into_raw()) };
        unsafe { Set::from_raw(ctx, ptr) }
    }

    pub fn empty(space: Space) -> Set {
        let ctx = space.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_empty(space.into_raw()) };
        unsafe { Set::from_raw(ctx, ptr) }
    }

    pub fn space(&self) -> Space {
        let ptr = unsafe { isl_sys::isl_set_get_space(self.ptr) };
        unsafe { Space::from_raw(self.ctx.clone(), ptr) }
    }

    pub fn dim(&self, ty: DimType) -> u32 {
        let n = unsafe { isl_sys::isl_set_dim(self.ptr, ty.to_raw()) };
        n.max(0) as u32
    }

    pub fn is_empty(&self) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_set_is_empty(self.ptr) })
    }

    pub fn is_equal(&self, other: &Set) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_set_is_equal(self.ptr, other.ptr) })
    }

    pub fn is_disjoint(&self, other: &Set) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_set_is_disjoint(self.ptr, other.ptr) })
    }

    pub fn is_subset(&self, other: &Set) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_set_is_subset(self.ptr, other.ptr) })
    }

    pub fn intersect(self, other: Set) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_intersect(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn union(self, other: Set) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_union(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn subtract(self, other: Set) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_subtract(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn complement(self) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_complement(self.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn intersect_params(self, params: Set) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_intersect_params(self.into_raw(), params.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn affine_hull(self) -> Result<BasicSet> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_affine_hull(self.into_raw()) };
        Ok(unsafe { BasicSet::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn convex_hull(self) -> Result<BasicSet> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_convex_hull(self.into_raw()) };
        Ok(unsafe { BasicSet::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn polyhedral_hull(self) -> Result<BasicSet> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_polyhedral_hull(self.into_raw()) };
        Ok(unsafe { BasicSet::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    /// Simplifies `self` relative to `context` (e.g. dropping constraints implied by an outer
    /// parameter domain) — used pervasively in the source system for producing readable
    /// diagnostic domains (see `docs/rust-port-design.md` §6).
    pub fn gist(self, context: Set) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_gist(self.into_raw(), context.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn coalesce(self) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_coalesce(self.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn project_out(self, ty: DimType, first: u32, n: u32) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_project_out(self.into_raw(), ty.to_raw(), first, n) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn has_upper_bound(&self, ty: DimType, pos: u32) -> Result<bool> {
        let b = unsafe { isl_sys::isl_set_dim_has_upper_bound(self.ptr, ty.to_raw(), pos) };
        self.ctx.check_bool(b)
    }

    pub fn has_lower_bound(&self, ty: DimType, pos: u32) -> Result<bool> {
        let b = unsafe { isl_sys::isl_set_dim_has_lower_bound(self.ptr, ty.to_raw(), pos) };
        self.ctx.check_bool(b)
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_set(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for Set {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::Isl))
    }
}

impl Clone for Set {
    fn clone(&self) -> Set {
        let ptr = unsafe { isl_sys::isl_set_copy(self.ptr) };
        unsafe { Set::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for Set {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_set_free(self.ptr) };
    }
}

/// A single conjunction of constraints (as opposed to `Set`, a union of these) — mainly
/// encountered as the result of hull operations, which always produce one convex piece.
pub struct BasicSet {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_basic_set,
}

impl BasicSet {
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_basic_set) -> BasicSet {
        debug_assert!(!ptr.is_null());
        BasicSet { ctx, ptr }
    }

    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_basic_set {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    pub fn into_set(self) -> Set {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_from_basic_set(self.into_raw()) };
        unsafe { Set::from_raw(ctx, ptr) }
    }
}

impl std::fmt::Display for BasicSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_print_basic_set(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            write!(f, "{s}")
        }
    }
}

impl Clone for BasicSet {
    fn clone(&self) -> BasicSet {
        let ptr = unsafe { isl_sys::isl_basic_set_copy(self.ptr) };
        unsafe { BasicSet::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for BasicSet {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_basic_set_free(self.ptr) };
    }
}
