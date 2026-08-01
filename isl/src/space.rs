//! `isl_space`: the "shape" (parameter/input/output dimension counts and names) shared by every
//! set/map/aff. `DimType` is isl's single most-referenced type in the source Java codebase (used
//! pervasively to distinguish param/in/out/set dims when building/querying spaces).

use crate::ctx::{Context, Result};
use std::ffi::CString;

/// `isl_dim_type`. `Set`/`Out` share a representation in isl itself (`isl_dim_set ==
/// isl_dim_out`); kept as one variant here rather than two spellings of the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DimType {
    Param,
    In,
    /// `Out` for maps, `Set` for sets — isl itself defines these as the same enum value.
    OutOrSet,
    Div,
    All,
}

impl DimType {
    pub(crate) fn to_raw(self) -> isl_sys::isl_dim_type::Type {
        use isl_sys::isl_dim_type as T;
        match self {
            DimType::Param => T::isl_dim_param,
            DimType::In => T::isl_dim_in,
            DimType::OutOrSet => T::isl_dim_out,
            DimType::Div => T::isl_dim_div,
            DimType::All => T::isl_dim_all,
        }
    }
}

pub struct Space {
    pub(crate) ctx: Context,
    pub(crate) ptr: *mut isl_sys::isl_space,
}

impl Space {
    /// Takes ownership of an already-checked, non-null isl pointer. Every public constructor
    /// elsewhere in this crate that produces a `Space` goes through here after `Context::check`.
    pub(crate) unsafe fn from_raw(ctx: Context, ptr: *mut isl_sys::isl_space) -> Space {
        debug_assert!(!ptr.is_null());
        Space { ctx, ptr }
    }

    pub fn dim(&self, ty: DimType) -> u32 {
        let n = unsafe { isl_sys::isl_space_dim(self.ptr, ty.to_raw()) };
        // isl_size is `int`; negative means "error querying an unbound/invalid space", which
        // shouldn't happen for a space obtained from a real set/map — treat as 0 rather than
        // panicking, since dimension counts are used in non-fallible contexts throughout.
        n.max(0) as u32
    }

    pub fn dim_name(&self, ty: DimType, pos: u32) -> Option<String> {
        let ptr = unsafe { isl_sys::isl_space_get_dim_name(self.ptr, ty.to_raw(), pos) };
        if ptr.is_null() {
            return None;
        }
        Some(
            unsafe { std::ffi::CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    pub fn set_dim_name(self, ty: DimType, pos: u32, name: &str) -> Result<Space> {
        let ctx = self.ctx.clone();
        let cname = CString::new(name).expect("dimension name must not contain NUL bytes");
        let ptr = unsafe {
            isl_sys::isl_space_set_dim_name(self.into_raw(), ty.to_raw(), pos, cname.as_ptr())
        };
        let ptr = ctx.check(ptr)?;
        Ok(unsafe { Space::from_raw(ctx, ptr) })
    }

    /// Consumes `self` without freeing — for handing ownership to an isl C function that takes
    /// (`__isl_take`) the pointer.
    pub(crate) fn into_raw(self) -> *mut isl_sys::isl_space {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }
}

impl Clone for Space {
    fn clone(&self) -> Space {
        let ptr = unsafe { isl_sys::isl_space_copy(self.ptr) };
        unsafe { Space::from_raw(self.ctx.clone(), ptr) }
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_space_free(self.ptr) };
    }
}
