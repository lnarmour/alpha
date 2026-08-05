//! `isl_val`: an arbitrary-precision rational, as returned by [`crate::Aff`]'s own per-dimension
//! coefficient/constant/denominator accessors — the API [`crate::print`]-equivalent (Alpha-syntax
//! function-literal reconstruction, see `alpha-transform/src/print.rs`) needs to turn `f@X`'s
//! affine function `f` back into `(i,j->i+1,j)`-style source text instead of isl's own map
//! syntax. Alpha's own function-literal coefficients always fit in a machine integer in
//! practice, so this wrapper only exposes the small-integer accessors (`isl_val_get_num_si`/
//! `_den_si`), not the full arbitrary-precision API.
use crate::ctx::Context;

pub struct Val {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_val,
}

impl Val {
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_val) -> Val {
        debug_assert!(!ptr.is_null());
        Val { ctx, ptr }
    }

    pub fn is_zero(&self) -> bool {
        unsafe { isl_sys::isl_val_is_zero(self.ptr) == isl_sys::isl_bool::isl_bool_true }
    }

    /// The value's own numerator as a machine integer — meaningless on its own unless paired
    /// with [`Self::den_si`] (a per-dimension `Aff` coefficient's `den_si` is always `1` in
    /// practice; a *shared* denominator only ever applies to the whole `Aff` at once, via
    /// [`crate::Aff::denominator`]).
    pub fn num_si(&self) -> i64 {
        unsafe { isl_sys::isl_val_get_num_si(self.ptr) as i64 }
    }

    pub fn den_si(&self) -> i64 {
        unsafe { isl_sys::isl_val_get_den_si(self.ptr) as i64 }
    }
}

impl Clone for Val {
    fn clone(&self) -> Val {
        let ptr = unsafe { isl_sys::isl_val_copy(self.ptr) };
        Val {
            ctx: self.ctx.clone(),
            ptr,
        }
    }
}

impl Drop for Val {
    fn drop(&mut self) {
        unsafe {
            isl_sys::isl_val_free(self.ptr);
        }
    }
}
