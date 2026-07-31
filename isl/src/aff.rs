//! `isl_aff`/`isl_multi_aff`: affine functions — Alpha's `JNIFunction`, and the type
//! `DependenceExpression`'s access function and `ReduceExpression`'s projection function
//! resolve to. See `docs/rust-port-design.md` §5/§6/§7 in the workspace root: isl's own
//! C-format pretty-printer for these (via [`MultiAff::to_string_fmt`]/[`Aff::to_string_fmt`]
//! with [`crate::Format::C`]) is what codegen leans on instead of a hand-rolled printer.
use crate::ctx::{take_c_string, Context, Result};
use crate::map::Map;
use crate::set::Format;
use crate::space::{DimType, Space};
use std::ffi::CString;

pub struct Aff {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_aff,
}

impl Aff {
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_aff) -> Aff {
        debug_assert!(!ptr.is_null());
        Aff { ctx, ptr }
    }

    /// Not yet called anywhere: no `Aff`-consuming isl operation is wired up yet (only
    /// `MultiAff`-level ones), but every other wrapper type in this crate keeps this as part of
    /// its ownership-transfer contract, so this one does too ahead of the first caller.
    #[allow(dead_code)]
    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_aff {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    pub fn read_from_str(ctx: &Context, s: &str) -> Result<Aff> {
        let cstr = CString::new(s).expect("isl aff text must not contain NUL bytes");
        let ptr = unsafe { isl_sys::isl_aff_read_from_str(ctx.as_ptr(), cstr.as_ptr()) };
        Ok(unsafe { Aff::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn space(&self) -> Space {
        let ptr = unsafe { isl_sys::isl_aff_get_space(self.ptr) };
        unsafe { Space::from_raw(self.ctx.clone(), ptr) }
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_aff(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for Aff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::Isl))
    }
}

impl Clone for Aff {
    fn clone(&self) -> Aff {
        let ptr = unsafe { isl_sys::isl_aff_copy(self.ptr) };
        unsafe { Aff::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for Aff {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_aff_free(self.ptr) };
    }
}

/// A tuple of [`Aff`]s sharing one domain space — e.g. the affine function `(i,j -> i+1,j-1)` in
/// Alpha's `(idx -> exprs)` function-literal notation resolves to one of these.
pub struct MultiAff {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_multi_aff,
}

impl MultiAff {
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_multi_aff) -> MultiAff {
        debug_assert!(!ptr.is_null());
        MultiAff { ctx, ptr }
    }

    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_multi_aff {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    pub fn read_from_str(ctx: &Context, s: &str) -> Result<MultiAff> {
        let cstr = CString::new(s).expect("isl multi-aff text must not contain NUL bytes");
        let ptr = unsafe { isl_sys::isl_multi_aff_read_from_str(ctx.as_ptr(), cstr.as_ptr()) };
        Ok(unsafe { MultiAff::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn space(&self) -> Space {
        let ptr = unsafe { isl_sys::isl_multi_aff_get_space(self.ptr) };
        unsafe { Space::from_raw(self.ctx.clone(), ptr) }
    }

    /// The input (domain) half of [`Self::space`] alone, as its own `Set`-kind space — unlike
    /// `space()`, which is a map-shaped `[in] -> [out]` space that `Set::universe`/`Set::empty`
    /// reject (they require a set space, `n_in == 0`). `IndexExpression`'s expression-domain
    /// inference (`docs/rust-port-design.md` §6: `ISLSet.buildUniverse(function.getDomainSpace())`
    /// in the source Java) needs exactly this.
    pub fn domain_space(&self) -> Space {
        let ptr = unsafe { isl_sys::isl_multi_aff_get_domain_space(self.ptr) };
        unsafe { Space::from_raw(self.ctx.clone(), ptr) }
    }

    pub fn dim(&self, ty: DimType) -> u32 {
        let n = unsafe { isl_sys::isl_multi_aff_dim(self.ptr, ty.to_raw()) };
        n.max(0) as u32
    }

    /// Number of output dimensions — the arity check `alpha-model` needs for e.g.
    /// `DependenceExpression`'s "does this function's output arity match the child expression's
    /// domain dimension?" check (see `docs/rust-port-design.md` §6).
    pub fn n_out(&self) -> u32 {
        self.dim(DimType::OutOrSet)
    }

    /// Moves `n` dims of type `src_ty` starting at `src_pos` to type `dst_ty` starting at
    /// `dst_pos` — `UniquenessAndCompletenessCheck`'s self-recursion check
    /// (`docs/rust-port-design.md` §6: `inUseEquation`'s "is this `UseEquation`'s call-parameter
    /// function the identity") uses this to turn a purely-parametric `MultiAff` (0 input dims,
    /// output expressed over parameters) into one with matching input dims, since
    /// [`Map::is_identity`] only means something once inputs and outputs are the same kind of
    /// dim.
    pub fn move_dims(
        self,
        dst_ty: DimType,
        dst_pos: u32,
        src_ty: DimType,
        src_pos: u32,
        n: u32,
    ) -> Result<MultiAff> {
        let ctx = self.ctx.clone();
        let ptr = unsafe {
            isl_sys::isl_multi_aff_move_dims(
                self.into_raw(),
                dst_ty.to_raw(),
                dst_pos,
                src_ty.to_raw(),
                src_pos,
                n,
            )
        };
        Ok(unsafe { MultiAff::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn get_aff(&self, pos: u32) -> Result<Aff> {
        let ptr = unsafe { isl_sys::isl_multi_aff_get_aff(self.ptr, pos as i32) };
        Ok(unsafe { Aff::from_raw(self.ctx.clone(), self.ctx.check(ptr)?) })
    }

    /// The identity function over a domain-shaped `space` (`n_in == 0`, e.g. from [`Self::domain_space`]
    /// or a plain `Set`'s [`crate::set::Set::space`]) — `Normalize`'s rule inserting an implicit
    /// identity dependence around a bare `VariableExpression` (`docs/rust-port-design.md` §7:
    /// "every `VariableExpression` must have a `DependenceExpression` parent") builds its function
    /// this way.
    pub fn identity_on_domain_space(space: Space) -> Result<MultiAff> {
        let ctx = space.ctx.clone();
        let ptr = unsafe { isl_sys::isl_multi_aff_identity_on_domain_space(space.into_raw()) };
        Ok(unsafe { MultiAff::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    /// `self ∘ other` (apply `other` first, then `self`) — `Normalize`'s dependence-composition
    /// rule (`f1 @ f2 @ E -> (f2 o f1) @ E`, `docs/rust-port-design.md` §7) is exactly
    /// `f2.pullback(f1)`.
    pub fn pullback(self, other: MultiAff) -> Result<MultiAff> {
        let ctx = self.ctx.clone();
        let ptr =
            unsafe { isl_sys::isl_multi_aff_pullback_multi_aff(self.into_raw(), other.into_raw()) };
        Ok(unsafe { MultiAff::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    /// True if `self` maps every input tuple to itself unchanged (`Normalize`'s dependence-removal
    /// rule: `f @ E = E if f` is the identity) — routes through [`Map::is_identity`] since isl has
    /// no direct `isl_multi_aff_is_identity`.
    pub fn is_identity(&self) -> Result<bool> {
        self.clone().into_map()?.is_identity()
    }

    /// True if `self` maps a 0-dimensional domain to a 0-dimensional range — the source system's
    /// `ISLUtil.isNoneToNone`, a degenerate case [`Self::is_identity`] can't evaluate (an identity
    /// check needs at least one dimension to compare).
    pub fn is_none_to_none(&self) -> bool {
        self.dim(DimType::In) == 0 && self.dim(DimType::OutOrSet) == 0
    }

    /// The graph of this function as a `Map` — the source system's "function.toMap()" pattern
    /// (see `docs/rust-port-design.md` §5), used to feed a `MultiAff` into map-based set/map
    /// algebra (`apply`, `intersect`, ...) where a plain function object won't do.
    pub fn into_map(self) -> Result<Map> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_map_from_multi_aff(self.into_raw()) };
        Ok(unsafe { Map::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }

    pub fn to_string_fmt(&self, format: Format) -> String {
        unsafe {
            let printer = isl_sys::isl_printer_to_str(self.ctx.as_ptr());
            let printer = isl_sys::isl_printer_set_output_format(printer, format.to_raw());
            let printer = isl_sys::isl_printer_print_multi_aff(printer, self.ptr);
            let s = take_c_string(isl_sys::isl_printer_get_str(printer));
            isl_sys::isl_printer_free(printer);
            s
        }
    }
}

impl std::fmt::Display for MultiAff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_fmt(Format::Isl))
    }
}

impl Clone for MultiAff {
    fn clone(&self) -> MultiAff {
        let ptr = unsafe { isl_sys::isl_multi_aff_copy(self.ptr) };
        unsafe { MultiAff::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for MultiAff {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_multi_aff_free(self.ptr) };
    }
}

impl crate::set::Set {
    /// The preimage of `self` under `f` — `ReduceExpression`/`ArgReduceExpression`'s domain
    /// inference (image/preimage under the projection function, see `docs/rust-port-design.md`
    /// §6) goes through this.
    pub fn preimage_multi_aff(self, f: MultiAff) -> Result<crate::set::Set> {
        let ctx = self.ctx.clone();
        let ptr = unsafe { isl_sys::isl_set_preimage_multi_aff(self.into_raw(), f.into_raw()) };
        Ok(unsafe { crate::set::Set::from_raw(ctx.clone(), ctx.check(ptr)?) })
    }
}
