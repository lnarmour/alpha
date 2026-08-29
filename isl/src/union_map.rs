//! `isl_union_map`: a union of maps, each with its own (possibly differently-shaped) tuple space
//! — Alpha's target-mapping schedules (`docs/scheduled-codegen-design.md` §6) are exactly one of
//! these, one per-statement `Map` fragment unioned together and keyed by tuple name.
use crate::ctx::{take_c_string, Context, Result};
use crate::map::Map;
use crate::set::Format;
use crate::set::Set;
use crate::space::{DimType, Space};
use std::ffi::CString;

pub struct UnionMap {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_union_map,
}

impl UnionMap {
    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_union_map {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    pub fn from_map(map: Map) -> UnionMap {
        let ctx = map.ctx.clone();
        let ptr = unsafe { isl_sys::isl_union_map_from_map(map.into_raw()) };
        UnionMap { ctx, ptr }
    }

    /// The union map with no fragments at all — the base case for building one up statement by
    /// statement (§6 of `docs/scheduled-codegen-design.md`) via repeated [`Self::union`], and the
    /// well-formed result for a system with zero schedulable statements.
    pub fn empty(ctx: &Context) -> UnionMap {
        let ptr = unsafe { isl_sys::isl_union_map_empty_ctx(ctx.as_ptr()) };
        UnionMap {
            ctx: ctx.clone(),
            ptr,
        }
    }

    /// Parses isl's own union-map syntax (`"{ S1[i] -> [i,0]; S2[i,j] -> [i,1,j]; }"`) — this is
    /// exactly the target-mapping text format (§6): one map per statement, keyed by tuple name.
    pub fn read_from_str(ctx: &Context, s: &str) -> Result<UnionMap> {
        let cstr = CString::new(s).expect("isl union map text must not contain NUL bytes");
        let ptr = unsafe { isl_sys::isl_union_map_read_from_str(ctx.as_ptr(), cstr.as_ptr()) };
        Ok(UnionMap {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    /// Combines per-statement schedule fragments (and identity defaults, §6) into one union map.
    pub fn union(self, other: UnionMap) -> Result<UnionMap> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_union_map_union(self.into_raw(), other.into_raw()) };
        Ok(UnionMap {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    pub fn intersect_params(self, params: Set) -> Result<UnionMap> {
        let ctx = self.ctx.clone();
        let ptr =
            unsafe { isl_sys::isl_union_map_intersect_params(self.into_raw(), params.into_raw()) };
        Ok(UnionMap {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    pub fn project_out(self, ty: DimType, first: u32, n: u32) -> Result<UnionMap> {
        let ctx = self.ctx.clone();
        let ptr =
            unsafe { isl_sys::isl_union_map_project_out(self.into_raw(), ty.to_raw(), first, n) };
        Ok(UnionMap {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    /// Pulls one statement's own map fragment back out of the union, by tuple space — used for
    /// legality checking and identity-default detection (§6, §7.2). isl returns an *empty* map
    /// with `space`'s shape, not an error, when no fragment with that exact tuple space is
    /// present in the union — check the result's own `Map::is_empty` to detect absence, rather
    /// than treating every call as fallible on "not found".
    pub fn extract_map(&self, space: Space) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_union_map_extract_map(self.ptr, space.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    /// Visits every individual `Map` fragment inside the union, each already carrying its own
    /// (already-parsed) tuple name and dimensionality — the only way to discover what's actually
    /// written in a target mapping (§6) without already knowing each statement's exact schedule
    /// width up front, since [`Self::extract_map`] needs the *whole* map space (domain and range
    /// shape both) to find a fragment, not just a domain tuple name.
    pub fn for_each_map(&self, mut f: impl FnMut(Map)) -> Result<()> {
        struct UserData<'a> {
            ctx: Context,
            f: &'a mut dyn FnMut(Map),
        }
        unsafe extern "C" fn trampoline(
            map: *mut isl_sys::isl_map,
            user: *mut std::ffi::c_void,
        ) -> isl_sys::isl_stat::Type {
            let data = &mut *(user as *mut UserData);
            // `map` is `__isl_take` per isl's own header (union_map.h) — this callback owns it;
            // wrapping it in `Map` (whose `Drop` frees it) is the correct way to consume it.
            let m = Map::from_raw(data.ctx.clone(), map);
            (data.f)(m);
            isl_sys::isl_stat::isl_stat_ok
        }
        let mut data = UserData {
            ctx: self.ctx.clone(),
            f: &mut f,
        };
        let user_ptr = &mut data as *mut UserData as *mut std::ffi::c_void;
        let stat =
            unsafe { isl_sys::isl_union_map_foreach_map(self.ptr, Some(trampoline), user_ptr) };
        self.ctx.check_stat(stat)
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_union_map(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for UnionMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::Isl))
    }
}

impl Clone for UnionMap {
    fn clone(&self) -> UnionMap {
        let ptr = unsafe { isl_sys::isl_union_map_copy(self.ptr) };
        UnionMap {
            ctx: self.ctx.clone(),
            ptr,
        }
    }
}

impl Drop for UnionMap {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_union_map_free(self.ptr) };
    }
}
