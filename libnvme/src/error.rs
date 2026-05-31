use std::borrow::Cow;
use std::io;
use std::os::raw::c_int;
use std::str::Utf8Error;

/// Errors returned by this crate.
///
/// Marked `#[non_exhaustive]` so future variants can be added without a
/// breaking release.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// A libnvme call failed at the OS level. The wrapped [`io::Error`]
    /// captures the platform's `errno`.
    Os(io::Error),

    /// The device returned a non-zero NVMe status code.
    ///
    /// The encoding follows the NVMe specification: bits 0–7 hold the status
    /// code, bits 8–10 the status code type. Refer to the NVMe spec or
    /// `libnvme`'s decoders for human-readable interpretation; a future
    /// release of this crate will expose a typed accessor.
    Nvme(u32),

    /// libnvme returned NULL or a sentinel indicating the requested value
    /// is not set on this handle.
    NotAvailable,

    /// A C string returned by libnvme contained invalid UTF-8.
    Utf8(Utf8Error),

    /// Caller passed an argument that this crate rejected before forwarding
    /// to libnvme (e.g. interior NUL byte in a string, controller-id list
    /// exceeding the spec maximum, NLB out of range, buffer length mismatch).
    ///
    /// The wrapped message describes which input failed and why. It is a
    /// [`Cow`] so static reasons cost nothing while dynamically-formatted
    /// ones (with the offending value interpolated) are still possible.
    /// Deref or `.as_ref()` to get a `&str`.
    InvalidArgument(Cow<'static, str>),
}

impl Error {
    /// Construct an [`Error::InvalidArgument`] from any string-like value.
    /// Static `&str` stays borrowed (no allocation); a `String` is moved in.
    pub(crate) fn invalid_argument(msg: impl Into<Cow<'static, str>>) -> Self {
        Error::InvalidArgument(msg.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Os(e) => write!(f, "libnvme: {e}"),
            Error::Nvme(status) => write!(f, "libnvme: NVMe status 0x{status:04x}"),
            Error::NotAvailable => write!(f, "libnvme: value not available"),
            Error::Utf8(e) => write!(f, "libnvme: invalid UTF-8: {e}"),
            Error::InvalidArgument(msg) => write!(f, "libnvme: invalid argument: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Os(e) => Some(e),
            Error::Utf8(e) => Some(e),
            Error::Nvme(_) | Error::NotAvailable | Error::InvalidArgument(_) => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Os(e)
    }
}

impl From<Utf8Error> for Error {
    fn from(e: Utf8Error) -> Self {
        Error::Utf8(e)
    }
}

/// Result type alias for fallible operations in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Map a libnvme `c_int` return value into our `Result<()>`.
///
/// libnvme convention:
/// - `0` — success
/// - negative — `-errno`, or `-1` with the platform errno set
/// - positive — NVMe status code from the device
pub(crate) fn check_ret(ret: c_int) -> Result<()> {
    if ret == 0 {
        Ok(())
    } else if ret < 0 {
        let err = if ret == -1 {
            io::Error::last_os_error()
        } else {
            io::Error::from_raw_os_error(-ret)
        };
        Err(Error::Os(err))
    } else {
        Err(Error::Nvme(ret as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enoent() -> Error {
        io::Error::from_raw_os_error(2).into()
    }

    #[test]
    fn display_has_libnvme_prefix() {
        assert!(format!("{}", enoent()).starts_with("libnvme: "));
    }

    #[test]
    fn source_is_inner_io_error() {
        let err = enoent();
        let source = std::error::Error::source(&err).expect("source should exist");
        assert!(source.downcast_ref::<io::Error>().is_some());
    }

    #[test]
    fn from_io_error_yields_os_variant() {
        assert!(matches!(enoent(), Error::Os(_)));
    }

    #[test]
    fn not_available_has_no_source() {
        let err = Error::NotAvailable;
        assert!(std::error::Error::source(&err).is_none());
    }

    #[test]
    #[allow(invalid_from_utf8)]
    fn utf8_error_displays_with_prefix() {
        let bad = std::str::from_utf8(&[0xFFu8, 0xFE]).unwrap_err();
        let err: Error = bad.into();
        assert!(format!("{err}").starts_with("libnvme: invalid UTF-8"));
    }

    #[test]
    fn nvme_status_formats_as_hex() {
        let err = Error::Nvme(0x4081);
        assert_eq!(format!("{err}"), "libnvme: NVMe status 0x4081");
    }

    #[test]
    fn check_ret_zero_is_ok() {
        assert!(check_ret(0).is_ok());
    }

    #[test]
    fn check_ret_negative_is_os_error() {
        let err = check_ret(-2).unwrap_err();
        match err {
            Error::Os(e) => assert_eq!(e.raw_os_error(), Some(2)),
            _ => panic!("expected Os variant"),
        }
    }

    #[test]
    fn check_ret_positive_is_nvme_status() {
        let err = check_ret(0x281).unwrap_err();
        assert!(matches!(err, Error::Nvme(0x281)));
    }
}
