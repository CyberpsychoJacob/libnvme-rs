//! NVMe Directives (Send / Receive).
//!
//! Directives carry workload hints from the host to the controller. Two
//! Directive Types are defined by the spec:
//!
//! - **Identify** (`DirectiveType::Identify`) — query/configure which
//!   directives the controller supports.
//! - **Streams** (`DirectiveType::Streams`) — host hints about which
//!   logical "stream" each write belongs to, so the controller can
//!   place related data together on media.
//!
//! Vendor-specific directive types live above `0x80`; pass the raw value
//! to [`DirectiveType::Raw`].
//!
//! All directives are namespace-scoped. Build via the methods on
//! [`Namespace`] (`directive_send`, `directive_recv`) and call `.execute()`.
//!
//! See NVMe spec §5.13–§5.14 for the full semantics.

use std::ffi::c_void;

use libnvme_sys::{
    nvme_directive_recv, nvme_directive_recv_args, nvme_directive_send, nvme_directive_send_args,
};

use crate::error::{check_ret, Error, Result};
use crate::namespace::Namespace;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Directive Type (dtype).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveType {
    /// Identify Directive — controller capability queries.
    Identify,
    /// Streams Directive — write-placement hints.
    Streams,
    /// Vendor-specific directive (0x80 and above).
    Raw(u8),
}

impl DirectiveType {
    fn as_raw(self) -> u32 {
        match self {
            DirectiveType::Identify => 0,
            DirectiveType::Streams => 1,
            DirectiveType::Raw(v) => u32::from(v),
        }
    }
}

/// Directive Send Operation (doper).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveSendOp {
    /// Identify Directive — Enable Directive.
    IdentifyEnable,
    /// Streams — Release Identifier.
    StreamsReleaseIdentifier,
    /// Streams — Release Resource.
    StreamsReleaseResource,
    /// Raw operation code for vendor-specific directives.
    Raw(u8),
}

impl DirectiveSendOp {
    fn as_raw(self) -> u32 {
        match self {
            DirectiveSendOp::IdentifyEnable | DirectiveSendOp::StreamsReleaseIdentifier => 0x01,
            DirectiveSendOp::StreamsReleaseResource => 0x02,
            DirectiveSendOp::Raw(v) => u32::from(v),
        }
    }
}

/// Directive Receive Operation (doper).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveRecvOp {
    /// Identify — Return Parameters.
    IdentifyReturnParameters,
    /// Streams — Return Parameters.
    StreamsReturnParameters,
    /// Streams — Get Status.
    StreamsGetStatus,
    /// Streams — Allocate Resources.
    StreamsAllocateResources,
    /// Raw operation code for vendor-specific directives.
    Raw(u8),
}

impl DirectiveRecvOp {
    fn as_raw(self) -> u32 {
        match self {
            DirectiveRecvOp::IdentifyReturnParameters
            | DirectiveRecvOp::StreamsReturnParameters => 0x01,
            DirectiveRecvOp::StreamsGetStatus => 0x02,
            DirectiveRecvOp::StreamsAllocateResources => 0x03,
            DirectiveRecvOp::Raw(v) => u32::from(v),
        }
    }
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::directive_send`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct DirectiveSend<'a, 'r> {
    ns: &'a Namespace<'r>,
    dtype: DirectiveType,
    doper: DirectiveSendOp,
    data: Option<&'a [u8]>,
    dspec: u16,
    cdw12: u32,
    timeout_ms: u32,
}

impl<'a, 'r> DirectiveSend<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, dtype: DirectiveType, doper: DirectiveSendOp) -> Self {
        DirectiveSend {
            ns,
            dtype,
            doper,
            data: None,
            dspec: 0,
            cdw12: 0,
            timeout_ms: 0,
        }
    }

    /// Data payload (interpretation depends on the operation).
    pub fn data(mut self, data: &'a [u8]) -> Self {
        self.data = Some(data);
        self
    }

    /// Directive-Specific field (DSPEC, 16-bit).
    pub fn dspec(mut self, dspec: u16) -> Self {
        self.dspec = dspec;
        self
    }

    /// Directive-specific CDW12 (operation-dependent).
    pub fn cdw12(mut self, cdw12: u32) -> Self {
        self.cdw12 = cdw12;
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Directive Send command. Returns the controller's CQE result
    /// dword.
    pub fn execute(self) -> Result<u32> {
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let (data_ptr, data_len) = match self.data {
            // libnvme's data field is `void *`; for Directive Send it is
            // read-only on the host side. The mut cast is a C-API conformance
            // cast, not a real mutability claim.
            Some(buf) => (buf.as_ptr() as *mut c_void, buf.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        let mut args = nvme_directive_send_args {
            result: &mut result,
            data: data_ptr,
            args_size: std::mem::size_of::<nvme_directive_send_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            doper: self.doper.as_raw(),
            dtype: self.dtype.as_raw(),
            cdw12: self.cdw12,
            data_len,
            dspec: self.dspec,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid; data
        // (if non-null) points to caller-owned bytes of `data_len`.
        let ret = unsafe { nvme_directive_send(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

/// Builder returned by [`Namespace::directive_recv`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct DirectiveRecv<'a, 'r> {
    ns: &'a Namespace<'r>,
    dtype: DirectiveType,
    doper: DirectiveRecvOp,
    data: Option<&'a mut [u8]>,
    dspec: u16,
    cdw12: u32,
    timeout_ms: u32,
}

impl<'a, 'r> DirectiveRecv<'a, 'r> {
    pub(crate) fn new(ns: &'a Namespace<'r>, dtype: DirectiveType, doper: DirectiveRecvOp) -> Self {
        DirectiveRecv {
            ns,
            dtype,
            doper,
            data: None,
            dspec: 0,
            cdw12: 0,
            timeout_ms: 0,
        }
    }

    /// Buffer to receive the directive's response payload.
    pub fn buffer(mut self, data: &'a mut [u8]) -> Self {
        self.data = Some(data);
        self
    }

    /// Directive-Specific field (DSPEC, 16-bit).
    pub fn dspec(mut self, dspec: u16) -> Self {
        self.dspec = dspec;
        self
    }

    /// Directive-specific CDW12 (operation-dependent).
    pub fn cdw12(mut self, cdw12: u32) -> Self {
        self.cdw12 = cdw12;
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Directive Receive command. Returns the controller's CQE
    /// result dword.
    pub fn execute(mut self) -> Result<u32> {
        let fd = ns_fd(self.ns)?;
        let mut result: u32 = 0;
        let (data_ptr, data_len) = match self.data.as_deref_mut() {
            Some(buf) => (buf.as_mut_ptr() as *mut c_void, buf.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        let mut args = nvme_directive_recv_args {
            result: &mut result,
            data: data_ptr,
            args_size: std::mem::size_of::<nvme_directive_recv_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            doper: self.doper.as_raw(),
            dtype: self.dtype.as_raw(),
            cdw12: self.cdw12,
            data_len,
            dspec: self.dspec,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid; data
        // (if non-null) points to a caller-owned buffer of `data_len`.
        let ret = unsafe { nvme_directive_recv(&mut args) };
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
