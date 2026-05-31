use std::ffi::CStr;
use std::os::raw::c_char;

use crate::{Error, Result};

/// Convert a libnvme-returned `const char *` into a trimmed `&str`.
///
/// Returns `Error::NotAvailable` if the pointer is NULL, or `Error::Utf8` if
/// the bytes aren't valid UTF-8.
///
/// # Safety
///
/// `ptr` must be either NULL or a valid NUL-terminated C string that remains
/// live for at least `'a`.
pub(crate) unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Result<&'a str> {
    if ptr.is_null() {
        return Err(Error::NotAvailable);
    }
    // SAFETY: ptr is non-null (checked above) and the caller of cstr_to_str
    // guarantees it points to a valid NUL-terminated C string live for 'a.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    let trimmed = trim_trailing_padding(cstr.to_bytes());
    Ok(std::str::from_utf8(trimmed)?)
}

/// Decode an NVMe fixed-length ASCII field (space-padded, possibly NUL-padded)
/// into a trimmed `&str` borrowing from the input slice.
pub(crate) fn fixed_ascii_to_str(bytes: &[c_char]) -> Result<&str> {
    // c_char is i8 on Linux x86_64/aarch64 but its bit pattern matches u8 for
    // ASCII. Reinterpret the slice without copying.
    // SAFETY: c_char and u8 have identical layout (size 1, align 1); the slice
    // pointer/length are valid for the lifetime of `bytes` and we only borrow.
    let raw: &[u8] =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u8, bytes.len()) };
    let trimmed = trim_trailing_padding(raw);
    Ok(std::str::from_utf8(trimmed)?)
}

fn trim_trailing_padding(s: &[u8]) -> &[u8] {
    let end = s
        .iter()
        .rposition(|&b| b != 0 && !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    &s[..end]
}

/// Convert a `&str` into a `CString` for FFI, mapping the interior-NUL
/// failure case to a typed [`Error::InvalidArgument`].
///
/// Used at every call site that hands a host-provided string into
/// libnvme — fabrics setup, auth keys, lookup-host, etc.
pub(crate) fn str_to_cstring(s: &str, err_msg: &'static str) -> Result<std::ffi::CString> {
    std::ffi::CString::new(s).map_err(|_| Error::invalid_argument(err_msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_trailing_spaces() {
        assert_eq!(trim_trailing_padding(b"hello   "), b"hello");
    }

    #[test]
    fn trims_trailing_nuls() {
        assert_eq!(trim_trailing_padding(b"hello\0\0"), b"hello");
    }

    #[test]
    fn trims_mixed_padding() {
        assert_eq!(trim_trailing_padding(b"abc \0 \0"), b"abc");
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(trim_trailing_padding(b""), b"");
    }

    #[test]
    fn all_padding_yields_empty() {
        assert_eq!(trim_trailing_padding(b"   "), b"");
    }

    #[test]
    fn preserves_internal_spaces() {
        assert_eq!(trim_trailing_padding(b"a b  "), b"a b");
    }

    #[test]
    fn fixed_ascii_decodes_padded_field() {
        let raw: [c_char; 8] = [b'm' as _, b'o' as _, b'd' as _, b' ' as _, 0, 0, 0, 0];
        assert_eq!(fixed_ascii_to_str(&raw).unwrap(), "mod");
    }
}
