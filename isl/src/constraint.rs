//! `isl_local_space`/`isl_constraint`: direct constraint construction, coefficient by
//! coefficient. This is the API
//! `WriteCExprConverter`'s reduce-loop-domain construction (`createReduceLoopDomain` in the
//! source Java) uses directly, and the one place `alpha-codegen` builds ISL objects "by hand"
//! rather than by parsing Alpha's own textual domains.
use crate::ctx::{take_c_string, Context, Result};
use crate::set::{BasicSet, Format};
use crate::space::{DimType, Space};

/// A [`Space`] plus a chosen basis for its existentially-quantified ("div") dimensions —
/// required by isl to allocate a [`Constraint`] against, even though Alpha-level constraints
/// never actually reference divs directly.
pub struct LocalSpace {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_local_space,
}

impl LocalSpace {
    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_local_space {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    pub fn from_space(space: Space) -> Result<LocalSpace> {
        let ctx = space.ctx.clone();
        let ptr = unsafe { isl_sys::isl_local_space_from_space(space.into_raw()) };
        Ok(LocalSpace {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }
}

impl Clone for LocalSpace {
    fn clone(&self) -> LocalSpace {
        let ptr = unsafe { isl_sys::isl_local_space_copy(self.ptr) };
        LocalSpace {
            ctx: self.ctx.clone(),
            ptr,
        }
    }
}

impl Drop for LocalSpace {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_local_space_free(self.ptr) };
    }
}

pub struct Constraint {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_constraint,
}

impl Constraint {
    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_constraint {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    /// A fresh `... = 0` equality constraint over `ls`'s space, with every coefficient/the
    /// constant initially zero — build it up via [`Self::set_coefficient`]/[`Self::set_constant`].
    pub fn equality(ls: LocalSpace) -> Result<Constraint> {
        let ctx = ls.ctx.clone();
        let ptr = unsafe { isl_sys::isl_constraint_alloc_equality(ls.into_raw()) };
        Ok(Constraint {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    /// A fresh `... >= 0` inequality constraint, same construction pattern as [`Self::equality`].
    pub fn inequality(ls: LocalSpace) -> Result<Constraint> {
        let ctx = ls.ctx.clone();
        let ptr = unsafe { isl_sys::isl_constraint_alloc_inequality(ls.into_raw()) };
        Ok(Constraint {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    pub fn set_coefficient(self, ty: DimType, pos: u32, v: i32) -> Result<Constraint> {
        let ctx = self.ctx.clone();
        let ptr = unsafe {
            isl_sys::isl_constraint_set_coefficient_si(self.into_raw(), ty.to_raw(), pos as i32, v)
        };
        Ok(Constraint {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    pub fn set_constant(self, v: i32) -> Result<Constraint> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_constraint_set_constant_si(self.into_raw(), v) };
        Ok(Constraint {
            ctx: ctx.clone(),
            ptr: ctx.check(ptr)?,
        })
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_constraint(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for Constraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::Isl))
    }
}

impl Clone for Constraint {
    fn clone(&self) -> Constraint {
        let ptr = unsafe { isl_sys::isl_constraint_copy(self.ptr) };
        Constraint {
            ctx: self.ctx.clone(),
            ptr,
        }
    }
}

impl Drop for Constraint {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_constraint_free(self.ptr) };
    }
}

impl BasicSet {
    pub fn add_constraint(self, c: Constraint) -> Result<BasicSet> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_basic_set_add_constraint(self.into_raw(), c.into_raw()) };
        Ok(unsafe { BasicSet::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }
}
