//! `isl_map`: relations between sets — Alpha's `JNIRelation`, and the workhorse behind
//! dependence-expression domain propagation (`apply`/`preimage`) in `alpha-model`. See
//! `docs/rust-port-design.md` §5/§6 in the workspace root.
use crate::ctx::{take_c_string, Context, Result};
use crate::set::{Format, Set};
use crate::space::DimType;
use std::ffi::CString;

pub struct Map {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_map,
}

impl Map {
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_map) -> Map {
        debug_assert!(!ptr.is_null());
        Map { ctx, ptr }
    }

    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_map {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    pub fn read_from_str(ctx: &Context, s: &str) -> Result<Map> {
        let cstr = CString::new(s).expect("isl map text must not contain NUL bytes");
        let ptr = unsafe { isl_sys::isl_map_read_from_str(ctx.as_ptr(), cstr.as_ptr()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn dim(&self, ty: DimType) -> u32 {
        let n = unsafe { isl_sys::isl_map_dim(self.ptr, ty.to_raw()) };
        n.max(0) as u32
    }

    pub fn domain(self) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_domain(self.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn range(self) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_range(self.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn reverse(self) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_reverse(self.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn is_empty(&self) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_map_is_empty(self.ptr) })
    }

    pub fn is_equal(&self, other: &Map) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_map_is_equal(self.ptr, other.ptr) })
    }

    pub fn is_disjoint(&self, other: &Map) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_map_is_disjoint(self.ptr, other.ptr) })
    }

    pub fn intersect(self, other: Map) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_intersect(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn union(self, other: Map) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_union(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn subtract(self, other: Map) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_subtract(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn intersect_domain(self, set: Set) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_intersect_domain(self.into_raw(), set.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn intersect_range(self, set: Set) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_intersect_range(self.into_raw(), set.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn subtract_domain(self, set: Set) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_subtract_domain(self.into_raw(), set.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn subtract_range(self, set: Set) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_subtract_range(self.into_raw(), set.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn apply_range(self, other: Map) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_apply_range(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn apply_domain(self, other: Map) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_apply_domain(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn flat_range_product(self, other: Map) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_flat_range_product(self.into_raw(), other.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn gist(self, context: Map) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_gist(self.into_raw(), context.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn coalesce(self) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_coalesce(self.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn project_out(self, ty: DimType, first: u32, n: u32) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_project_out(self.into_raw(), ty.to_raw(), first, n) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_map(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for Map {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::Isl))
    }
}

impl Clone for Map {
    fn clone(&self) -> Map {
        let ptr = unsafe { isl_sys::isl_map_copy(self.ptr) };
        unsafe { Map::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_map_free(self.ptr) };
    }
}

impl Set {
    /// The image of `self` under `map` (`map.apply(set)` in the source Java's naming) —
    /// `DependenceExpression`'s forward pre/post-image computation in `alpha-model` goes through
    /// this. Consumes both, per isl's own ownership convention (see `docs/rust-port-design.md`
    /// §5 on why consuming isl calls take `self`/args by value here).
    pub fn apply(self, map: Map) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_apply(self.into_raw(), map.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }
}
