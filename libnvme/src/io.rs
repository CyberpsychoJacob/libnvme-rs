//! Namespace-scoped NVM I/O commands.
//!
//! Wraps libnvme's I/O command surface: Read, Write, Compare, Verify, Write
//! Zeroes, Write Uncorrectable, Flush, Dataset Management (DSM), and Copy.
//!
//! Each command (except [`Namespace::flush`]) returns a builder that lets you
//! tune optional fields — Force Unit Access, Limited Retry, end-to-end
//! Protection Information, application/reference tags, dataset-management
//! hints, directive specifiers, and the per-command timeout — before calling
//! [`Read::execute`] et al.
//!
//! Block counts in this API are **1-based**: `nlb = 1` reads a single LBA.
//! libnvme's underlying argument struct uses the NVMe-spec 0's-based encoding;
//! we subtract one for you. The maximum value is therefore `u16::MAX as u32 + 1`
//! (`65_536`) LBAs per command.
//!
//! # Example
//!
//! ```no_run
//! use libnvme::Root;
//!
//! let root = Root::scan()?;
//! let host = root.hosts().next().ok_or("no host")?;
//! let subsys = host.subsystems().next().ok_or("no subsys")?;
//! let ctrl = subsys.controllers().next().ok_or("no ctrl")?;
//! let ns = ctrl.namespaces().next().ok_or("no ns")?;
//!
//! // Read the first 8 LBAs into an owned Vec.
//! let buf = ns.read_to_vec(0, 8)?;
//! println!("first byte: 0x{:02x}", buf[0]);
//!
//! // Write with FUA + limited retry.
//! ns.write(0, 1, &[0u8; 4096]).fua().limited_retry().execute()?;
//!
//! // Trim 64 MiB starting at LBA 0.
//! use libnvme::{DsmRange, DsmAttr};
//! ns.dsm(DsmAttr::DEALLOCATE)
//!     .ranges(&[DsmRange::new(0, 16_384)])
//!     .execute()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::ffi::c_void;

use libnvme_sys::{
    nvme_copy, nvme_copy_args, nvme_copy_range, nvme_dsm, nvme_dsm_args, nvme_dsm_range, nvme_io,
    nvme_io_args, nvme_io_passthru, nvme_ns_get_fd,
};

use crate::error::{check_ret, Error, Result};
use crate::namespace::Namespace;

// libnvme opcodes (from `enum nvme_io_opcode`). Bindgen exposes these as
// constants under various integer types depending on bindgen version; cast
// to u8 at use-site.
const OPC_FLUSH: u8 = libnvme_sys::nvme_cmd_flush as u8;
const OPC_WRITE: u8 = libnvme_sys::nvme_cmd_write as u8;
const OPC_READ: u8 = libnvme_sys::nvme_cmd_read as u8;
const OPC_WRITE_UNCOR: u8 = libnvme_sys::nvme_cmd_write_uncor as u8;
const OPC_COMPARE: u8 = libnvme_sys::nvme_cmd_compare as u8;
const OPC_WRITE_ZEROES: u8 = libnvme_sys::nvme_cmd_write_zeroes as u8;
const OPC_VERIFY: u8 = libnvme_sys::nvme_cmd_verify as u8;

/// I/O control-flag bit values for `nvme_io_args.control` (CDW12 upper bits).
///
/// Values are fixed by the NVMe spec, so we hardcode them rather than
/// depending on libnvme exposing each constant. Some (e.g. `NSZ`) were
/// added after libnvme 1.8 and would otherwise break builds on older
/// distros.
///
/// Kept private; the public surface is the chainable builder setters
/// (`.fua()`, `.limited_retry()`, `.check_guard()`, etc.) on each I/O
/// command builder.
#[allow(dead_code)] // some bits are reserved for future builder methods
struct IoControl;

#[allow(dead_code)]
impl IoControl {
    const DTYPE_STREAMS: u16 = 1 << 4;
    const NSZ: u16 = 1 << 7;
    const STC: u16 = 1 << 8;
    const DEAC: u16 = 1 << 9;
    const PRINFO_PRCHK_REF: u16 = 1 << 10;
    const PRINFO_PRCHK_APP: u16 = 1 << 11;
    const PRINFO_PRCHK_GUARD: u16 = 1 << 12;
    const PRINFO_PRACT: u16 = 1 << 13;
    const FUA: u16 = 1 << 14;
    const LR: u16 = 1 << 15;
}

// ---------------------------------------------------------------------------
// Shared option bag
// ---------------------------------------------------------------------------

/// Common, optional knobs shared by every nvme_io_args-based command.
///
/// Kept private; the public builders forward setters into this.
#[derive(Default, Clone)]
struct IoOpts {
    control: u16,
    dsm_hint: u8,
    dspec: u16,
    apptag: u16,
    appmask: u16,
    reftag: u32,
    reftag_u64: u64,
    storage_tag: u64,
    sts: u8,
    pif: u8,
    timeout_ms: u32,
}

impl IoOpts {
    fn apply_to(&self, args: &mut nvme_io_args) {
        args.control = self.control;
        args.dsm = self.dsm_hint;
        args.dspec = self.dspec;
        args.apptag = self.apptag;
        args.appmask = self.appmask;
        args.reftag = self.reftag;
        args.reftag_u64 = self.reftag_u64;
        args.storage_tag = self.storage_tag;
        args.sts = self.sts;
        args.pif = self.pif;
        args.timeout = self.timeout_ms;
    }
}

/// Number of blocks to encode as the NVMe 0's-based `NLB` field.
///
/// Caller passes a 1-based count (`nlb = 1` means one LBA). Spec maxes at
/// 65_536. Returns [`Error::InvalidArgument`] if `nlb == 0` or `nlb > 65_536`.
fn encode_nlb(nlb: u32) -> Result<u16> {
    if nlb == 0 || nlb > 65_536 {
        return Err(invalid(format!("nlb must be 1..=65536, got {nlb}")));
    }
    Ok((nlb - 1) as u16)
}

/// Build an [`Error::InvalidArgument`] from a (possibly formatted) message.
/// All caller-input validation in this module routes through here so the
/// variant is consistent with the rest of the crate.
fn invalid(msg: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::invalid_argument(msg)
}

fn ns_fd(ns: &Namespace<'_>) -> Result<std::os::raw::c_int> {
    // SAFETY: ns_raw(ns) is a non-null nvme_ns_t tied to the Root tree via 'r;
    // libnvme opens the device lazily and returns -1 on failure.
    let fd = unsafe { nvme_ns_get_fd(ns_raw(ns)) };
    if fd < 0 {
        return Err(Error::Os(std::io::Error::last_os_error()));
    }
    Ok(fd)
}

// Reach into Namespace to grab the raw nvme_ns_t handle. Namespace::inner is
// pub(crate); we expose a helper to keep the unsafe minimal in here.
fn ns_raw(ns: &Namespace<'_>) -> libnvme_sys::nvme_ns_t {
    ns.raw_handle()
}

fn check_buf_len(buf_len: usize, nlb: u32, lba_size: u32) -> Result<()> {
    let want = u64::from(nlb) * u64::from(lba_size);
    if buf_len as u64 != want {
        return Err(invalid(format!(
            "buffer length {} bytes does not match nlb*lba_size = {}*{} = {}",
            buf_len, nlb, lba_size, want
        )));
    }
    Ok(())
}

/// Build a partly-populated [`nvme_io_args`] with the mandatory fields set.
///
/// Uses `Default::default()` (bindgen derives it because
/// `libnvme-sys/build.rs` enables `.derive_default(true)`) rather than
/// `mem::zeroed`. Today the two are equivalent, but if libnvme adds a
/// field requiring a non-zero default (e.g. a length sentinel meaning
/// "infer"), `Default` picks it up automatically; `zeroed` would
/// silently produce wrong behavior.
fn base_args(
    fd: std::os::raw::c_int,
    nsid: u32,
    slba: u64,
    nlb_enc: u16,
    data: *mut c_void,
    data_len: u32,
) -> nvme_io_args {
    nvme_io_args {
        args_size: std::mem::size_of::<nvme_io_args>() as i32,
        fd,
        nsid,
        slba,
        nlb: nlb_enc,
        data,
        data_len,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Read / Write / Compare (data-bearing)
// ---------------------------------------------------------------------------

macro_rules! io_data_setters {
    () => {
        /// Force Unit Access — bypass volatile write cache for this command.
        pub fn fua(mut self) -> Self {
            self.opts.control |= IoControl::FUA;
            self
        }

        /// Limited Retry — controller should bound retry effort on error.
        pub fn limited_retry(mut self) -> Self {
            self.opts.control |= IoControl::LR;
            self
        }

        /// Set Protection Information Action (PRACT). Strips/inserts PI bytes
        /// per the namespace format's protection setting.
        pub fn protection_action(mut self) -> Self {
            self.opts.control |= IoControl::PRINFO_PRACT;
            self
        }

        /// Check Reference Tag (PRCHK.REF) — enable PI ref-tag check.
        pub fn check_reftag(mut self) -> Self {
            self.opts.control |= IoControl::PRINFO_PRCHK_REF;
            self
        }

        /// Check Application Tag (PRCHK.APP).
        pub fn check_apptag(mut self) -> Self {
            self.opts.control |= IoControl::PRINFO_PRCHK_APP;
            self
        }

        /// Check Guard (PRCHK.GUARD) — enable PI CRC check.
        pub fn check_guard(mut self) -> Self {
            self.opts.control |= IoControl::PRINFO_PRCHK_GUARD;
            self
        }

        /// Expected initial Logical Block Reference Tag (ILBRT, 32-bit).
        pub fn ref_tag(mut self, tag: u32) -> Self {
            self.opts.reftag = tag;
            self
        }

        /// Expected initial Logical Block Reference Tag (64-bit variant —
        /// enhanced PI namespaces).
        pub fn ref_tag_u64(mut self, tag: u64) -> Self {
            self.opts.reftag_u64 = tag;
            self
        }

        /// Expected Logical Block Application Tag.
        pub fn app_tag(mut self, tag: u16) -> Self {
            self.opts.apptag = tag;
            self
        }

        /// Logical Block Application Tag Mask.
        pub fn app_mask(mut self, mask: u16) -> Self {
            self.opts.appmask = mask;
            self
        }

        /// Dataset Management hint — a combination of frequency/latency/
        /// access-pattern bits per NVMe `nvme_io_dsm_flags`.
        pub fn dataset_mgmt(mut self, dsm: u8) -> Self {
            self.opts.dsm_hint = dsm;
            self
        }

        /// Directive Specific value (DSPEC) — only meaningful when a
        /// directive is in use (e.g. streams).
        pub fn directive(mut self, dspec: u16) -> Self {
            self.opts.dspec = dspec;
            self
        }

        /// Use the streams directive type for this command.
        pub fn streams(mut self) -> Self {
            self.opts.control |= IoControl::DTYPE_STREAMS;
            self
        }

        /// Storage Tag (variable-size, packed into CDW2/CDW3 — for enhanced
        /// protection-info namespaces).
        pub fn storage_tag(mut self, tag: u64) -> Self {
            self.opts.storage_tag = tag;
            self
        }

        /// Enable Storage Tag Check (STC).
        pub fn check_storage_tag(mut self) -> Self {
            self.opts.control |= IoControl::STC;
            self
        }

        /// Storage tag size in bits — must match the namespace's Extended
        /// LBA Format. Default `0` lets libnvme use the namespace setting.
        pub fn storage_tag_size(mut self, sts: u8) -> Self {
            self.opts.sts = sts;
            self
        }

        /// Protection Information Format — must match the namespace's
        /// Extended LBA Format. Default `0` (16b guard).
        pub fn pi_format(mut self, pif: u8) -> Self {
            self.opts.pif = pif;
            self
        }

        /// Per-command timeout in milliseconds. `0` lets libnvme use the
        /// default.
        pub fn timeout_ms(mut self, ms: u32) -> Self {
            self.opts.timeout_ms = ms;
            self
        }
    };
}

/// Builder returned by [`Namespace::read`].
///
/// Fills `data` from device into the supplied buffer; `data.len()` must equal
/// `nlb * lba_size`.
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Read<'a, 'r> {
    ns: &'a Namespace<'r>,
    slba: u64,
    nlb: u32,
    data: &'a mut [u8],
    metadata: Option<&'a mut [u8]>,
    opts: IoOpts,
}

impl<'a, 'r> Read<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, slba: u64, nlb: u32, data: &'a mut [u8]) -> Self {
        Read {
            ns,
            slba,
            nlb,
            data,
            metadata: None,
            opts: IoOpts::default(),
        }
    }

    /// Attach a metadata buffer (separate from data). Length must match
    /// `nlb * meta_size`.
    pub fn metadata(mut self, md: &'a mut [u8]) -> Self {
        self.metadata = Some(md);
        self
    }

    io_data_setters!();

    /// Issue the Read command. Returns the result-dword reported by the
    /// controller (`CDW0` of the CQE).
    pub fn execute(mut self) -> Result<u32> {
        check_buf_len(self.data.len(), self.nlb, self.ns.lba_size())?;
        let nlb_enc = encode_nlb(self.nlb)?;
        let fd = ns_fd(self.ns)?;
        let data_ptr = self.data.as_mut_ptr() as *mut c_void;
        let data_len = self.data.len() as u32;
        let mut result: u32 = 0;
        let mut args = base_args(fd, self.ns.nsid(), self.slba, nlb_enc, data_ptr, data_len);
        if let Some(md) = self.metadata.as_deref_mut() {
            args.metadata = md.as_mut_ptr() as *mut c_void;
            args.metadata_len = md.len() as u32;
        }
        args.result = &mut result;
        self.opts.apply_to(&mut args);
        // SAFETY: args is fully-initialized on the stack; fd is valid; data
        // points into self.data which lives for the duration of the call;
        // metadata (if any) is alive likewise; result is a valid &mut u32.
        let ret = unsafe { nvme_io(&mut args, OPC_READ) };
        check_ret(ret)?;
        Ok(result)
    }
}

/// Builder returned by [`Namespace::write`].
///
/// Sends bytes from `data` to the device; `data.len()` must equal
/// `nlb * lba_size`.
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Write<'a, 'r> {
    ns: &'a Namespace<'r>,
    slba: u64,
    nlb: u32,
    data: &'a [u8],
    metadata: Option<&'a [u8]>,
    opts: IoOpts,
}

impl<'a, 'r> Write<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, slba: u64, nlb: u32, data: &'a [u8]) -> Self {
        Write {
            ns,
            slba,
            nlb,
            data,
            metadata: None,
            opts: IoOpts::default(),
        }
    }

    /// Attach a metadata buffer (separate from data).
    pub fn metadata(mut self, md: &'a [u8]) -> Self {
        self.metadata = Some(md);
        self
    }

    io_data_setters!();

    /// Issue the Write command. Returns the controller's CQE result dword.
    pub fn execute(self) -> Result<u32> {
        check_buf_len(self.data.len(), self.nlb, self.ns.lba_size())?;
        let nlb_enc = encode_nlb(self.nlb)?;
        let fd = ns_fd(self.ns)?;
        // libnvme's nvme_io_args.data is `void *`; for write it is read-only
        // on the host side. The cast to `*mut` is a C-API conformance cast,
        // not a real mutability claim.
        let data_ptr = self.data.as_ptr() as *mut c_void;
        let data_len = self.data.len() as u32;
        let mut result: u32 = 0;
        let mut args = base_args(fd, self.ns.nsid(), self.slba, nlb_enc, data_ptr, data_len);
        if let Some(md) = self.metadata {
            args.metadata = md.as_ptr() as *mut c_void;
            args.metadata_len = md.len() as u32;
        }
        args.result = &mut result;
        self.opts.apply_to(&mut args);
        // SAFETY: args is fully-initialized on the stack; fd is valid; data
        // points into self.data which lives for the call (the *mut cast is
        // C-API conformance only — libnvme only reads it for a Write);
        // metadata (if any) is alive likewise.
        let ret = unsafe { nvme_io(&mut args, OPC_WRITE) };
        check_ret(ret)?;
        Ok(result)
    }
}

/// Builder returned by [`Namespace::compare`].
///
/// Compares stored LBAs against the supplied host buffer. The controller
/// returns NVMe status `0x85` (Compare Failure) via [`Error::Nvme`] if any
/// byte differs.
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Compare<'a, 'r> {
    ns: &'a Namespace<'r>,
    slba: u64,
    nlb: u32,
    data: &'a [u8],
    metadata: Option<&'a [u8]>,
    opts: IoOpts,
}

impl<'a, 'r> Compare<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, slba: u64, nlb: u32, data: &'a [u8]) -> Self {
        Compare {
            ns,
            slba,
            nlb,
            data,
            metadata: None,
            opts: IoOpts::default(),
        }
    }

    pub fn metadata(mut self, md: &'a [u8]) -> Self {
        self.metadata = Some(md);
        self
    }

    io_data_setters!();

    /// Issue the Compare command. Returns the controller's CQE result dword.
    pub fn execute(self) -> Result<u32> {
        check_buf_len(self.data.len(), self.nlb, self.ns.lba_size())?;
        let nlb_enc = encode_nlb(self.nlb)?;
        let fd = ns_fd(self.ns)?;
        let data_ptr = self.data.as_ptr() as *mut c_void;
        let data_len = self.data.len() as u32;
        let mut result: u32 = 0;
        let mut args = base_args(fd, self.ns.nsid(), self.slba, nlb_enc, data_ptr, data_len);
        if let Some(md) = self.metadata {
            args.metadata = md.as_ptr() as *mut c_void;
            args.metadata_len = md.len() as u32;
        }
        args.result = &mut result;
        self.opts.apply_to(&mut args);
        // SAFETY: args is fully-initialized on the stack; fd is valid; data
        // points into self.data which lives for the call (the *mut cast is
        // C-API conformance only — libnvme only reads it for Compare);
        // metadata (if any) is alive likewise.
        let ret = unsafe { nvme_io(&mut args, OPC_COMPARE) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Verify / WriteZeroes / WriteUncorrectable (no host buffer)
// ---------------------------------------------------------------------------

macro_rules! io_nodata_setters {
    () => {
        /// Force Unit Access — bypass volatile write cache for this command.
        pub fn fua(mut self) -> Self {
            self.opts.control |= IoControl::FUA;
            self
        }

        /// Limited Retry — controller should bound retry effort on error.
        pub fn limited_retry(mut self) -> Self {
            self.opts.control |= IoControl::LR;
            self
        }

        /// Set Protection Information Action (PRACT).
        pub fn protection_action(mut self) -> Self {
            self.opts.control |= IoControl::PRINFO_PRACT;
            self
        }

        /// Check Reference Tag (PRCHK.REF) — enable PI ref-tag check.
        pub fn check_reftag(mut self) -> Self {
            self.opts.control |= IoControl::PRINFO_PRCHK_REF;
            self
        }

        /// Check Application Tag (PRCHK.APP).
        pub fn check_apptag(mut self) -> Self {
            self.opts.control |= IoControl::PRINFO_PRCHK_APP;
            self
        }

        /// Check Guard (PRCHK.GUARD) — enable PI CRC check.
        pub fn check_guard(mut self) -> Self {
            self.opts.control |= IoControl::PRINFO_PRCHK_GUARD;
            self
        }

        /// Expected initial Logical Block Reference Tag (ILBRT, 32-bit).
        pub fn ref_tag(mut self, tag: u32) -> Self {
            self.opts.reftag = tag;
            self
        }

        /// Expected Logical Block Application Tag.
        pub fn app_tag(mut self, tag: u16) -> Self {
            self.opts.apptag = tag;
            self
        }

        /// Logical Block Application Tag Mask.
        pub fn app_mask(mut self, mask: u16) -> Self {
            self.opts.appmask = mask;
            self
        }

        /// Per-command timeout in milliseconds. `0` lets libnvme use the
        /// default.
        pub fn timeout_ms(mut self, ms: u32) -> Self {
            self.opts.timeout_ms = ms;
            self
        }
    };
}

/// Builder returned by [`Namespace::verify`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Verify<'a, 'r> {
    ns: &'a Namespace<'r>,
    slba: u64,
    nlb: u32,
    opts: IoOpts,
}

impl<'a, 'r> Verify<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, slba: u64, nlb: u32) -> Self {
        Verify {
            ns,
            slba,
            nlb,
            opts: IoOpts::default(),
        }
    }

    io_nodata_setters!();

    /// Issue the Verify command. Returns the controller's CQE result dword.
    pub fn execute(self) -> Result<u32> {
        let nlb_enc = encode_nlb(self.nlb)?;
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = base_args(
            fd,
            self.ns.nsid(),
            self.slba,
            nlb_enc,
            std::ptr::null_mut(),
            0,
        );
        args.result = &mut result;
        self.opts.apply_to(&mut args);
        // SAFETY: args is fully-initialized on the stack; fd is valid; no
        // data/metadata buffers are used (args.data is NULL, len 0).
        let ret = unsafe { nvme_io(&mut args, OPC_VERIFY) };
        check_ret(ret)?;
        Ok(result)
    }
}

/// Builder returned by [`Namespace::write_zeroes`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct WriteZeroes<'a, 'r> {
    ns: &'a Namespace<'r>,
    slba: u64,
    nlb: u32,
    opts: IoOpts,
}

impl<'a, 'r> WriteZeroes<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, slba: u64, nlb: u32) -> Self {
        WriteZeroes {
            ns,
            slba,
            nlb,
            opts: IoOpts::default(),
        }
    }

    io_nodata_setters!();

    /// Set Deallocate (DEAC) — after zeroing, the LBA range is unmapped.
    pub fn deallocate(mut self) -> Self {
        self.opts.control |= IoControl::DEAC;
        self
    }

    /// No-Deallocate after Successful Zeroing (NSZ — NVMe 2.0+).
    pub fn no_deallocate_after_zero(mut self) -> Self {
        self.opts.control |= IoControl::NSZ;
        self
    }

    /// Issue the Write Zeroes command. Returns the controller's CQE result
    /// dword.
    pub fn execute(self) -> Result<u32> {
        let nlb_enc = encode_nlb(self.nlb)?;
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = base_args(
            fd,
            self.ns.nsid(),
            self.slba,
            nlb_enc,
            std::ptr::null_mut(),
            0,
        );
        args.result = &mut result;
        self.opts.apply_to(&mut args);
        // SAFETY: args is fully-initialized on the stack; fd is valid; no
        // data/metadata buffers are used (args.data is NULL, len 0).
        let ret = unsafe { nvme_io(&mut args, OPC_WRITE_ZEROES) };
        check_ret(ret)?;
        Ok(result)
    }
}

/// Builder returned by [`Namespace::write_uncorrectable`].
///
/// # Warning
///
/// **Destructive.** Marks the LBA range so that subsequent reads return
/// Unrecovered Read Error. Existing data in the range becomes unreadable
/// until those LBAs are rewritten (either via [`Namespace::write`] or
/// [`Namespace::write_zeroes`]). Use only for fault-injection testing
/// or to deliberately invalidate data; never on production drives
/// without explicit intent.
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct WriteUncorrectable<'a, 'r> {
    ns: &'a Namespace<'r>,
    slba: u64,
    nlb: u32,
    opts: IoOpts,
}

impl<'a, 'r> WriteUncorrectable<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, slba: u64, nlb: u32) -> Self {
        WriteUncorrectable {
            ns,
            slba,
            nlb,
            opts: IoOpts::default(),
        }
    }

    /// Per-command timeout in milliseconds.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.opts.timeout_ms = ms;
        self
    }

    /// Issue the Write Uncorrectable command. Returns the controller's CQE
    /// result dword.
    pub fn execute(self) -> Result<u32> {
        let nlb_enc = encode_nlb(self.nlb)?;
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = base_args(
            fd,
            self.ns.nsid(),
            self.slba,
            nlb_enc,
            std::ptr::null_mut(),
            0,
        );
        args.result = &mut result;
        self.opts.apply_to(&mut args);
        // SAFETY: args is fully-initialized on the stack; fd is valid; no
        // data/metadata buffers are used (args.data is NULL, len 0).
        let ret = unsafe { nvme_io(&mut args, OPC_WRITE_UNCOR) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// DSM (Dataset Management)
// ---------------------------------------------------------------------------

/// DSM attribute bits (CDW11). Combine with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DsmAttr(u32);

impl DsmAttr {
    /// Integral Dataset Read.
    pub const INTEGRAL_READ: Self = Self(1 << 0);
    /// Integral Dataset Write.
    pub const INTEGRAL_WRITE: Self = Self(1 << 1);
    /// Deallocate (the well-known TRIM/UNMAP).
    pub const DEALLOCATE: Self = Self(1 << 2);

    /// Construct from a raw mask.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Raw mask.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl std::ops::BitOr for DsmAttr {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for DsmAttr {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// One DSM range entry. NVMe spec: each entry covers up to 4 GiB of LBAs.
///
/// Fields are private — construct via [`DsmRange::new`] (which takes a
/// **1-based** block count, matching the rest of this module's I/O API)
/// and refine with [`DsmRange::with_context`]. This makes it impossible
/// to confuse the public 1-based count with the spec's 0-based wire
/// encoding (which `execute` applies internally).
#[derive(Debug, Clone, Copy)]
pub struct DsmRange {
    /// Context attributes (DSM context-attribute hints).
    context: u32,
    /// Length in logical blocks, stored 0-based per the spec wire format
    /// (`new` subtracts 1 from the caller's 1-based count).
    length: u32,
    /// Starting LBA.
    slba: u64,
}

impl DsmRange {
    /// Construct a range starting at `slba` covering `nlb` logical blocks.
    ///
    /// `nlb` is **1-based** (1 = a single LBA), consistent with the rest of
    /// this module; it is decremented to the spec's 0-based encoding when
    /// the command is submitted. `nlb` of `0` is treated as `1`.
    #[must_use]
    pub fn new(slba: u64, nlb: u32) -> Self {
        DsmRange {
            context: 0,
            length: nlb.saturating_sub(1),
            slba,
        }
    }

    /// Attach DSM context-attribute bits (frequency / latency /
    /// access-pattern hints — see the NVMe spec figure on DSM context
    /// attributes).
    #[must_use]
    pub fn with_context(mut self, context: u32) -> Self {
        self.context = context;
        self
    }
}

/// Builder returned by [`Namespace::dsm`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Dsm<'a, 'r> {
    ns: &'a Namespace<'r>,
    attrs: DsmAttr,
    ranges: Option<&'a [DsmRange]>,
    timeout_ms: u32,
}

impl<'a, 'r> Dsm<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, attrs: DsmAttr) -> Self {
        Dsm {
            ns,
            attrs,
            ranges: None,
            timeout_ms: 0,
        }
    }

    /// Provide the LBA ranges this DSM applies to. The DSM command accepts
    /// up to 256 ranges per submission.
    pub fn ranges(mut self, ranges: &'a [DsmRange]) -> Self {
        self.ranges = Some(ranges);
        self
    }

    /// Per-command timeout in milliseconds. `0` lets libnvme use the default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Dataset Management command. Returns the controller's CQE
    /// result dword.
    pub fn execute(self) -> Result<u32> {
        let ranges = self.ranges.unwrap_or(&[]);
        if ranges.is_empty() {
            return Err(invalid("DSM requires at least one range"));
        }
        if ranges.len() > 256 {
            return Err(invalid(format!(
                "DSM supports at most 256 ranges, got {}",
                ranges.len()
            )));
        }

        // Convert our Rust DsmRange into the libnvme/NVMe wire layout
        // (little-endian). The hardware reads this directly via DMA.
        let mut raw: Vec<nvme_dsm_range> = ranges
            .iter()
            .map(|r| nvme_dsm_range {
                cattr: r.context.to_le(),
                nlb: r.length.to_le(),
                slba: r.slba.to_le(),
            })
            .collect();

        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = nvme_dsm_args {
            result: &mut result,
            dsm: raw.as_mut_ptr(),
            args_size: std::mem::size_of::<nvme_dsm_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            attrs: self.attrs.bits(),
            nr_ranges: raw.len() as u16,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid; raw is
        // alive for the duration of the call and holds nr_ranges entries.
        let ret = unsafe { nvme_dsm(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

/// One source range for the NVMe Copy command (format 0).
#[derive(Debug, Clone, Copy, Default)]
pub struct CopyRange {
    /// Source starting LBA.
    pub slba: u64,
    /// Number of logical blocks. **1-based** in this API; converted to the
    /// spec's 0-based encoding on send.
    pub nlb: u16,
    /// Expected initial logical block reference tag (only meaningful on
    /// PI-formatted namespaces).
    pub eilbrt: u32,
    /// Expected logical block application tag.
    pub elbat: u16,
    /// Expected logical block application tag mask.
    pub elbatm: u16,
}

impl CopyRange {
    /// Construct a source range starting at `slba` covering `nlb` logical
    /// blocks (1-based).
    pub fn new(slba: u64, nlb: u16) -> Self {
        CopyRange {
            slba,
            nlb,
            ..Default::default()
        }
    }

    /// Expected initial logical block reference tag (EILBRT).
    pub fn ref_tag(mut self, eilbrt: u32) -> Self {
        self.eilbrt = eilbrt;
        self
    }

    /// Expected logical block application tag (ELBAT) + mask (ELBATM).
    pub fn app_tag(mut self, elbat: u16, mask: u16) -> Self {
        self.elbat = elbat;
        self.elbatm = mask;
        self
    }
}

/// Builder returned by [`Namespace::copy`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Copy<'a, 'r> {
    ns: &'a Namespace<'r>,
    sdlba: u64,
    ranges: &'a [CopyRange],
    ilbrt: u32,
    ilbrt_u64: u64,
    lbat: u16,
    lbatm: u16,
    prinfor: u8,
    prinfow: u8,
    fua: bool,
    lr: bool,
    dtype: u8,
    dspec: u16,
    format: u8,
    timeout_ms: u32,
}

impl<'a, 'r> Copy<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, sdlba: u64, ranges: &'a [CopyRange]) -> Self {
        Copy {
            ns,
            sdlba,
            ranges,
            ilbrt: 0,
            ilbrt_u64: 0,
            lbat: 0,
            lbatm: 0,
            prinfor: 0,
            prinfow: 0,
            fua: false,
            lr: false,
            dtype: 0,
            dspec: 0,
            format: 0,
            timeout_ms: 0,
        }
    }

    /// Force Unit Access for the destination writes.
    pub fn fua(mut self) -> Self {
        self.fua = true;
        self
    }

    /// Limited Retry for the destination writes.
    pub fn limited_retry(mut self) -> Self {
        self.lr = true;
        self
    }

    /// Destination initial logical block reference tag (32-bit).
    pub fn dest_ref_tag(mut self, tag: u32) -> Self {
        self.ilbrt = tag;
        self
    }

    /// Destination initial logical block reference tag (64-bit, enhanced PI).
    pub fn dest_ref_tag_u64(mut self, tag: u64) -> Self {
        self.ilbrt_u64 = tag;
        self
    }

    /// Destination logical-block application tag + mask.
    pub fn dest_app_tag(mut self, tag: u16, mask: u16) -> Self {
        self.lbat = tag;
        self.lbatm = mask;
        self
    }

    /// PI field for reads from the source.
    pub fn prinfo_read(mut self, prinfo: u8) -> Self {
        self.prinfor = prinfo;
        self
    }

    /// PI field for writes to the destination.
    pub fn prinfo_write(mut self, prinfo: u8) -> Self {
        self.prinfow = prinfo;
        self
    }

    /// Source-range descriptor format. `0` (default) selects the 32-byte
    /// format; `1` selects the 40-byte enhanced-PI format. Must match the
    /// `CopyRange` shape you provided.
    pub fn descriptor_format(mut self, format: u8) -> Self {
        self.format = format;
        self
    }

    /// Directive type + specifier (e.g. streams).
    pub fn directive(mut self, dtype: u8, dspec: u16) -> Self {
        self.dtype = dtype;
        self.dspec = dspec;
        self
    }

    /// Per-command timeout in milliseconds. `0` lets libnvme use the default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Copy command. Returns the controller's CQE result dword.
    pub fn execute(self) -> Result<u32> {
        if self.ranges.is_empty() {
            return Err(invalid("Copy requires at least one source range"));
        }
        if self.ranges.len() > 128 {
            return Err(invalid(format!(
                "Copy supports at most 128 ranges, got {}",
                self.ranges.len()
            )));
        }

        // Build the wire-format source-range array. NVMe spec is LE.
        let mut raw: Vec<nvme_copy_range> = self
            .ranges
            .iter()
            .map(|r| {
                let nlb_enc = r.nlb.saturating_sub(1);
                nvme_copy_range {
                    rsvd0: [0u8; 8],
                    slba: r.slba.to_le(),
                    nlb: nlb_enc.to_le(),
                    rsvd18: [0u8; 6],
                    eilbrt: r.eilbrt.to_le(),
                    elbat: r.elbat.to_le(),
                    elbatm: r.elbatm.to_le(),
                }
            })
            .collect();

        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = nvme_copy_args {
            sdlba: self.sdlba,
            result: &mut result,
            copy: raw.as_mut_ptr(),
            args_size: std::mem::size_of::<nvme_copy_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            ilbrt: self.ilbrt,
            lr: self.lr as i32,
            fua: self.fua as i32,
            nr: (raw.len() - 1) as u16, // NR is 0-based per spec
            dspec: self.dspec,
            lbatm: self.lbatm,
            lbat: self.lbat,
            prinfor: self.prinfor,
            prinfow: self.prinfow,
            dtype: self.dtype,
            format: self.format,
            ilbrt_u64: self.ilbrt_u64,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid; raw is
        // alive for the duration of the call and holds nr+1 source ranges.
        let ret = unsafe { nvme_copy(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Flush (a passthru command — the libnvme inline wrapper builds one too)
// ---------------------------------------------------------------------------

/// Issue a Flush command on this namespace. No options — see
/// [`Namespace::flush`] for the public entry point.
pub(crate) fn flush(ns: &Namespace<'_>) -> Result<()> {
    let fd = ns_fd(ns)?;
    // Replicate `static inline nvme_flush` from libnvme/ioctl.h: build a
    // minimal passthru with opcode=0x00, nsid set. We funnel through
    // nvme_admin_passthru's signature would be wrong (Flush is an I/O
    // opcode), so use io_passthru via libnvme_sys.
    let mut result: u32 = 0;
    // SAFETY: fd is a valid file descriptor for this namespace; the passthru
    // call uses NULL data/metadata pointers with zero lengths (Flush carries
    // no payload); result is a valid &mut u32 alive for the call.
    let ret = unsafe {
        nvme_io_passthru(
            fd,
            OPC_FLUSH,
            0,
            0,
            ns.nsid(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            0,
            &mut result,
        )
    };
    check_ret(ret)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- encode_nlb: the 1-based -> 0-based NLB conversion --------------

    #[test]
    fn encode_nlb_is_one_based() {
        // 1 LBA encodes to the spec's 0-based 0; 2 -> 1; etc.
        assert_eq!(encode_nlb(1).unwrap(), 0);
        assert_eq!(encode_nlb(2).unwrap(), 1);
        assert_eq!(encode_nlb(65_536).unwrap(), 65_535);
    }

    #[test]
    fn encode_nlb_rejects_zero_and_overflow() {
        assert!(matches!(encode_nlb(0), Err(Error::InvalidArgument(_))));
        assert!(matches!(encode_nlb(65_537), Err(Error::InvalidArgument(_))));
        assert!(matches!(
            encode_nlb(u32::MAX),
            Err(Error::InvalidArgument(_))
        ));
    }

    // -- IoControl: CDW12 control-flag bit layout (NVMe spec fixed) -----

    #[test]
    fn io_control_bits_match_spec() {
        assert_eq!(IoControl::DTYPE_STREAMS, 1 << 4);
        assert_eq!(IoControl::NSZ, 1 << 7);
        assert_eq!(IoControl::STC, 1 << 8);
        assert_eq!(IoControl::DEAC, 1 << 9);
        assert_eq!(IoControl::PRINFO_PRCHK_REF, 1 << 10);
        assert_eq!(IoControl::PRINFO_PRCHK_APP, 1 << 11);
        assert_eq!(IoControl::PRINFO_PRCHK_GUARD, 1 << 12);
        assert_eq!(IoControl::PRINFO_PRACT, 1 << 13);
        assert_eq!(IoControl::FUA, 1 << 14);
        assert_eq!(IoControl::LR, 1 << 15);
    }

    // -- DsmAttr: CDW11 attribute bits + combination -------------------

    #[test]
    fn dsm_attr_bits_and_combination() {
        assert_eq!(DsmAttr::INTEGRAL_READ.bits(), 1 << 0);
        assert_eq!(DsmAttr::INTEGRAL_WRITE.bits(), 1 << 1);
        assert_eq!(DsmAttr::DEALLOCATE.bits(), 1 << 2);

        let combined = DsmAttr::DEALLOCATE | DsmAttr::INTEGRAL_WRITE;
        assert_eq!(combined.bits(), 0b110);

        let mut acc = DsmAttr::INTEGRAL_READ;
        acc |= DsmAttr::DEALLOCATE;
        assert_eq!(acc.bits(), 0b101);

        assert_eq!(DsmAttr::from_bits(0b111).bits(), 0b111);
    }

    // -- DsmRange: the 1-based length stored as spec 0-based -----------

    #[test]
    fn dsm_range_length_is_one_based() {
        // new(slba, nlb=1) stores spec 0-based length 0.
        let r = DsmRange::new(0x1000, 1);
        assert_eq!(r.slba, 0x1000);
        assert_eq!(r.length, 0);
        assert_eq!(r.context, 0);

        // nlb=8 -> stored 7.
        assert_eq!(DsmRange::new(0, 8).length, 7);

        // nlb=0 is clamped (saturating_sub) to 0, not underflowed.
        assert_eq!(DsmRange::new(0, 0).length, 0);

        // with_context sets the attribute bits.
        let r = DsmRange::new(0, 4).with_context(0xABCD);
        assert_eq!(r.context, 0xABCD);
        assert_eq!(r.length, 3);
    }

    #[test]
    fn dsm_range_wire_encoding_is_little_endian() {
        // Mirror what Dsm::execute builds, to lock the LE conversion.
        let r = DsmRange::new(0x1122_3344_5566_7788, 0x10).with_context(0xDEAD_BEEF);
        let raw = nvme_dsm_range {
            cattr: r.context.to_le(),
            nlb: r.length.to_le(),
            slba: r.slba.to_le(),
        };
        assert_eq!(u32::from_le(raw.cattr), 0xDEAD_BEEF);
        assert_eq!(u32::from_le(raw.nlb), 0x0F); // 0x10 - 1, 1-based
        assert_eq!(u64::from_le(raw.slba), 0x1122_3344_5566_7788);
    }

    // -- CopyRange: 1-based nlb, builder setters -----------------------

    #[test]
    fn copy_range_builder() {
        let r = CopyRange::new(0x2000, 4)
            .ref_tag(0x55)
            .app_tag(0x1234, 0xFF00);
        assert_eq!(r.slba, 0x2000);
        assert_eq!(r.nlb, 4); // stored 1-based; decremented at execute time
        assert_eq!(r.eilbrt, 0x55);
        assert_eq!(r.elbat, 0x1234);
        assert_eq!(r.elbatm, 0xFF00);
    }

    #[test]
    fn copy_range_wire_encoding_decrements_nlb() {
        // Mirror the nlb_enc computation in Copy::execute.
        let r = CopyRange::new(0, 16);
        let nlb_enc = r.nlb.saturating_sub(1);
        assert_eq!(nlb_enc, 15);
    }

    // -- check_buf_len: buffer-size validation -------------------------

    #[test]
    fn check_buf_len_matches_nlb_times_lba_size() {
        // 8 blocks * 512 = 4096 ok.
        assert!(check_buf_len(4096, 8, 512).is_ok());
        // wrong size rejected.
        assert!(matches!(
            check_buf_len(4095, 8, 512),
            Err(Error::InvalidArgument(_))
        ));
        assert!(matches!(
            check_buf_len(8192, 8, 512),
            Err(Error::InvalidArgument(_))
        ));
    }
}
