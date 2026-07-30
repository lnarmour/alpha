//! Confirms the generated bindings don't just compile but actually link and run correctly:
//! create a context, parse a set via isl's own string parser, do one real operation, free
//! everything. Not exhaustive (that's the `isl` safe-wrapper crate's job) — just a sanity check
//! that the FFI boundary itself works on this platform.

use std::ffi::CString;

#[test]
fn parse_a_set_and_check_emptiness() {
    unsafe {
        let ctx = isl_sys::isl_ctx_alloc();
        assert!(!ctx.is_null());

        let empty_str = CString::new("{ [i] : i > 0 and i < 0 }").unwrap();
        let empty_set = isl_sys::isl_set_read_from_str(ctx, empty_str.as_ptr());
        assert!(!empty_set.is_null());
        assert_eq!(
            isl_sys::isl_set_is_empty(empty_set),
            isl_sys::isl_bool::isl_bool_true
        );
        isl_sys::isl_set_free(empty_set);

        let nonempty_str = CString::new("{ [i] : 0 <= i and i < 10 }").unwrap();
        let nonempty_set = isl_sys::isl_set_read_from_str(ctx, nonempty_str.as_ptr());
        assert!(!nonempty_set.is_null());
        assert_eq!(
            isl_sys::isl_set_is_empty(nonempty_set),
            isl_sys::isl_bool::isl_bool_false
        );
        isl_sys::isl_set_free(nonempty_set);

        isl_sys::isl_ctx_free(ctx);
    }
}
