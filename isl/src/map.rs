//! `isl_map`: relations between sets — Alpha's `JNIRelation`, and the workhorse behind
//! dependence-expression domain propagation (`apply`/`preimage`) in `alpha-model`.
use crate::ctx::{take_c_string, Context, Result};
use crate::set::{Format, Set};
use crate::space::{DimType, Space};
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

    /// The universal `{ [x] -> [y] : x >=_lex y }` ordering relation on `space` treated as a set
    /// space (i.e. `space -> space`) — the building block scheduled codegen's legality check
    /// (§7.2 of `docs/scheduled-codegen-design.md`) composes with a dependence edge to test
    /// whether a schedule reorders it.
    pub fn lex_ge_on_space(space: Space) -> Result<Map> {
        let ctx = space.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_lex_ge(space.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn dim(&self, ty: DimType) -> u32 {
        let n = unsafe { isl_sys::isl_map_dim(self.ptr, ty.to_raw()) };
        n.max(0) as u32
    }

    pub fn space(&self) -> Space {
        let ptr = unsafe { isl_sys::isl_map_get_space(self.ptr) };
        unsafe { Space::from_raw(self.ctx.clone(), ptr) }
    }

    /// Tags the domain (`DimType::In`) or range (`DimType::OutOrSet`) side of `self`'s space with
    /// a tuple name — needed wherever a map built from an analysis artifact with no inherent
    /// statement identity (e.g. a reduce's own `projection: MultiAff`, `alpha-codegen`'s
    /// scheduled-codegen legality checker) needs to compose via `apply_range`/`apply_domain`
    /// against another map keyed by statement name, since those require exact tuple-id equality
    /// on the composing side (`isl_space_tuple_is_equal`), not just matching dim counts.
    pub fn set_tuple_name(self, ty: DimType, name: &str) -> Result<Map> {
        let ctx = self.ctx.clone();
        let cname = CString::new(name).expect("tuple name must not contain NUL bytes");
        let ptr = unsafe {
            isl_sys::isl_map_set_tuple_name(self.into_raw(), ty.to_raw(), cname.as_ptr())
        };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    /// Clears any tuple name on the domain/range side of `self`'s space (isl's own convention: a
    /// null name pointer means anonymous) — the counterpart to [`Self::set_tuple_name`], needed to
    /// undo tuple identity a construction step incidentally introduced (e.g. an identity map's
    /// range inheriting its domain's own tuple name, since the two share one space) where a plain
    /// anonymous tuple, matching every *other* differently-constructed map with the same
    /// dimensionality, is what's actually wanted.
    pub fn reset_tuple_name(self, ty: DimType) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe {
            isl_sys::isl_map_set_tuple_name(self.into_raw(), ty.to_raw(), std::ptr::null())
        };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    /// Names dim `pos` of type `ty` — `alpha-transform/src/print.rs`'s `Select` printing uses this
    /// to make a relation's range-side names explicit and queryable before printing (isl's own
    /// `Display` can show a plausible synthesized name for an unnamed dim that `Space::dim_name`
    /// still reports as unset — a display-only convenience, not a stored, queryable fact).
    pub fn set_dim_name(self, ty: DimType, pos: u32, name: &str) -> Result<Map> {
        let ctx = self.ctx.clone();
        let cname = CString::new(name).expect("dimension name must not contain NUL bytes");
        let ptr = unsafe {
            isl_sys::isl_map_set_dim_name(self.into_raw(), ty.to_raw(), pos, cname.as_ptr())
        };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
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

    /// True if `self` maps every input tuple to itself unchanged — used by
    /// `UniquenessAndCompletenessCheck`'s self-recursion check.
    pub fn is_identity(&self) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_map_is_identity(self.ptr) })
    }

    /// True if no two distinct domain points map to the same range point — a target mapping's
    /// per-statement schedule must be injective on its own domain (§6 of
    /// `docs/scheduled-codegen-design.md`): two instances colliding on one schedule-space point
    /// makes their relative order undefined.
    pub fn is_injective(&self) -> Result<bool> {
        self.ctx
            .check_bool(unsafe { isl_sys::isl_map_is_injective(self.ptr) })
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
    /// this. Consumes both, per isl's own ownership convention (see this crate's doc on why
    /// consuming isl calls take `self`/args by value here).
    pub fn apply(self, map: Map) -> Result<Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_apply(self.into_raw(), map.into_raw()) };
        Ok(unsafe { Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }
}
