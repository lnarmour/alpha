//! The isl context (`isl_ctx`) and error handling.
//!
//! Every isl object needs a context to have been created in, and that context must outlive
//! every object created from it. We enforce that by making [`Context`] a cheap `Rc` clone that
//! every wrapper type in this crate holds alongside its raw pointer — the underlying `isl_ctx`
//! is only actually freed once the last `Context` clone (and therefore the last object using it)
//! is dropped.
use std::ffi::CStr;
use std::rc::Rc;

struct CtxHandle(*mut isl_sys::isl_ctx);

impl Drop for CtxHandle {
    fn drop(&mut self) {
        unsafe { isl_sys::isl_ctx_free(self.0) };
    }
}

#[derive(Clone)]
pub struct Context(Rc<CtxHandle>);

impl Context {
    pub fn new() -> Context {
        let ptr = unsafe { isl_sys::isl_ctx_alloc() };
        assert!(!ptr.is_null(), "isl_ctx_alloc returned null");
        // isl's default is to print parse/other errors straight to stderr (`ISL_ON_ERROR_WARN`)
        // in addition to recording them for `isl_ctx_last_error_msg` — fine for a one-shot CLI,
        // noisy for an IDE-facing tool where a user mid-typing an invalid domain would otherwise
        // spam stderr on every keystroke. `Context::check`/`check_bool` already surface every
        // error through `Result`, so silence isl's own printing and rely on that instead.
        unsafe {
            isl_sys::isl_options_set_on_error(ptr, isl_sys::ISL_ON_ERROR_CONTINUE as i32);
        }
        Context(Rc::new(CtxHandle(ptr)))
    }

    pub(crate) fn as_ptr(&self) -> *mut isl_sys::isl_ctx {
        self.0 .0
    }

    /// Every isl FFI call that can fail returns a null pointer on failure; this turns that into
    /// a `Result`, pulling the human-readable message isl itself already tracked via
    /// `isl_ctx_last_error_msg` before resetting the context's error state (mirroring the Java
    /// system's `callISLwithErrorHandling` pattern).
    pub(crate) fn check<T>(&self, ptr: *mut T) -> Result<*mut T> {
        if ptr.is_null() {
            Err(self.last_error())
        } else {
            Ok(ptr)
        }
    }

    /// Same idea as [`Self::check`], for isl's tri-state `isl_bool` (`true`/`false`/`error`)
    /// return convention instead of a pointer.
    pub(crate) fn check_bool(&self, b: isl_sys::isl_bool::Type) -> Result<bool> {
        match b {
            isl_sys::isl_bool::isl_bool_true => Ok(true),
            isl_sys::isl_bool::isl_bool_false => Ok(false),
            _ => Err(self.last_error()),
        }
    }

    /// Same idea as [`Self::check`]/[`Self::check_bool`], for isl's `isl_stat` return convention
    /// (`isl_stat_ok`/`isl_stat_error`) — used by `foreach`-style calls whose callback can itself
    /// signal failure back through the C call by returning `isl_stat_error`.
    pub(crate) fn check_stat(&self, stat: isl_sys::isl_stat::Type) -> Result<()> {
        if stat == isl_sys::isl_stat::isl_stat_ok {
            Ok(())
        } else {
            Err(self.last_error())
        }
    }

    fn last_error(&self) -> IslError {
        let ctx = self.as_ptr();
        let message = unsafe {
            let msg = isl_sys::isl_ctx_last_error_msg(ctx);
            let message = if msg.is_null() {
                "isl operation failed with no further message".to_string()
            } else {
                CStr::from_ptr(msg).to_string_lossy().into_owned()
            };
            isl_sys::isl_ctx_reset_error(ctx);
            message
        };
        IslError { message }
    }
}

impl Default for Context {
    fn default() -> Self {
        Context::new()
    }
}

/// An isl operation failed. Carries isl's own error message (via `isl_ctx_last_error_msg`) —
/// isl's finer-grained `isl_error` code (alloc/invalid/unsupported/...) isn't surfaced
/// separately since none of `alpha-model`'s callers need to branch on it, only display it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IslError {
    pub message: String,
}

impl std::fmt::Display for IslError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for IslError {}

pub type Result<T> = std::result::Result<T, IslError>;

/// Renders a C-string owned/returned by isl (`char *`, to be freed with libc's `free`) into an
/// owned Rust `String`, then frees the original. Used by every `to_string`/`isl_text`-style
/// method backed by `isl_*_to_str`/`isl_printer_get_str`.
pub(crate) unsafe fn take_c_string(ptr: *mut std::os::raw::c_char) -> String {
    assert!(!ptr.is_null(), "isl returned a null string");
    let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    libc::free(ptr as *mut libc::c_void);
    s
}
