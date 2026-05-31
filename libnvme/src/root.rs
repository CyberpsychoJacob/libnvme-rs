use std::io;
use std::marker::PhantomData;
use std::ptr::NonNull;

#[cfg(has_hostid_from_file)]
use libnvme_sys::nvmf_hostid_from_file;
#[cfg(has_hostid_generate)]
use libnvme_sys::nvmf_hostid_generate;
use libnvme_sys::{
    nvme_default_host, nvme_free_tree, nvme_lookup_host, nvme_root, nvme_scan,
    nvmf_hostnqn_from_file, nvmf_hostnqn_generate,
};

use crate::host::{Host, Hosts};
use crate::util::str_to_cstring;
use crate::{Error, Result};

/// The owning handle to the libnvme tree.
///
/// Created by [`Root::scan`], which calls `nvme_scan` to enumerate hosts,
/// subsystems, controllers, and namespaces on the system. All other handle
/// types ([`Host`], [`Subsystem`], [`Controller`], [`Namespace`]) borrow from
/// the `Root` via the `'r` lifetime, so dropping the `Root` cascades-frees
/// the entire tree via `nvme_free_tree`.
///
/// `Root` is `!Send` and `!Sync`. libnvme's scan state isn't documented as
/// thread-safe; this restriction may be relaxed in a future release after
/// an audit.
///
/// [`Host`]: crate::Host
/// [`Subsystem`]: crate::Subsystem
/// [`Controller`]: crate::Controller
/// [`Namespace`]: crate::Namespace
pub struct Root {
    inner: NonNull<nvme_root>,
    _not_send_sync: PhantomData<*const ()>,
}

impl Root {
    /// Scan the system and build a libnvme handle tree.
    ///
    /// Equivalent to `nvme_scan(NULL)` — uses the default config-file lookup.
    /// Returns the platform's last-set `errno` (via [`std::io::Error`]) if
    /// libnvme returns NULL.
    pub fn scan() -> Result<Self> {
        // SAFETY: nvme_scan accepts NULL as a valid argument (means: use default
        // config-file path). The returned pointer is either NULL or a fresh
        // owned root handle we wrap below.
        let raw = unsafe { nvme_scan(std::ptr::null()) };
        let inner = NonNull::new(raw).ok_or_else(io::Error::last_os_error)?;
        Ok(Root {
            inner,
            _not_send_sync: PhantomData,
        })
    }

    /// Iterate over the hosts in this tree.
    ///
    /// libnvme typically reports a single host (the local machine) unless
    /// the caller has configured Fabrics with multiple hostnqn entries.
    pub fn hosts(&self) -> Hosts<'_> {
        Hosts::new(self.inner.as_ptr())
    }

    /// Get or create the default host for this root.
    ///
    /// libnvme uses `/etc/nvme/hostnqn` and `/etc/nvme/hostid` if present,
    /// otherwise generates new identifiers and persists them.
    pub fn default_host(&self) -> Result<Host<'_>> {
        // SAFETY: self.inner is a non-null nvme_root_t we own, alive for &self.
        let raw = unsafe { nvme_default_host(self.inner.as_ptr()) };
        if raw.is_null() {
            return Err(Error::Os(io::Error::last_os_error()));
        }
        Ok(Host::from_raw(raw, self.inner.as_ptr()))
    }

    /// Look up or create a host with the given NQN and (optional) HostID.
    ///
    /// Use this when you need a non-default host identity — for instance, a
    /// fabrics client that wants to present a specific HostNQN to a target.
    pub fn lookup_host(&self, hostnqn: &str, hostid: Option<&str>) -> Result<Host<'_>> {
        let hostnqn_c = str_to_cstring(hostnqn, "interior NUL byte in hostnqn")?;
        let hostid_c = hostid
            .map(|h| str_to_cstring(h, "interior NUL byte in hostid"))
            .transpose()?;
        let hostid_ptr = match &hostid_c {
            Some(c) => c.as_ptr(),
            None => std::ptr::null(),
        };
        // SAFETY: self.inner is a non-null nvme_root_t we own; hostnqn_c is a
        // valid NUL-terminated C string alive for this call; hostid_ptr is
        // either NULL or points to hostid_c which is alive through this call.
        let raw = unsafe { nvme_lookup_host(self.inner.as_ptr(), hostnqn_c.as_ptr(), hostid_ptr) };
        if raw.is_null() {
            return Err(Error::Os(io::Error::last_os_error()));
        }
        Ok(Host::from_raw(raw, self.inner.as_ptr()))
    }
}

/// Generate a fresh HostNQN (NVMe spec format, embeds a freshly-generated UUID).
/// Equivalent to libnvme's `nvmf_hostnqn_generate`. Returned string is owned.
pub fn generate_hostnqn() -> Result<String> {
    // SAFETY: nvmf_hostnqn_generate takes no arguments and returns either NULL
    // or a malloc-allocated C string we take ownership of via take_owned_cstr.
    let raw = unsafe { nvmf_hostnqn_generate() };
    take_owned_cstr(raw)
}

/// Generate a fresh HostID (random UUID, formatted as a hex string).
///
/// Only present when built against a libnvme that exposes
/// `nvmf_hostid_generate` (added after libnvme 1.8).
#[cfg(has_hostid_generate)]
pub fn generate_hostid() -> Result<String> {
    // SAFETY: nvmf_hostid_generate takes no arguments and returns either NULL
    // or a malloc-allocated C string we take ownership of via take_owned_cstr.
    let raw = unsafe { nvmf_hostid_generate() };
    take_owned_cstr(raw)
}

/// Read the local HostNQN from `/etc/nvme/hostnqn` if it exists.
pub fn hostnqn_from_file() -> Result<String> {
    // SAFETY: nvmf_hostnqn_from_file takes no arguments and returns either NULL
    // or a malloc-allocated C string we take ownership of via take_owned_cstr.
    let raw = unsafe { nvmf_hostnqn_from_file() };
    take_owned_cstr(raw)
}

/// Read the local HostID from `/etc/nvme/hostid` if it exists.
///
/// Only present when built against a libnvme that exposes
/// `nvmf_hostid_from_file` (added after libnvme 1.8).
#[cfg(has_hostid_from_file)]
pub fn hostid_from_file() -> Result<String> {
    // SAFETY: nvmf_hostid_from_file takes no arguments and returns either NULL
    // or a malloc-allocated C string we take ownership of via take_owned_cstr.
    let raw = unsafe { nvmf_hostid_from_file() };
    take_owned_cstr(raw)
}

/// Take ownership of a libnvme-allocated `*mut c_char`, copy it into an owned
/// `String`, and free the original via libc's `free`.
///
/// The free runs unconditionally on the non-null path — including the
/// UTF-8 error case — so libnvme's allocation is never leaked, even on
/// bad input.
///
/// A NULL return is treated as a failed *operation* and mapped to
/// [`Error::Os`] (capturing `errno` — e.g. `ENOENT` for a missing
/// `/etc/nvme/hostnqn`, `ENOMEM` on allocation failure), consistent with
/// the other operation entry points in this module. [`Error::NotAvailable`]
/// is reserved for handle accessors where a property is genuinely unset.
fn take_owned_cstr(ptr: *mut std::os::raw::c_char) -> Result<String> {
    if ptr.is_null() {
        return Err(Error::Os(io::Error::last_os_error()));
    }
    // Copy bytes out before freeing so the free is unconditional regardless
    // of whether the bytes form valid UTF-8.
    // SAFETY: ptr is non-null (checked above) and points to a valid
    // NUL-terminated C string produced by libnvme via malloc. The CStr
    // borrow ends before we free the pointer.
    let bytes = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_bytes().to_vec();
    // SAFETY: ptr came from a libnvme allocator (malloc-family). We own it
    // exclusively; nothing aliases it past this point.
    unsafe { libc_free(ptr as *mut _) };
    String::from_utf8(bytes).map_err(|e| Error::Utf8(e.utf8_error()))
}

unsafe extern "C" {
    #[link_name = "free"]
    fn libc_free(ptr: *mut std::ffi::c_void);
}

impl Drop for Root {
    fn drop(&mut self) {
        // SAFETY: we own self.inner and this is the only Drop path; libnvme's
        // tree-free recursively releases all child handles.
        unsafe { nvme_free_tree(self.inner.as_ptr()) };
    }
}

impl std::fmt::Debug for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Root")
            .field("inner", &self.inner.as_ptr())
            .finish()
    }
}
