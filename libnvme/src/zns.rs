//! NVMe Zoned Namespaces (ZNS) command set.
//!
//! ZNS exposes a namespace as a set of "zones" — append-only regions
//! that must be written sequentially. Hosts must explicitly open zones
//! before writing, finish them when done, and reset them to reclaim
//! space. This module wraps the three ZNS commands `libnvme` exposes:
//!
//! - **Zone Management Send** ([`Namespace::zns_mgmt_send`]) — Open,
//!   Close, Finish, Reset, Offline, Set Descriptor Extension, ZRWA
//!   Flush.
//! - **Zone Management Receive** ([`Namespace::zns_mgmt_recv`]) — Report
//!   Zones / Extended Report Zones.
//! - **Zone Append** ([`Namespace::zns_append`]) — write to a zone
//!   without specifying an LBA (the controller picks the next).
//!
//! See NVMe ZNS Command Set spec rev 1.2 §3 for the full semantics.

use std::ffi::c_void;

use libnvme_sys::{
    nvme_zns_append, nvme_zns_append_args, nvme_zns_mgmt_recv, nvme_zns_mgmt_recv_args,
    nvme_zns_mgmt_send, nvme_zns_mgmt_send_args,
};

use crate::error::{check_ret, Error, Result};
use crate::namespace::Namespace;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Zone Send Action (zsa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u8)]
pub enum ZoneSendAction {
    /// Close an open zone.
    Close = 0x1,
    /// Finish a zone — transition to Full.
    Finish = 0x2,
    /// Explicitly open a zone.
    Open = 0x3,
    /// Reset a zone — transition to Empty.
    Reset = 0x4,
    /// Offline a zone — Read Only zones become Offline.
    Offline = 0x5,
    /// Set Zone Descriptor Extension data.
    SetDescriptorExtension = 0x10,
    /// ZRWA Flush — flush a Zone Random-Write Area.
    ZrwaFlush = 0x11,
}

impl ZoneSendAction {
    fn as_raw(self) -> u32 {
        self as u8 as u32
    }
}

/// Zone Receive Action (zra).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u8)]
pub enum ZoneRecvAction {
    /// Report Zones — standard report.
    #[default]
    Report = 0x0,
    /// Extended Report Zones — includes descriptor-extension data.
    ExtendedReport = 0x1,
}

impl ZoneRecvAction {
    fn as_raw(self) -> u32 {
        self as u8 as u32
    }
}

/// Report-zones filter (ZRASF, zone-receive action-specific field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u16)]
pub enum ZoneReportFilter {
    /// All zones, regardless of state.
    #[default]
    All = 0x0,
    /// Empty zones only.
    Empty = 0x1,
    /// Implicitly opened zones.
    ImplicitlyOpened = 0x2,
    /// Explicitly opened zones.
    ExplicitlyOpened = 0x3,
    /// Closed zones.
    Closed = 0x4,
    /// Full zones.
    Full = 0x5,
    /// Read-only zones.
    ReadOnly = 0x6,
    /// Offline zones.
    Offline = 0x7,
}

impl ZoneReportFilter {
    fn as_raw(self) -> u16 {
        self as u16
    }
}

// ---------------------------------------------------------------------------
// Management Send
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::zns_mgmt_send`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct ZnsMgmtSend<'a, 'r> {
    ns: &'a Namespace<'r>,
    slba: u64,
    action: ZoneSendAction,
    data: Option<&'a [u8]>,
    select_all: bool,
    zsaso: u8,
    timeout_ms: u32,
}

impl<'a, 'r> ZnsMgmtSend<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, slba: u64, action: ZoneSendAction) -> Self {
        ZnsMgmtSend {
            ns,
            slba,
            action,
            data: None,
            select_all: false,
            zsaso: 0,
            timeout_ms: 0,
        }
    }

    /// Data payload (only meaningful for `SetDescriptorExtension`).
    pub fn data(mut self, data: &'a [u8]) -> Self {
        self.data = Some(data);
        self
    }

    /// Apply this action to **all** zones, ignoring `slba`.
    pub fn select_all(mut self) -> Self {
        self.select_all = true;
        self
    }

    /// Zone Send Action Specific Option (operation-dependent).
    pub fn zsaso(mut self, zsaso: u8) -> Self {
        self.zsaso = zsaso;
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Zone Management Send command. Returns the controller's CQE
    /// result dword.
    pub fn execute(self) -> Result<u32> {
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let (data_ptr, data_len) = match self.data {
            Some(buf) => (buf.as_ptr() as *mut c_void, buf.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        let mut args = nvme_zns_mgmt_send_args {
            slba: self.slba,
            result: &mut result,
            data: data_ptr,
            args_size: std::mem::size_of::<nvme_zns_mgmt_send_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            zsa: self.action.as_raw(),
            data_len,
            select_all: self.select_all,
            zsaso: self.zsaso,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid; data
        // (if non-null) points to caller-owned bytes of `data_len`.
        let ret = unsafe { nvme_zns_mgmt_send(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Management Receive
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::zns_mgmt_recv`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct ZnsMgmtRecv<'a, 'r> {
    ns: &'a Namespace<'r>,
    slba: u64,
    action: ZoneRecvAction,
    filter: ZoneReportFilter,
    partial: bool,
    data: Option<&'a mut [u8]>,
    timeout_ms: u32,
}

impl<'a, 'r> ZnsMgmtRecv<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, slba: u64) -> Self {
        ZnsMgmtRecv {
            ns,
            slba,
            action: ZoneRecvAction::default(),
            filter: ZoneReportFilter::default(),
            partial: false,
            data: None,
            timeout_ms: 0,
        }
    }

    /// Use Extended Report Zones (default is Report Zones).
    pub fn extended_report(mut self) -> Self {
        self.action = ZoneRecvAction::ExtendedReport;
        self
    }

    /// Filter zones by state (default: All).
    pub fn filter(mut self, filter: ZoneReportFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Partial-report bit — return whatever fits in the buffer instead
    /// of reporting the full zone count.
    pub fn partial(mut self) -> Self {
        self.partial = true;
        self
    }

    /// Read the report into this buffer (must be at least 8 bytes for
    /// the header). Required before [`Self::execute`].
    pub fn buffer(mut self, buf: &'a mut [u8]) -> Self {
        self.data = Some(buf);
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Zone Management Receive command. Returns the controller's CQE
    /// result dword.
    pub fn execute(mut self) -> Result<u32> {
        let buf = self.data.as_deref_mut().ok_or(Error::invalid_argument(
            "zns_mgmt_recv requires a buffer via .buffer()",
        ))?;
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let mut args = nvme_zns_mgmt_recv_args {
            slba: self.slba,
            result: &mut result,
            data: buf.as_mut_ptr() as *mut c_void,
            args_size: std::mem::size_of::<nvme_zns_mgmt_recv_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            zra: self.action.as_raw(),
            data_len: buf.len() as u32,
            zrasf: self.filter.as_raw(),
            zras_feat: self.partial,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid; data
        // points to a caller-owned buffer of `data_len`.
        let ret = unsafe { nvme_zns_mgmt_recv(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Append
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::zns_append`].
///
/// Writes `data` into the zone that contains `zslba`. Unlike a normal
/// write, the controller picks the destination LBA (always the zone's
/// write pointer) and returns it in the result-dword. Block count is
/// 1-based; we subtract one for the spec's 0-based NLB encoding.
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct ZnsAppend<'a, 'r> {
    ns: &'a Namespace<'r>,
    zslba: u64,
    nlb: u32,
    data: &'a [u8],
    metadata: Option<&'a [u8]>,
    ilbrt: u32,
    ilbrt_u64: u64,
    lbat: u16,
    lbatm: u16,
    control: u16,
    timeout_ms: u32,
}

const ZNS_CTRL_FUA: u16 = 1 << 14;
const ZNS_CTRL_LR: u16 = 1 << 15;
const ZNS_CTRL_PRINFO_PRACT: u16 = 1 << 13;
const ZNS_CTRL_PRINFO_PRCHK_GUARD: u16 = 1 << 12;
const ZNS_CTRL_PRINFO_PRCHK_APP: u16 = 1 << 11;
const ZNS_CTRL_PRINFO_PRCHK_REF: u16 = 1 << 10;

impl<'a, 'r> ZnsAppend<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, zslba: u64, nlb: u32, data: &'a [u8]) -> Self {
        ZnsAppend {
            ns,
            zslba,
            nlb,
            data,
            metadata: None,
            ilbrt: 0,
            ilbrt_u64: 0,
            lbat: 0,
            lbatm: 0,
            control: 0,
            timeout_ms: 0,
        }
    }

    /// Attach a metadata buffer (separate from data).
    pub fn metadata(mut self, md: &'a [u8]) -> Self {
        self.metadata = Some(md);
        self
    }

    /// Force Unit Access.
    pub fn fua(mut self) -> Self {
        self.control |= ZNS_CTRL_FUA;
        self
    }

    /// Limited Retry.
    pub fn limited_retry(mut self) -> Self {
        self.control |= ZNS_CTRL_LR;
        self
    }

    /// PI Action (PRACT).
    pub fn protection_action(mut self) -> Self {
        self.control |= ZNS_CTRL_PRINFO_PRACT;
        self
    }

    /// Check Guard (PRCHK.GUARD) — enable PI CRC check.
    pub fn check_guard(mut self) -> Self {
        self.control |= ZNS_CTRL_PRINFO_PRCHK_GUARD;
        self
    }

    /// Check Application Tag (PRCHK.APP).
    pub fn check_apptag(mut self) -> Self {
        self.control |= ZNS_CTRL_PRINFO_PRCHK_APP;
        self
    }

    /// Check Reference Tag (PRCHK.REF) — enable PI ref-tag check.
    pub fn check_reftag(mut self) -> Self {
        self.control |= ZNS_CTRL_PRINFO_PRCHK_REF;
        self
    }

    /// Initial Logical Block Reference Tag (32-bit).
    pub fn ref_tag(mut self, ilbrt: u32) -> Self {
        self.ilbrt = ilbrt;
        self
    }

    /// Initial Logical Block Reference Tag (64-bit, enhanced PI).
    pub fn ref_tag_u64(mut self, ilbrt: u64) -> Self {
        self.ilbrt_u64 = ilbrt;
        self
    }

    /// Logical Block Application Tag + mask.
    pub fn app_tag(mut self, tag: u16, mask: u16) -> Self {
        self.lbat = tag;
        self.lbatm = mask;
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Execute the Append. Returns the 64-bit assigned LBA via the
    /// result-dword pair (low 32 bits are the LBA's lower half; the
    /// controller writes the full 64-bit value into `result`).
    pub fn execute(self) -> Result<u64> {
        if self.nlb == 0 || self.nlb > 65_536 {
            return Err(Error::invalid_argument(format!(
                "nlb must be 1..=65536, got {}",
                self.nlb
            )));
        }
        let nlb_enc = (self.nlb - 1) as u16;
        let lba_size = self.ns.lba_size();
        let want = u64::from(self.nlb) * u64::from(lba_size);
        if self.data.len() as u64 != want {
            return Err(Error::invalid_argument(format!(
                "buffer length {} doesn't match nlb * lba_size = {}",
                self.data.len(),
                want
            )));
        }
        let fd = ns_fd(self.ns)?;
        let mut result: u64 = 0;
        let (md_ptr, md_len) = match self.metadata {
            Some(md) => (md.as_ptr() as *mut c_void, md.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        let mut args = nvme_zns_append_args {
            zslba: self.zslba,
            result: &mut result,
            // libnvme's data field is `void *`; for Append the buffer is
            // read by the controller, but the C API requires a mutable
            // pointer. Not a real mutability claim.
            data: self.data.as_ptr() as *mut c_void,
            metadata: md_ptr,
            args_size: std::mem::size_of::<nvme_zns_append_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            ilbrt: self.ilbrt,
            data_len: self.data.len() as u32,
            metadata_len: md_len,
            nlb: nlb_enc,
            control: self.control,
            lbat: self.lbat,
            lbatm: self.lbatm,
            rsvd1: [0u8; 4],
            ilbrt_u64: self.ilbrt_u64,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid;
        // data and metadata (if non-null) point to caller-owned buffers.
        let ret = unsafe { nvme_zns_append(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

fn ns_fd(ns: &Namespace<'_>) -> Result<std::os::raw::c_int> {
    // SAFETY: ns.raw_handle() is a non-null nvme_ns_t tied to the Root tree.
    let fd = unsafe { libnvme_sys::nvme_ns_get_fd(ns.raw_handle()) };
    if fd < 0 {
        return Err(Error::Os(std::io::Error::last_os_error()));
    }
    Ok(fd)
}
