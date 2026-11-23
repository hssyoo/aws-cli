use crate::runtime::Library;
use crate::s3::UNKNOWN_ERROR_CODE;
use crt_sys as ffi;
use std::ffi::CStr;
use std::ffi::c_int;
use std::ptr::NonNull;

pub(crate) fn check(status: c_int) -> Result<(), i32> {
    check_keeping_status(status).map_err(|(_, code)| code)
}
pub(crate) fn check_keeping_status(status: c_int) -> Result<(), (c_int, i32)> {
    if status == 0 {
        Ok(())
    } else {
        Err((status, last_error_or_unknown()))
    }
}
pub(crate) fn check_pointer<T>(pointer: *mut T) -> Result<NonNull<T>, i32> {
    NonNull::new(pointer).ok_or_else(last_error_or_unknown)
}
#[cfg(test)]
pub(crate) fn last_raised() -> i32 {
    last_error_or_unknown()
}
fn last_error_or_unknown() -> i32 {
    // SAFETY: reads only calling-thread error state.
    let error = unsafe { ffi::rust_crt_last_error() };
    if error == 0 {
        UNKNOWN_ERROR_CODE
    } else {
        error
    }
}

pub(crate) fn copy_c_string(pointer: *const std::ffi::c_char) -> Vec<u8> {
    if pointer.is_null() {
        return Vec::new();
    }
    // SAFETY: CRT lookup returns a static NUL-terminated string while initialized.
    unsafe { CStr::from_ptr(pointer).to_bytes().to_vec() }
}
/// The CRT's name for `code`, `AWS_ERROR_OOM` for example; empty for a code
/// the CRT does not recognize.
pub fn error_name(code: i32) -> Vec<u8> {
    let _library = Library::acquire();
    // SAFETY: CRT accepts any integer error code.
    copy_c_string(unsafe { ffi::rust_crt_error_name(code) })
}
/// The CRT's sentence for `code`; empty for a code the CRT does not
/// recognize. Holds its own `Library` reference, as [`error_name`] does.
pub fn error_message(code: i32) -> Vec<u8> {
    let _library = Library::acquire();
    // SAFETY: CRT accepts any integer error code.
    copy_c_string(unsafe { ffi::rust_crt_error_message(code) })
}
