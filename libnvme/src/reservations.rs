//! NVMe Reservation commands (Acquire / Register / Release / Report).
//!
//! Reservations let multiple host controllers coordinate access to a shared
//! namespace — the NVMe analogue of SCSI Persistent Reservations. Typical
//! use is multi-host NVMe-oF clustering where two or more hosts back the
//! same namespace and need to negotiate which one has write access.
//!
//! All four commands are namespace-scoped. Build them via the methods on
//! [`Namespace`] (`reservation_acquire`, `reservation_register`,
//! `reservation_release`, `reservation_report`) and call `.execute()`.
//!
//! See NVMe spec §8.19 for the full semantics of each action.

use std::ffi::c_void;

use libnvme_sys::{
    nvme_resv_acquire, nvme_resv_acquire_args, nvme_resv_register, nvme_resv_register_args,
    nvme_resv_release, nvme_resv_release_args, nvme_resv_report, nvme_resv_report_args,
};

use crate::error::{check_ret, Error, Result};
use crate::namespace::Namespace;

/// The raw Reservation Status data structure (`nvme_resv_status`),
/// re-exported from `libnvme-sys` so callers of
/// [`ReservationReport::execute`] / [`ReservationReport::execute_to_vec`]
/// can name and cast to it without taking a direct `libnvme-sys`
/// dependency.
///
/// This is a `#[repr(C)]` struct with a variable-length trailing array of
/// per-controller entries; a higher-level decoded view may be added in a
/// future (additive) release. The `pub use` also brings it into scope for
/// this module's internal use (the buffer-size checks in `execute`).
pub use libnvme_sys::nvme_resv_status;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Reservation type (rtype) — sets the access semantics for the reservation
/// holder vs. registered hosts vs. non-registered hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u8)]
pub enum ReservationType {
    /// Write Exclusive — only the holder can write; all hosts can read.
    WriteExclusive = 1,
    /// Exclusive Access — only the holder can read or write.
    ExclusiveAccess = 2,
    /// Write Exclusive, Registrants Only.
    WriteExclusiveRegistrantsOnly = 3,
    /// Exclusive Access, Registrants Only.
    ExclusiveAccessRegistrantsOnly = 4,
    /// Write Exclusive, All Registrants.
    WriteExclusiveAllRegistrants = 5,
    /// Exclusive Access, All Registrants.
    ExclusiveAccessAllRegistrants = 6,
}

impl ReservationType {
    fn as_raw(self) -> u32 {
        self as u8 as u32
    }
}

/// Reservation Acquire action (racqa).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u8)]
pub enum ReservationAcquireAction {
    /// Acquire the reservation.
    #[default]
    Acquire = 0,
    /// Preempt the current holder.
    Preempt = 1,
    /// Preempt and abort outstanding commands from the current holder.
    PreemptAndAbort = 2,
}

impl ReservationAcquireAction {
    fn as_raw(self) -> u32 {
        self as u8 as u32
    }
}

/// Reservation Register action (rrega).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u8)]
pub enum ReservationRegisterAction {
    /// Register a new reservation key.
    #[default]
    Register = 0,
    /// Unregister this host's reservation key.
    Unregister = 1,
    /// Replace the existing reservation key.
    Replace = 2,
}

impl ReservationRegisterAction {
    fn as_raw(self) -> u32 {
        self as u8 as u32
    }
}

/// Reservation Release action (rrela).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u8)]
pub enum ReservationReleaseAction {
    /// Release the reservation.
    #[default]
    Release = 0,
    /// Clear all reservations and registrations.
    Clear = 1,
}

impl ReservationReleaseAction {
    fn as_raw(self) -> u32 {
        self as u8 as u32
    }
}

/// Change-Persist-Through-Power-Loss (cptpl) for Reservation Register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u8)]
pub enum PtplChange {
    /// No change to PTPL state.
    #[default]
    NoChange = 0,
    /// Reservations and registrations are released on power-on.
    Clear = 2,
    /// Reservations and registrations persist across power loss.
    Persist = 3,
}

impl PtplChange {
    fn as_raw(self) -> u32 {
        self as u8 as u32
    }
}

// ---------------------------------------------------------------------------
// Acquire
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::reservation_acquire`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct ReservationAcquire<'a, 'r> {
    ns: &'a Namespace<'r>,
    crkey: u64,
    nrkey: u64,
    rtype: ReservationType,
    action: ReservationAcquireAction,
    iekey: bool,
    timeout_ms: u32,
}

impl<'a, 'r> ReservationAcquire<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>) -> Self {
        ReservationAcquire {
            ns,
            crkey: 0,
            nrkey: 0,
            rtype: ReservationType::WriteExclusive,
            action: ReservationAcquireAction::default(),
            iekey: false,
            timeout_ms: 0,
        }
    }

    /// Current Reservation Key associated with this host.
    pub fn key(mut self, crkey: u64) -> Self {
        self.crkey = crkey;
        self
    }

    /// New Reservation Key — used only when `action` is
    /// [`ReservationAcquireAction::Preempt`] or `PreemptAndAbort`.
    pub fn new_key(mut self, nrkey: u64) -> Self {
        self.nrkey = nrkey;
        self
    }

    /// Reservation type to acquire.
    pub fn rtype(mut self, rtype: ReservationType) -> Self {
        self.rtype = rtype;
        self
    }

    /// Which acquire action to perform.
    pub fn action(mut self, action: ReservationAcquireAction) -> Self {
        self.action = action;
        self
    }

    /// Ignore Existing Key — bypass the host-registered key check.
    pub fn ignore_existing_key(mut self) -> Self {
        self.iekey = true;
        self
    }

    /// Per-command timeout in milliseconds.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Reservation Acquire command.
    pub fn execute(self) -> Result<u32> {
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = nvme_resv_acquire_args {
            crkey: self.crkey,
            nrkey: self.nrkey,
            result: &mut result,
            args_size: std::mem::size_of::<nvme_resv_acquire_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            rtype: self.rtype.as_raw(),
            racqa: self.action.as_raw(),
            iekey: self.iekey,
        };
        // SAFETY: args is fully-initialized on the stack; fd is a valid
        // file descriptor for this namespace.
        let ret = unsafe { nvme_resv_acquire(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Register
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::reservation_register`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct ReservationRegister<'a, 'r> {
    ns: &'a Namespace<'r>,
    crkey: u64,
    nrkey: u64,
    action: ReservationRegisterAction,
    cptpl: PtplChange,
    iekey: bool,
    timeout_ms: u32,
}

impl<'a, 'r> ReservationRegister<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>) -> Self {
        ReservationRegister {
            ns,
            crkey: 0,
            nrkey: 0,
            action: ReservationRegisterAction::default(),
            cptpl: PtplChange::default(),
            iekey: false,
            timeout_ms: 0,
        }
    }

    /// Current Reservation Key — only required when replacing or
    /// unregistering an existing key.
    pub fn key(mut self, crkey: u64) -> Self {
        self.crkey = crkey;
        self
    }

    /// New Reservation Key to register (or replace with).
    pub fn new_key(mut self, nrkey: u64) -> Self {
        self.nrkey = nrkey;
        self
    }

    /// Which register action to perform.
    pub fn action(mut self, action: ReservationRegisterAction) -> Self {
        self.action = action;
        self
    }

    /// Change Persist-Through-Power-Loss state.
    pub fn ptpl(mut self, cptpl: PtplChange) -> Self {
        self.cptpl = cptpl;
        self
    }

    /// Ignore Existing Key — bypass the host-registered key check.
    pub fn ignore_existing_key(mut self) -> Self {
        self.iekey = true;
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Reservation Register command.
    pub fn execute(self) -> Result<u32> {
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = nvme_resv_register_args {
            crkey: self.crkey,
            nrkey: self.nrkey,
            result: &mut result,
            args_size: std::mem::size_of::<nvme_resv_register_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            rrega: self.action.as_raw(),
            cptpl: self.cptpl.as_raw(),
            iekey: self.iekey,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid.
        let ret = unsafe { nvme_resv_register(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Release
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::reservation_release`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct ReservationRelease<'a, 'r> {
    ns: &'a Namespace<'r>,
    crkey: u64,
    rtype: ReservationType,
    action: ReservationReleaseAction,
    iekey: bool,
    timeout_ms: u32,
}

impl<'a, 'r> ReservationRelease<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>) -> Self {
        ReservationRelease {
            ns,
            crkey: 0,
            rtype: ReservationType::WriteExclusive,
            action: ReservationReleaseAction::default(),
            iekey: false,
            timeout_ms: 0,
        }
    }

    /// Current Reservation Key being released.
    pub fn key(mut self, crkey: u64) -> Self {
        self.crkey = crkey;
        self
    }

    /// Reservation type being released (must match the held type).
    pub fn rtype(mut self, rtype: ReservationType) -> Self {
        self.rtype = rtype;
        self
    }

    /// Release action.
    pub fn action(mut self, action: ReservationReleaseAction) -> Self {
        self.action = action;
        self
    }

    /// Ignore Existing Key.
    pub fn ignore_existing_key(mut self) -> Self {
        self.iekey = true;
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Reservation Release command.
    pub fn execute(self) -> Result<u32> {
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = nvme_resv_release_args {
            crkey: self.crkey,
            result: &mut result,
            args_size: std::mem::size_of::<nvme_resv_release_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            rtype: self.rtype.as_raw(),
            rrela: self.action.as_raw(),
            iekey: self.iekey,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid.
        let ret = unsafe { nvme_resv_release(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::reservation_report`].
///
/// Reads into a caller-supplied buffer. The buffer holds an
/// `nvme_resv_status` header followed by a variable number of registered
/// controller entries; callers cast to the bindgen type or use the
/// convenience [`ReservationReport::execute_to_vec`] for a default-sized
/// allocation.
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct ReservationReport<'a, 'r> {
    ns: &'a Namespace<'r>,
    buf: Option<&'a mut [u8]>,
    eds: bool,
    timeout_ms: u32,
}

impl<'a, 'r> ReservationReport<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>) -> Self {
        ReservationReport {
            ns,
            buf: None,
            eds: false,
            timeout_ms: 0,
        }
    }

    /// Read the report into this buffer. Must be at least
    /// `size_of::<nvme_resv_status>()` bytes; bigger buffers will read
    /// the variable controller-entry tail too. Required before
    /// [`Self::execute`].
    pub fn buffer(mut self, buf: &'a mut [u8]) -> Self {
        self.buf = Some(buf);
        self
    }

    /// Request the Extended Data Structure (64-bit reservation keys
    /// instead of the legacy 16-bit registrant IDs).
    pub fn extended(mut self) -> Self {
        self.eds = true;
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Execute against the user-provided buffer.
    pub fn execute(self) -> Result<u32> {
        let buf = self.buf.ok_or(Error::invalid_argument(
            "ReservationReport requires a buffer via .buffer()",
        ))?;
        if buf.len() < std::mem::size_of::<nvme_resv_status>() {
            return Err(Error::invalid_argument(
                "ReservationReport buffer smaller than nvme_resv_status header",
            ));
        }
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = nvme_resv_report_args {
            result: &mut result,
            report: buf.as_mut_ptr() as *mut nvme_resv_status,
            args_size: std::mem::size_of::<nvme_resv_report_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            len: buf.len() as u32,
            eds: self.eds,
        };
        // SAFETY: args is fully-initialized on the stack; report points
        // to a caller-owned buffer of `len` bytes (verified above).
        let ret = unsafe { nvme_resv_report(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }

    /// Convenience: allocate a 4 KiB buffer and execute. Returns the raw
    /// bytes; reinterpret the leading bytes as [`nvme_resv_status`]
    /// (re-exported from this crate) to decode the report.
    pub fn execute_to_vec(self) -> Result<Vec<u8>> {
        let eds = self.eds;
        let timeout_ms = self.timeout_ms;
        let ns = self.ns;
        let mut buf = vec![0u8; 4096];
        ReservationReport {
            ns,
            buf: Some(&mut buf),
            eds,
            timeout_ms,
        }
        .execute()?;
        Ok(buf)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ns_fd(ns: &Namespace<'_>) -> Result<std::os::raw::c_int> {
    // SAFETY: ns.raw_handle() is a non-null nvme_ns_t tied to the Root tree
    // via 'r; libnvme opens the device lazily and returns -1 on failure.
    let fd = unsafe { libnvme_sys::nvme_ns_get_fd(ns.raw_handle()) };
    if fd < 0 {
        return Err(Error::Os(std::io::Error::last_os_error()));
    }
    Ok(fd)
}

// Silence unused-import lints when `c_void` is only referenced through
// type-system equivalences (kept for future buffer-bearing report variants).
#[allow(dead_code)]
const _C_VOID_HINT: Option<*const c_void> = None;
