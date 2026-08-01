//! `isl_pw_qpolynomial`: piecewise-quasipolynomials — Alpha's `JNIPolynomial`
//! (`PolynomialIndexExpression`, `val { ... }`). Parsing/printing these is core (vanilla) isl;
//! *counting* the cardinality of a
//! set as one of these (Ehrhart/Barvinok counting, for `malloc` sizing) is not — that lives in
//! the separate, GPL-licensed `barvinok` crate, feature-gated in `alpha-codegen` (§5, §10).
use crate::ctx::{take_c_string, Context, Result};
use crate::set::Format;
use crate::space::Space;
use std::ffi::CString;

pub struct PwQPolynomial {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_pw_qpolynomial,
}

impl PwQPolynomial {
    pub(crate) unsafe fn from_raw(
        ctx: Context,
        ptr: *mut isl_sys::isl_pw_qpolynomial,
    ) -> PwQPolynomial {
        debug_assert!(!ptr.is_null());
        PwQPolynomial { ctx, ptr }
    }

    pub fn read_from_str(ctx: &Context, s: &str) -> Result<PwQPolynomial> {
        let cstr = CString::new(s).expect("isl polynomial text must not contain NUL bytes");
        let ptr = unsafe { isl_sys::isl_pw_qpolynomial_read_from_str(ctx.as_ptr(), cstr.as_ptr()) };
        Ok(unsafe { PwQPolynomial::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn space(&self) -> Space {
        let ptr = unsafe { isl_sys::isl_pw_qpolynomial_get_space(self.ptr) };
        unsafe { Space::from_raw(self.ctx.clone(), ptr) }
    }

    /// The polynomial's domain space alone, as its own `Set`-kind space — unlike `space()`,
    /// which (for a `pw_qpolynomial` built from `[idx] -> poly` text) is map-shaped and rejected
    /// by `Set::universe`/`Set::empty` (they require a set space, `n_in == 0`).
    /// `PolynomialIndexExpression`'s expression-domain inference needs exactly this.
    pub fn domain_space(&self) -> Space {
        let ptr = unsafe { isl_sys::isl_pw_qpolynomial_get_domain_space(self.ptr) };
        unsafe { Space::from_raw(self.ctx.clone(), ptr) }
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_pw_qpolynomial(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for PwQPolynomial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::Isl))
    }
}

impl Clone for PwQPolynomial {
    fn clone(&self) -> PwQPolynomial {
        let ptr = unsafe { isl_sys::isl_pw_qpolynomial_copy(self.ptr) };
        unsafe { PwQPolynomial::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for PwQPolynomial {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_pw_qpolynomial_free(self.ptr) };
    }
}
