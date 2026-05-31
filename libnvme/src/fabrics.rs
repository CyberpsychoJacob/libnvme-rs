//! NVMe over Fabrics — Connect, Disconnect, Discover.
//!
//! Fabrics workflow:
//! 1. Get or create a [`Host`](crate::Host) via
//!    [`Root::default_host`](crate::Root::default_host) or
//!    [`Root::lookup_host`](crate::Root::lookup_host).
//! 2. Build a connection with [`Host::connect`](crate::Host::connect),
//!    chaining setters for transport-specific options.
//! 3. Call [`Connect::execute`] to issue the NVMe Connect command and
//!    return a live [`Controller`](crate::Controller).
//! 4. (Optional) Read the discovery log via
//!    [`Controller::discovery_log`](crate::Controller::discovery_log) when
//!    connected to a discovery controller.
//! 5. Tear down with [`Controller::disconnect`](crate::Controller::disconnect).

use std::ffi::CString;
use std::marker::PhantomData;
use std::ptr::NonNull;

#[cfg(has_unique_discovery_ctrl)]
use libnvme_sys::nvme_ctrl_set_unique_discovery_ctrl;
use libnvme_sys::{
    nvme_create_ctrl, nvme_ctrl_set_discovery_ctrl, nvme_ctrl_set_persistent, nvme_fabrics_config,
    nvmf_add_ctrl, nvmf_default_config, nvmf_disc_log_entry, nvmf_discovery_log,
    nvmf_get_discovery_log,
};

use crate::error::check_ret;
use crate::host::Host;
use crate::util::cstr_to_str;
use crate::{Controller, Error, Result};

// libnvme's discovery-log functions allocate via malloc; we free via libc's
// free without pulling in the libc crate.
unsafe extern "C" {
    fn free(ptr: *mut std::ffi::c_void);
}

/// NVMe-oF transport selector. The `Other` variant is an escape hatch for
/// transports added to libnvme after this crate was released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport<'a> {
    /// TCP/IP transport (the most common).
    Tcp,
    /// RDMA (RoCE, iWARP, InfiniBand).
    Rdma,
    /// Fibre Channel.
    Fc,
    /// Intra-host loopback (the `nvme_loop` kernel driver).
    Loop,
    /// Anything else libnvme accepts as a transport string.
    Other(&'a str),
}

impl Transport<'_> {
    fn as_str(&self) -> &str {
        match self {
            Transport::Tcp => "tcp",
            Transport::Rdma => "rdma",
            Transport::Fc => "fc",
            Transport::Loop => "loop",
            Transport::Other(s) => s,
        }
    }
}

/// Builder for an NVMe-oF Connect operation.
///
/// Created by [`Host::connect`](crate::Host::connect). Required parameters
/// (`transport`, `subsysnqn`) are taken at construction; everything else is
/// chained on as optional setters and defaults to libnvme's
/// `nvmf_default_config` values.
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Connect<'a, 'r> {
    host: &'a Host<'r>,
    transport: String,
    subsysnqn: String,
    traddr: Option<String>,
    trsvcid: Option<String>,
    host_traddr: Option<String>,
    host_iface: Option<String>,
    queue_size: Option<i32>,
    nr_io_queues: Option<i32>,
    keep_alive_tmo: Option<i32>,
    reconnect_delay: Option<i32>,
    ctrl_loss_tmo: Option<i32>,
    hdr_digest: bool,
    data_digest: bool,
    tls: bool,
    duplicate_connect: bool,
    disable_sqflow: bool,
    persistent: bool,
    discovery: bool,
    unique_discovery: bool,
}

impl<'a, 'r> Connect<'a, 'r> {
    pub(crate) fn new(host: &'a Host<'r>, transport: Transport<'_>, subsysnqn: &str) -> Self {
        Connect {
            host,
            transport: transport.as_str().to_owned(),
            subsysnqn: subsysnqn.to_owned(),
            traddr: None,
            trsvcid: None,
            host_traddr: None,
            host_iface: None,
            queue_size: None,
            nr_io_queues: None,
            keep_alive_tmo: None,
            reconnect_delay: None,
            ctrl_loss_tmo: None,
            hdr_digest: false,
            data_digest: false,
            tls: false,
            duplicate_connect: false,
            disable_sqflow: false,
            persistent: false,
            discovery: false,
            unique_discovery: false,
        }
    }

    /// Transport target address — IP/hostname for TCP/RDMA, WWNN/WWPN for FC.
    pub fn traddr(mut self, addr: &str) -> Self {
        self.traddr = Some(addr.into());
        self
    }

    /// Transport service identifier — TCP/RDMA port number, FC service id.
    pub fn trsvcid(mut self, svcid: &str) -> Self {
        self.trsvcid = Some(svcid.into());
        self
    }

    /// Host-side transport address (Fabrics `host_traddr`).
    pub fn host_traddr(mut self, addr: &str) -> Self {
        self.host_traddr = Some(addr.into());
        self
    }

    /// Host network interface (Fabrics `host_iface`).
    pub fn host_iface(mut self, iface: &str) -> Self {
        self.host_iface = Some(iface.into());
        self
    }

    /// Per-queue depth for I/O queues.
    pub fn queue_size(mut self, n: i32) -> Self {
        self.queue_size = Some(n);
        self
    }

    /// Number of I/O queues to establish.
    pub fn nr_io_queues(mut self, n: i32) -> Self {
        self.nr_io_queues = Some(n);
        self
    }

    /// Keep-alive timeout in seconds.
    pub fn keep_alive_tmo(mut self, seconds: i32) -> Self {
        self.keep_alive_tmo = Some(seconds);
        self
    }

    /// Delay between reconnect attempts, in seconds.
    pub fn reconnect_delay(mut self, seconds: i32) -> Self {
        self.reconnect_delay = Some(seconds);
        self
    }

    /// Total timeout for controller-loss reconnect, in seconds.
    pub fn ctrl_loss_tmo(mut self, seconds: i32) -> Self {
        self.ctrl_loss_tmo = Some(seconds);
        self
    }

    /// Enable PDU header digest (TCP only).
    pub fn hdr_digest(mut self, enabled: bool) -> Self {
        self.hdr_digest = enabled;
        self
    }

    /// Enable PDU data digest (TCP only).
    pub fn data_digest(mut self, enabled: bool) -> Self {
        self.data_digest = enabled;
        self
    }

    /// Enable TLS on the connection (TCP only).
    pub fn tls(mut self, enabled: bool) -> Self {
        self.tls = enabled;
        self
    }

    /// Allow multiple connections to the same target.
    pub fn duplicate_connect(mut self, enabled: bool) -> Self {
        self.duplicate_connect = enabled;
        self
    }

    /// Disable submission-queue flow control.
    pub fn disable_sqflow(mut self, enabled: bool) -> Self {
        self.disable_sqflow = enabled;
        self
    }

    /// Mark the resulting controller as persistent (kernel-side keep-alive).
    pub fn persistent(mut self, enabled: bool) -> Self {
        self.persistent = enabled;
        self
    }

    /// Mark the controller as a discovery controller. Set this when connecting
    /// to a discovery service (subsysnqn = `nqn.2014-08.org.nvmexpress.discovery`).
    pub fn discovery(mut self) -> Self {
        self.discovery = true;
        self
    }

    /// Mark as a unique discovery controller (NVMe spec ≥ 2.0).
    ///
    /// Only present when built against a libnvme that exposes
    /// `nvme_ctrl_set_unique_discovery_ctrl`.
    #[cfg(has_unique_discovery_ctrl)]
    pub fn unique_discovery(mut self) -> Self {
        self.discovery = true;
        self.unique_discovery = true;
        self
    }

    /// Issue the NVMe Connect command and return the live [`Controller`].
    pub fn execute(self) -> Result<Controller<'r>> {
        let subsysnqn = CString::new(self.subsysnqn).map_err(invalid_input)?;
        let transport = CString::new(self.transport).map_err(invalid_input)?;
        let traddr = string_to_cstring_opt(self.traddr)?;
        let trsvcid = string_to_cstring_opt(self.trsvcid)?;
        let host_traddr = string_to_cstring_opt(self.host_traddr)?;
        let host_iface = string_to_cstring_opt(self.host_iface)?;

        // SAFETY: host.root_ptr() is a non-null nvme_root_t alive for 'r;
        // subsysnqn/transport are valid NUL-terminated C strings (alive for
        // the call); the remaining pointers are either NULL or point to
        // CStrings kept alive in this scope.
        let ctrl = unsafe {
            nvme_create_ctrl(
                self.host.root_ptr(),
                subsysnqn.as_ptr(),
                transport.as_ptr(),
                cstr_ptr_or_null(&traddr),
                cstr_ptr_or_null(&host_traddr),
                cstr_ptr_or_null(&host_iface),
                cstr_ptr_or_null(&trsvcid),
            )
        };
        if ctrl.is_null() {
            return Err(Error::Os(std::io::Error::last_os_error()));
        }

        if self.discovery {
            // SAFETY: ctrl is non-null (checked above) and we own it until we
            // return it via Controller::from_raw.
            unsafe { nvme_ctrl_set_discovery_ctrl(ctrl, true) };
        }
        #[cfg(has_unique_discovery_ctrl)]
        if self.unique_discovery {
            // SAFETY: ctrl is non-null (checked above) and we own it until we
            // return it via Controller::from_raw.
            unsafe { nvme_ctrl_set_unique_discovery_ctrl(ctrl, true) };
        }
        if self.persistent {
            // SAFETY: ctrl is non-null (checked above) and we own it until we
            // return it via Controller::from_raw.
            unsafe { nvme_ctrl_set_persistent(ctrl, true) };
        }

        // Build the fabrics config from defaults + our overrides.
        let mut cfg = nvme_fabrics_config::default();
        // SAFETY: &mut cfg points to a stack-allocated, default-initialized
        // nvme_fabrics_config which libnvme overwrites with its defaults.
        unsafe { nvmf_default_config(&mut cfg) };
        if let Some(n) = self.queue_size {
            cfg.queue_size = n;
        }
        if let Some(n) = self.nr_io_queues {
            cfg.nr_io_queues = n;
        }
        if let Some(n) = self.keep_alive_tmo {
            cfg.keep_alive_tmo = n;
        }
        if let Some(n) = self.reconnect_delay {
            cfg.reconnect_delay = n;
        }
        if let Some(n) = self.ctrl_loss_tmo {
            cfg.ctrl_loss_tmo = n;
        }
        cfg.hdr_digest = self.hdr_digest;
        cfg.data_digest = self.data_digest;
        cfg.tls = self.tls;
        cfg.duplicate_connect = self.duplicate_connect;
        cfg.disable_sqflow = self.disable_sqflow;

        // nvme_fabrics_config has *mut c_char for host_traddr / host_iface.
        // libnvme reads (not stores) these during the call, so keeping the
        // CStrings alive for the duration of nvmf_add_ctrl is sufficient.
        if let Some(ref s) = host_traddr {
            cfg.host_traddr = s.as_ptr() as *mut _;
        }
        if let Some(ref s) = host_iface {
            cfg.host_iface = s.as_ptr() as *mut _;
        }

        // SAFETY: host.as_ptr() is a non-null nvme_host_t tied to 'r; ctrl is
        // the non-null handle we created above; &cfg references our local
        // fabrics_config; any CString-backed pointers inside cfg are still
        // alive in this scope.
        let ret = unsafe { nvmf_add_ctrl(self.host.as_ptr(), ctrl, &cfg) };
        check_ret(ret)?;

        // Tie the new ctrl's lifetime to the same root.
        Ok(Controller::from_raw(ctrl))
    }
}

/// Discovery-log header + entries returned by a discovery controller's
/// log page (LID 0x70).
pub struct DiscoveryLog {
    inner: NonNull<nvmf_discovery_log>,
    _not_send_sync: PhantomData<*const ()>,
}

impl DiscoveryLog {
    pub(crate) fn from_raw(ptr: *mut nvmf_discovery_log) -> Result<Self> {
        let inner = NonNull::new(ptr).ok_or_else(|| Error::Os(std::io::Error::last_os_error()))?;
        Ok(DiscoveryLog {
            inner,
            _not_send_sync: PhantomData,
        })
    }

    /// Discovery generation counter — increments whenever the discovery
    /// service's record set changes. Clients use this to detect updates.
    pub fn generation_counter(&self) -> u64 {
        // SAFETY: self.inner is a NonNull pointer to a malloc'd discovery log
        // we own; we only read the genctr field, alive for &self.
        unsafe { (*self.inner.as_ptr()).genctr }
    }

    /// Number of entries in this discovery log.
    pub fn num_records(&self) -> u64 {
        // SAFETY: self.inner is a NonNull pointer to a malloc'd discovery log
        // we own; we only read the numrec field, alive for &self.
        unsafe { (*self.inner.as_ptr()).numrec }
    }

    /// Iterate over the entries (as typed wrappers).
    pub fn entries(&self) -> impl Iterator<Item = DiscoveryLogEntry<'_>> {
        let count = self.num_records() as usize;
        // `entries` is bindgen's __IncompleteArrayField — its `as_ptr()`
        // points at the address immediately past the header where libnvme
        // wrote `count` entries.
        // SAFETY: self.inner is a NonNull pointer to a malloc'd discovery log
        // we own; entries.as_ptr() yields the start of the trailing flexible
        // array which libnvme populated with `count` records.
        let ptr = unsafe { (*self.inner.as_ptr()).entries.as_ptr() };
        (0..count).map(move |i| DiscoveryLogEntry {
            // SAFETY: i is in 0..count and libnvme wrote `count` consecutive
            // entries past the header; the borrow's lifetime is bounded by
            // &self so the backing allocation outlives the iterator.
            raw: unsafe { &*ptr.add(i) },
        })
    }
}

impl Drop for DiscoveryLog {
    fn drop(&mut self) {
        // SAFETY: self.inner came from libnvme's malloc-family allocator
        // (returned by nvmf_get_discovery_log); we own it exclusively and
        // this is the only Drop path.
        unsafe { free(self.inner.as_ptr() as *mut _) };
    }
}

impl std::fmt::Debug for DiscoveryLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryLog")
            .field("genctr", &self.generation_counter())
            .field("numrec", &self.num_records())
            .finish()
    }
}

/// One entry from the discovery log page.
pub struct DiscoveryLogEntry<'log> {
    raw: &'log nvmf_disc_log_entry,
}

impl<'log> DiscoveryLogEntry<'log> {
    /// Raw transport type byte (`nvmf_trtype` values: 1=RDMA, 2=FC, 3=TCP,
    /// 254=loop, ...).
    pub fn transport_type(&self) -> u8 {
        self.raw.trtype
    }

    /// Address family for the `traddr` field (1=IPv4, 2=IPv6, ...).
    pub fn address_family(&self) -> u8 {
        self.raw.adrfam
    }

    /// Subsystem type (1=discovery, 2=NVM, ...).
    pub fn subsystem_type(&self) -> u8 {
        self.raw.subtype
    }

    /// Transport requirements byte.
    pub fn transport_requirements(&self) -> u8 {
        self.raw.treq
    }

    /// Port identifier within the discovery service.
    pub fn port_id(&self) -> u16 {
        self.raw.portid
    }

    /// Controller ID assigned to the discovered controller (`0xFFFF` =
    /// dynamic).
    pub fn controller_id(&self) -> u16 {
        self.raw.cntlid
    }

    /// Admin Submission Queue size for the discovered controller.
    pub fn asq_size(&self) -> u16 {
        self.raw.asqsz
    }

    /// Discovery-log entry flags.
    pub fn entry_flags(&self) -> u16 {
        self.raw.eflags
    }

    /// Transport service identifier (port number for TCP/RDMA).
    pub fn transport_service_id(&self) -> Result<&'log str> {
        // SAFETY: self.raw borrows from the DiscoveryLog's malloc'd buffer for
        // 'log; trsvcid is a fixed-size NUL-padded ASCII field within the
        // entry, which cstr_to_str will safely scan.
        unsafe { cstr_to_str(self.raw.trsvcid.as_ptr()) }
    }

    /// Subsystem NQN of the discovered target.
    pub fn subnqn(&self) -> Result<&'log str> {
        // SAFETY: self.raw borrows from the DiscoveryLog's malloc'd buffer for
        // 'log; subnqn is a fixed-size NUL-padded ASCII field within the
        // entry, which cstr_to_str will safely scan.
        unsafe { cstr_to_str(self.raw.subnqn.as_ptr()) }
    }

    /// Transport address (IP, WWPN, ...) of the discovered target.
    pub fn traddr(&self) -> Result<&'log str> {
        // SAFETY: self.raw borrows from the DiscoveryLog's malloc'd buffer for
        // 'log; traddr is a fixed-size NUL-padded ASCII field within the
        // entry, which cstr_to_str will safely scan.
        unsafe { cstr_to_str(self.raw.traddr.as_ptr()) }
    }
}

impl std::fmt::Debug for DiscoveryLogEntry<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryLogEntry")
            .field("trtype", &self.transport_type())
            .field("adrfam", &self.address_family())
            .field("subnqn", &self.subnqn().ok())
            .field("traddr", &self.traddr().ok())
            .field("trsvcid", &self.transport_service_id().ok())
            .finish()
    }
}

/// Read `nvmf_get_discovery_log` for an already-connected discovery
/// controller. Crate-internal helper used by [`Controller::discovery_log`].
pub(crate) fn fetch_discovery_log(
    ctrl: libnvme_sys::nvme_ctrl_t,
    max_retries: i32,
) -> Result<DiscoveryLog> {
    let mut logp: *mut nvmf_discovery_log = std::ptr::null_mut();
    // SAFETY: ctrl is a non-null nvme_ctrl_t tied to the libnvme tree; &mut
    // logp points to a local pointer libnvme will overwrite with a fresh
    // malloc-allocated discovery log (or leave NULL on failure).
    let ret = unsafe { nvmf_get_discovery_log(ctrl, &mut logp, max_retries) };
    check_ret(ret)?;
    DiscoveryLog::from_raw(logp)
}

fn string_to_cstring_opt(opt: Option<String>) -> Result<Option<CString>> {
    match opt {
        None => Ok(None),
        Some(s) => CString::new(s).map(Some).map_err(invalid_input),
    }
}

fn cstr_ptr_or_null(opt: &Option<CString>) -> *const std::os::raw::c_char {
    match opt {
        Some(s) => s.as_ptr(),
        None => std::ptr::null(),
    }
}

fn invalid_input(_e: std::ffi::NulError) -> Error {
    Error::invalid_argument("interior NUL byte in fabrics parameter")
}
