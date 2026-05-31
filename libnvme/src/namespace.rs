use std::marker::PhantomData;

use libnvme_sys::{
    nvme_ctrl_first_ns, nvme_ctrl_next_ns, nvme_ctrl_t, nvme_format_nvm, nvme_format_nvm_args,
    nvme_id_ns, nvme_ns_get_csi, nvme_ns_get_eui64, nvme_ns_get_firmware, nvme_ns_get_generic_name,
    nvme_ns_get_lba_count, nvme_ns_get_lba_size, nvme_ns_get_lba_util, nvme_ns_get_meta_size,
    nvme_ns_get_model, nvme_ns_get_name, nvme_ns_get_nguid, nvme_ns_get_nsid, nvme_ns_get_serial,
    nvme_ns_get_uuid, nvme_ns_identify, nvme_ns_t,
};

use crate::admin::{MetadataSettings, ProtectionInfo, ProtectionLocation, SecureErase};
use crate::error::check_ret;
use crate::identify::IdentifyNamespace;
use crate::io::{
    self, Compare, Copy, CopyRange, Dsm, DsmAttr, Read, Verify, Write, WriteUncorrectable,
    WriteZeroes,
};
use crate::path::Paths;
use crate::util::cstr_to_str;
use crate::{Result, Root};

/// An NVMe namespace.
///
/// Maps to a `/dev/nvmeXnY` block device. A namespace is an addressable
/// region of logical blocks, with its own LBA format, identifier(s), and size.
pub struct Namespace<'r> {
    inner: nvme_ns_t,
    _marker: PhantomData<&'r Root>,
    _not_send_sync: PhantomData<*const ()>,
}

impl<'r> Namespace<'r> {
    pub(crate) fn from_raw(inner: nvme_ns_t) -> Self {
        Namespace {
            inner,
            _marker: PhantomData,
            _not_send_sync: PhantomData,
        }
    }

    /// Kernel-assigned namespace name, e.g. `nvme0n1`.
    pub fn name(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_ns_get_name(self.inner)) }
    }

    /// Generic-namespace name, e.g. `ng0n1`. The generic device exposes the
    /// namespace via `/dev/ng*` for passthrough I/O.
    pub fn generic_name(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_ns_get_generic_name(self.inner)) }
    }

    /// Namespace identifier (1-based, unique within the controller).
    pub fn nsid(&self) -> u32 {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        (unsafe { nvme_ns_get_nsid(self.inner) }) as u32
    }

    /// Logical block size in bytes (typically 512 or 4096).
    pub fn lba_size(&self) -> u32 {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        (unsafe { nvme_ns_get_lba_size(self.inner) }) as u32
    }

    /// Metadata bytes per LBA, or `0` if metadata is not used in the active
    /// LBA format.
    pub fn meta_size(&self) -> u32 {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        (unsafe { nvme_ns_get_meta_size(self.inner) }) as u32
    }

    /// Total number of logical blocks in the namespace.
    pub fn lba_count(&self) -> u64 {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        unsafe { nvme_ns_get_lba_count(self.inner) }
    }

    /// Number of logical blocks actually allocated within the namespace
    /// (`nuse` in Identify Namespace).
    pub fn lba_utilization(&self) -> u64 {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        unsafe { nvme_ns_get_lba_util(self.inner) }
    }

    /// Total namespace size in bytes (`lba_count * lba_size`).
    pub fn size_bytes(&self) -> u64 {
        self.lba_count().saturating_mul(u64::from(self.lba_size()))
    }

    /// Command Set Identifier. `0` = NVM, `1` = Key-Value, `2` = Zoned.
    pub fn csi(&self) -> u8 {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        (unsafe { nvme_ns_get_csi(self.inner) }) as u8
    }

    /// Model string of the controller that owns this namespace
    /// (whitespace-trimmed). Convenience wrapper that avoids walking back up
    /// through `Subsystem` / `Controller`.
    pub fn model(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_ns_get_model(self.inner)) }
    }

    /// Serial number of the controller that owns this namespace
    /// (whitespace-trimmed).
    pub fn serial(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_ns_get_serial(self.inner)) }
    }

    /// Firmware revision of the controller that owns this namespace
    /// (whitespace-trimmed).
    pub fn firmware(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_ns_get_firmware(self.inner)) }
    }

    /// 128-bit namespace UUID, or all-zero if not reported.
    pub fn uuid(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        // SAFETY: self.inner is a non-null nvme_ns_t tied to 'r; `out` is a
        // stack buffer of exactly 16 bytes which libnvme writes into.
        unsafe { nvme_ns_get_uuid(self.inner, out.as_mut_ptr()) };
        out
    }

    /// 128-bit Namespace Globally Unique Identifier (NGUID), or all-zero.
    pub fn nguid(&self) -> [u8; 16] {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to 'r. Returns NULL
        // or a pointer to a 16-byte field owned by the tree.
        let ptr = unsafe { nvme_ns_get_nguid(self.inner) };
        if ptr.is_null() {
            return [0; 16];
        }
        let mut out = [0u8; 16];
        // SAFETY: ptr is non-null (checked) and points to at least 16 bytes
        // (an NGUID field owned by the tree); out is a stack array of 16
        // bytes; the regions don't overlap.
        unsafe { std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), 16) };
        out
    }

    /// 64-bit IEEE Extended Unique Identifier (EUI-64), or all-zero.
    pub fn eui64(&self) -> [u8; 8] {
        // SAFETY: self.inner is a non-null nvme_ns_t tied to 'r. Returns NULL
        // or a pointer to an 8-byte field owned by the tree.
        let ptr = unsafe { nvme_ns_get_eui64(self.inner) };
        if ptr.is_null() {
            return [0; 8];
        }
        let mut out = [0u8; 8];
        // SAFETY: ptr is non-null (checked) and points to at least 8 bytes
        // (an EUI-64 field owned by the tree); out is a stack array of 8
        // bytes; the regions don't overlap.
        unsafe { std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), 8) };
        out
    }

    /// Issue the Identify Namespace admin command and return the decoded
    /// data structure.
    pub fn identify(&self) -> Result<IdentifyNamespace> {
        let mut id = Box::new(nvme_id_ns::default());
        // SAFETY: self.inner is a non-null nvme_ns_t tied to 'r; id is a
        // uniquely-owned, heap-allocated nvme_id_ns that outlives the call
        // and which libnvme will fill in via the &mut pointer.
        let ret = unsafe { nvme_ns_identify(self.inner, id.as_mut() as *mut _) };
        check_ret(ret)?;
        Ok(IdentifyNamespace { inner: id })
    }

    /// Iterate over the multipath paths through which this namespace is
    /// reachable. Empty on non-multipath setups.
    pub fn paths(&self) -> Paths<'r> {
        Paths::from_namespace(self.inner)
    }

    /// Begin building a Format NVM admin command for this namespace.
    ///
    /// # Warning
    ///
    /// **Destructive and irreversible.** Format NVM erases all user data in
    /// the namespace and applies the configured LBA format, protection
    /// settings, and secure-erase mode. There is no undo. The returned
    /// [`Format`] builder is inert until [`Format::execute`] is called —
    /// that's the destructive step. Verify against the QEMU fixture
    /// before pointing this at real hardware.
    ///
    /// ```no_run
    /// # use libnvme::{Root, SecureErase};
    /// # let root = Root::scan()?;
    /// # let ns: libnvme::Namespace<'_> = todo!();
    /// ns.format()
    ///     .lba_format(0)
    ///     .secure_erase(SecureErase::Cryptographic)
    ///     .execute()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn format(&self) -> Format<'_, 'r> {
        Format::new(self)
    }

    // -------- I/O commands (see crate::io) -----------------------------

    /// Raw libnvme namespace handle. `pub(crate)` so the `io` module can
    /// reach it without exposing it externally.
    pub(crate) fn raw_handle(&self) -> nvme_ns_t {
        self.inner
    }

    /// Build a Read command starting at `slba`, reading `nlb` blocks
    /// (1-based) into `buf`. `buf.len()` must equal `nlb * lba_size`.
    pub fn read<'a>(&'a self, slba: u64, nlb: u32, buf: &'a mut [u8]) -> Read<'a, 'r> {
        Read::new(self, slba, nlb, buf)
    }

    /// Convenience: allocate a Vec sized for `nlb` blocks and Read into it.
    /// Defaults apply (no FUA, no protection-info checks). For finer control
    /// use [`Namespace::read`] with your own buffer.
    pub fn read_to_vec(&self, slba: u64, nlb: u32) -> Result<Vec<u8>> {
        let len = (nlb as usize)
            .checked_mul(self.lba_size() as usize)
            .ok_or_else(|| crate::Error::invalid_argument("nlb * lba_size overflows usize"))?;
        let mut buf = vec![0u8; len];
        self.read(slba, nlb, &mut buf).execute()?;
        Ok(buf)
    }

    /// Build a Write command. `buf.len()` must equal `nlb * lba_size`.
    pub fn write<'a>(&'a self, slba: u64, nlb: u32, buf: &'a [u8]) -> Write<'a, 'r> {
        Write::new(self, slba, nlb, buf)
    }

    /// Build a Compare command — compares stored LBAs against `buf`.
    /// Returns [`crate::Error::Nvme`] with status `0x85` if they differ.
    pub fn compare<'a>(&'a self, slba: u64, nlb: u32, buf: &'a [u8]) -> Compare<'a, 'r> {
        Compare::new(self, slba, nlb, buf)
    }

    /// Build a Verify command. No host buffer — the controller reads and
    /// validates the LBA range internally.
    pub fn verify(&self, slba: u64, nlb: u32) -> Verify<'_, 'r> {
        Verify::new(self, slba, nlb)
    }

    /// Build a Write Zeroes command.
    pub fn write_zeroes(&self, slba: u64, nlb: u32) -> WriteZeroes<'_, 'r> {
        WriteZeroes::new(self, slba, nlb)
    }

    /// Build a Write Uncorrectable command. **Destructive:** the named LBAs
    /// become unreadable (return Unrecovered Read Error) until rewritten.
    pub fn write_uncorrectable(&self, slba: u64, nlb: u32) -> WriteUncorrectable<'_, 'r> {
        WriteUncorrectable::new(self, slba, nlb)
    }

    /// Issue a Flush command. Pushes the volatile write cache to media.
    pub fn flush(&self) -> Result<()> {
        io::flush(self)
    }

    /// Build a Dataset Management command. Combine attributes with `|`,
    /// e.g. [`DsmAttr::DEALLOCATE`] for TRIM. Set the LBA ranges via
    /// [`Dsm::ranges`].
    pub fn dsm(&self, attrs: DsmAttr) -> Dsm<'_, 'r> {
        Dsm::new(self, attrs)
    }

    /// Build a Copy command. `sdlba` is the destination LBA; `ranges` are
    /// the source ranges. Up to 128 ranges per command.
    pub fn copy<'a>(&'a self, sdlba: u64, ranges: &'a [CopyRange]) -> Copy<'a, 'r> {
        Copy::new(self, sdlba, ranges)
    }

    // -------- Reservations (see crate::reservations) -------------------

    /// Build a Reservation Acquire command. See
    /// [`ReservationAcquire`](crate::ReservationAcquire) for the chainable
    /// setters.
    pub fn reservation_acquire(&self) -> crate::reservations::ReservationAcquire<'_, 'r> {
        crate::reservations::ReservationAcquire::new(self)
    }

    /// Build a Reservation Register command.
    pub fn reservation_register(&self) -> crate::reservations::ReservationRegister<'_, 'r> {
        crate::reservations::ReservationRegister::new(self)
    }

    /// Build a Reservation Release command.
    pub fn reservation_release(&self) -> crate::reservations::ReservationRelease<'_, 'r> {
        crate::reservations::ReservationRelease::new(self)
    }

    /// Build a Reservation Report command. Supply a buffer via
    /// [`ReservationReport::buffer`](crate::ReservationReport::buffer) (or use
    /// `execute_to_vec`).
    pub fn reservation_report(&self) -> crate::reservations::ReservationReport<'_, 'r> {
        crate::reservations::ReservationReport::new(self)
    }

    // -------- Directives (see crate::directives) -----------------------

    /// Build a Directive Send command.
    pub fn directive_send(
        &self,
        dtype: crate::directives::DirectiveType,
        doper: crate::directives::DirectiveSendOp,
    ) -> crate::directives::DirectiveSend<'_, 'r> {
        crate::directives::DirectiveSend::new(self, dtype, doper)
    }

    /// Build a Directive Receive command.
    pub fn directive_recv(
        &self,
        dtype: crate::directives::DirectiveType,
        doper: crate::directives::DirectiveRecvOp,
    ) -> crate::directives::DirectiveRecv<'_, 'r> {
        crate::directives::DirectiveRecv::new(self, dtype, doper)
    }

    // -------- ZNS (see crate::zns) -------------------------------------

    /// Build a Zone Management Send command.
    pub fn zns_mgmt_send(
        &self,
        slba: u64,
        action: crate::zns::ZoneSendAction,
    ) -> crate::zns::ZnsMgmtSend<'_, 'r> {
        crate::zns::ZnsMgmtSend::new(self, slba, action)
    }

    /// Build a Zone Management Receive command. Supply a buffer via
    /// [`ZnsMgmtRecv::buffer`](crate::ZnsMgmtRecv::buffer).
    pub fn zns_mgmt_recv(&self, slba: u64) -> crate::zns::ZnsMgmtRecv<'_, 'r> {
        crate::zns::ZnsMgmtRecv::new(self, slba)
    }

    /// Build a Zone Append command. Block count is 1-based.
    pub fn zns_append<'a>(
        &'a self,
        zslba: u64,
        nlb: u32,
        data: &'a [u8],
    ) -> crate::zns::ZnsAppend<'a, 'r> {
        crate::zns::ZnsAppend::new(self, zslba, nlb, data)
    }
}

/// Builder for the Format NVM admin command.
///
/// Created via [`Namespace::format`]. All fields default to "no-op /
/// conservative" values (LBA format 0, no secure erase, PI disabled),
/// so calling [`Format::execute`] without further chaining performs a
/// metadata-only format.
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Format<'a, 'r> {
    ns: &'a Namespace<'r>,
    lba_format: u8,
    secure_erase: SecureErase,
    protection_info: ProtectionInfo,
    protection_location: ProtectionLocation,
    metadata: MetadataSettings,
    lba_format_upper: u8,
    timeout_ms: u32,
}

impl<'a, 'r> Format<'a, 'r> {
    fn new(ns: &'a Namespace<'r>) -> Self {
        Format {
            ns,
            lba_format: 0,
            secure_erase: SecureErase::None,
            protection_info: ProtectionInfo::Disabled,
            protection_location: ProtectionLocation::Last,
            metadata: MetadataSettings::Separate,
            lba_format_upper: 0,
            timeout_ms: 0,
        }
    }

    /// Select the LBA format (an index into the array reported by
    /// Identify Namespace). The active format becomes the new namespace
    /// format after the command completes.
    pub fn lba_format(mut self, index: u8) -> Self {
        self.lba_format = index;
        self
    }

    /// For NVMe 2.0+ controllers that expose more than 16 LBA formats, the
    /// upper bits of the format index live here.
    pub fn lba_format_upper(mut self, upper: u8) -> Self {
        self.lba_format_upper = upper;
        self
    }

    /// Configure secure-erase behaviour.
    pub fn secure_erase(mut self, mode: SecureErase) -> Self {
        self.secure_erase = mode;
        self
    }

    /// Configure end-to-end data protection.
    pub fn protection_info(mut self, pi: ProtectionInfo) -> Self {
        self.protection_info = pi;
        self
    }

    /// Where PI guard bytes sit within metadata (only meaningful when
    /// `protection_info` is not [`ProtectionInfo::Disabled`]).
    pub fn protection_location(mut self, location: ProtectionLocation) -> Self {
        self.protection_location = location;
        self
    }

    /// Whether metadata is separate or interleaved with LBA data.
    pub fn metadata(mut self, settings: MetadataSettings) -> Self {
        self.metadata = settings;
        self
    }

    /// Per-command timeout in milliseconds. `0` (the default) means the
    /// libnvme/kernel default — usually plenty for any reasonable format,
    /// but on large drives with `SecureErase::UserData` this can be raised.
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Execute the Format NVM command. Blocks until the controller reports
    /// completion or returns an error.
    pub fn execute(self) -> Result<()> {
        // SAFETY: self.ns.inner is a non-null nvme_ns_t tied to 'r; libnvme
        // opens the device lazily and returns -1 on failure.
        let fd = unsafe { libnvme_sys::nvme_ns_get_fd(self.ns.inner) };
        if fd < 0 {
            return Err(crate::Error::Os(std::io::Error::last_os_error()));
        }
        let mut args = nvme_format_nvm_args {
            result: std::ptr::null_mut(),
            args_size: std::mem::size_of::<nvme_format_nvm_args>() as i32,
            fd,
            timeout: self.timeout_ms,
            nsid: self.ns.nsid(),
            mset: self.metadata.as_raw(),
            pi: self.protection_info.as_raw(),
            pil: self.protection_location.as_raw(),
            ses: self.secure_erase.as_raw(),
            lbaf: self.lba_format,
            rsvd1: [0; 7],
            lbafu: self.lba_format_upper,
        };
        // SAFETY: args is fully-initialized on the stack; fd is valid; no
        // pointer fields reference external memory.
        let ret = unsafe { nvme_format_nvm(&mut args) };
        check_ret(ret)
    }
}

impl std::fmt::Debug for Namespace<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Namespace")
            .field("name", &self.name().ok())
            .field("nsid", &self.nsid())
            .field("size_bytes", &self.size_bytes())
            .field("lba_size", &self.lba_size())
            .finish()
    }
}

/// Iterator over [`Namespace`] entries reachable through a [`crate::Controller`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Namespaces<'r> {
    ctrl: nvme_ctrl_t,
    cursor: nvme_ns_t,
    _marker: PhantomData<&'r Root>,
}

impl<'r> Namespaces<'r> {
    pub(crate) fn new(ctrl: nvme_ctrl_t) -> Self {
        // SAFETY: ctrl is a valid non-null nvme_ctrl_t from the libnvme tree,
        // tied to 'r; iterator helpers return NULL when there are no children.
        let cursor = unsafe { nvme_ctrl_first_ns(ctrl) };
        Namespaces {
            ctrl,
            cursor,
            _marker: PhantomData,
        }
    }
}

impl<'r> Iterator for Namespaces<'r> {
    type Item = Namespace<'r>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_null() {
            return None;
        }
        let current = self.cursor;
        // SAFETY: self.ctrl and current are valid non-null handles from the
        // same libnvme tree, tied to 'r; libnvme returns NULL at end-of-list.
        self.cursor = unsafe { nvme_ctrl_next_ns(self.ctrl, current) };
        Some(Namespace::from_raw(current))
    }
}
