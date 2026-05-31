//! Standalone admin commands.
//!
//! Sanitize, Device Self-Test, Security Send/Receive, Lockdown, Get LBA
//! Status, Set/Get Property (Fabrics), and the generic Admin/IO Passthru
//! escape hatches. Each command lives as a method on [`Controller`] in this
//! file (via a separate `impl` block) to keep `controller.rs` focused on
//! enumeration + properties + the higher-traffic admin commands.

use libnvme_sys::{
    nvme_admin_passthru, nvme_dev_self_test, nvme_dev_self_test_args, nvme_get_lba_status,
    nvme_get_lba_status_args, nvme_get_property, nvme_get_property_args, nvme_io_passthru,
    nvme_lockdown, nvme_lockdown_args, nvme_sanitize_nvm, nvme_sanitize_nvm_args,
    nvme_security_receive, nvme_security_receive_args, nvme_security_send, nvme_security_send_args,
    nvme_set_property, nvme_set_property_args,
};

use crate::admin::{SanitizeAction, SelfTestAction};
use crate::error::check_ret;
use crate::{Controller, Result};

/// Builder for the Sanitize NVM admin command.
///
/// Created by [`Controller::sanitize`]. Defaults to `BlockErase` with no
/// AUSE, no overwrite passes, no invert, allow deallocation. Tune via the
/// chainable setters then call [`Sanitize::execute`].
#[must_use = "this builder does nothing until `.execute()` is called"]
pub struct Sanitize<'a, 'r> {
    ctrl: &'a Controller<'r>,
    action: SanitizeAction,
    ause: bool,
    owpass: u8,
    oipbp: bool,
    nodas: bool,
    emvs: bool,
    ovrpat: u32,
    timeout_ms: u32,
}

impl<'a, 'r> Sanitize<'a, 'r> {
    pub(crate) fn new(ctrl: &'a Controller<'r>) -> Self {
        Sanitize {
            ctrl,
            action: SanitizeAction::BlockErase,
            ause: false,
            owpass: 0,
            oipbp: false,
            nodas: false,
            emvs: false,
            ovrpat: 0,
            timeout_ms: 0,
        }
    }

    /// Select which sanitize action to perform.
    pub fn action(mut self, action: SanitizeAction) -> Self {
        self.action = action;
        self
    }

    /// Allow Unrestricted Sanitize Exit (AUSE) — permits operations on the
    /// drive between Sanitize start and completion.
    pub fn ause(mut self, ause: bool) -> Self {
        self.ause = ause;
        self
    }

    /// Number of overwrite passes (only meaningful for `Overwrite` action).
    /// `0` lets the drive choose; values 1–15 are explicit.
    pub fn overwrite_pass_count(mut self, n: u8) -> Self {
        self.owpass = n;
        self
    }

    /// Overwrite Invert Between Passes — flip the pattern every other pass.
    pub fn overwrite_invert(mut self, oipbp: bool) -> Self {
        self.oipbp = oipbp;
        self
    }

    /// No Deallocate After Sanitize — when true, post-sanitize LBAs report
    /// the configured pattern instead of being deallocated.
    pub fn no_deallocate_after(mut self, nodas: bool) -> Self {
        self.nodas = nodas;
        self
    }

    /// Emulated Media Verify (NVMe 2.0+).
    pub fn emulated_media_verify(mut self, emvs: bool) -> Self {
        self.emvs = emvs;
        self
    }

    /// 32-bit overwrite pattern. Only used when action is `Overwrite`.
    pub fn overwrite_pattern(mut self, pattern: u32) -> Self {
        self.ovrpat = pattern;
        self
    }

    /// Per-command timeout in milliseconds. `0` uses libnvme's default
    /// (already very long for sanitize — overwrites can take hours on big
    /// drives).
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Issue the Sanitize NVM command. Returns the controller's completion
    /// result dword (CDW0) once the controller acknowledges the start; the
    /// actual sanitize runs asynchronously and progress is tracked via the
    /// Sanitize Status log page.
    ///
    /// Returns `Result<u32>` for consistency with the other command builders
    /// in this crate (`io`, `reservations`, `zns`). The dword is rarely
    /// load-bearing for Sanitize, but surfacing it costs nothing and avoids
    /// the lone `Result<()>` builder outlier.
    // Field reassignment after Default::default() is deliberate — the
    // `emvs` field is cfg-gated and a struct literal can't conditionally
    // omit a field, so we use Default + per-field assignment instead.
    #[allow(clippy::field_reassign_with_default)]
    pub fn execute(self) -> Result<u32> {
        let fd = self.ctrl.open_fd()?;
        let mut result: u32 = 0;
        let mut args = nvme_sanitize_nvm_args::default();
        args.args_size = std::mem::size_of::<nvme_sanitize_nvm_args>() as i32;
        args.fd = fd;
        args.result = &mut result;
        args.timeout = self.timeout_ms;
        args.sanact = self.action.as_raw();
        args.ovrpat = self.ovrpat;
        args.ause = self.ause;
        args.owpass = self.owpass;
        args.oipbp = self.oipbp;
        args.nodas = self.nodas;
        #[cfg(has_sanitize_emvs)]
        {
            args.emvs = self.emvs;
        }
        // SAFETY: args is fully-initialized on the stack; fd is a valid file
        // descriptor for this controller; result points to a live u32.
        let ret = unsafe { nvme_sanitize_nvm(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }
}

/// Parameters for the Lockdown admin command (NVMe 2.0+).
///
/// See the NVMe 2.0 spec §5.20 for the semantics of each field. The
/// short version: `scp` is the scope class (commands / features /
/// log pages / etc.), `prhbt` toggles between prohibit and allow,
/// `ifc` is the interface mask, `ofi` is the opcode/feature id, and
/// `uuidx` is the UUID index for vendor-specific lockdown scopes.
#[derive(Debug, Default, Clone, Copy)]
pub struct LockdownArgs {
    /// Scope (SCP) — lockdown scope class (commands / features / log pages).
    pub scope: u8,
    /// Prohibit (PRHBT) — `1` prohibits, `0` allows the scoped capability.
    pub prohibit: u8,
    /// Interface (IFC) — interface mask the lockdown applies to.
    pub interface: u8,
    /// Opcode/Feature Identifier (OFI) — the opcode or feature id in scope.
    pub opcode_or_fid: u8,
    /// UUID Index (UUIDX) — UUID index for vendor-specific lockdown scopes.
    pub uuid_index: u8,
    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub timeout_ms: u32,
}

/// Parameters for Get LBA Status (NVMe 1.4+).
#[derive(Debug, Default, Clone, Copy)]
pub struct GetLbaStatusArgs {
    /// Namespace Identifier (NSID) to query.
    pub nsid: u32,
    /// Starting LBA (SLBA) of the range to report on.
    pub slba: u64,
    /// Maximum Number of Dwords (MNDW) the response buffer can hold.
    pub mndw: u32,
    /// Action Type (ATYPE) — selects which LBA status is reported.
    pub atype: u32,
    /// Range Length (RL) in logical blocks.
    pub rl: u16,
    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub timeout_ms: u32,
}

/// Parameters for `Controller::admin_passthru` / `Controller::io_passthru`.
///
/// Mirrors libnvme's procedural-style passthru API: every command-dword
/// is exposed verbatim. Use [`PassthruArgs::default`] then set only the
/// dwords you need.
///
/// `data` (and `metadata`, where supported) are filled in by reference
/// to byte buffers; their `_len` parameters are derived automatically from
/// the slice lengths.
#[derive(Debug, Default)]
pub struct PassthruArgs<'a> {
    /// Command opcode (OPC).
    pub opcode: u8,
    /// Command flags (FLAGS) — the command-dword-0 flags field.
    pub flags: u8,
    /// Reserved field (RSVD).
    pub rsvd: u16,
    /// Namespace Identifier (NSID).
    pub nsid: u32,
    /// Command Dword 2 (CDW2).
    pub cdw2: u32,
    /// Command Dword 3 (CDW3).
    pub cdw3: u32,
    /// Command Dword 10 (CDW10).
    pub cdw10: u32,
    /// Command Dword 11 (CDW11).
    pub cdw11: u32,
    /// Command Dword 12 (CDW12).
    pub cdw12: u32,
    /// Command Dword 13 (CDW13).
    pub cdw13: u32,
    /// Command Dword 14 (CDW14).
    pub cdw14: u32,
    /// Command Dword 15 (CDW15).
    pub cdw15: u32,
    /// Data buffer; its length supplies the data transfer length.
    pub data: Option<&'a mut [u8]>,
    /// Metadata buffer; its length supplies the metadata transfer length.
    pub metadata: Option<&'a mut [u8]>,
    /// Per-command timeout in milliseconds. `0` uses libnvme's default.
    pub timeout_ms: u32,
}

impl<'r> Controller<'r> {
    /// Begin building a Sanitize NVM command.
    ///
    /// # Warning
    ///
    /// **Destructive and irreversible.** Sanitize erases all user data on
    /// the drive in a way that satisfies the NVMe spec's media-clear
    /// guarantees. Cannot be aborted once started; runs asynchronously
    /// (the command returns immediately and the operation continues in
    /// the background). The drive remains in a sanitize-in-progress state
    /// until completion — most operations are rejected during this time.
    /// Verify against the QEMU fixture before running against real
    /// hardware.
    pub fn sanitize(&self) -> Sanitize<'_, 'r> {
        Sanitize::new(self)
    }

    /// Issue a Device Self-Test admin command.
    ///
    /// `nsid` of `0xFFFFFFFF` runs the test on all namespaces. Use
    /// [`SelfTestAction::Abort`] with any `nsid` to abort a running test.
    pub fn self_test(&self, nsid: u32, action: SelfTestAction) -> Result<()> {
        let fd = self.open_fd()?;
        let mut args = nvme_dev_self_test_args {
            result: std::ptr::null_mut(),
            args_size: std::mem::size_of::<nvme_dev_self_test_args>() as i32,
            fd,
            timeout: 0,
            nsid,
            stc: action.as_raw(),
        };
        // SAFETY: args is fully-initialized on the stack; fd is a valid file
        // descriptor for this controller; result pointer is NULL as we don't
        // need the CQE result here.
        let ret = unsafe { nvme_dev_self_test(&mut args) };
        check_ret(ret)
    }

    /// Issue a Lockdown admin command (NVMe 2.0+).
    ///
    /// Restricts or unlocks specific NVMe interface capabilities — see
    /// [`LockdownArgs`] for the parameter encoding.
    ///
    /// # Warning
    ///
    /// **Potentially destructive.** Lockdown can disable management
    /// interfaces (out-of-band MI, debug paths) or restrict specific
    /// opcodes; some lockdown scopes are persistent across resets and
    /// cannot be undone without a full controller reset or vendor unlock.
    /// Read the controller's lockdown-support reporting (Identify
    /// Controller CAP3) and the NVMe-MI 1.2 spec sections on Lockdown
    /// before calling.
    pub fn lockdown(&self, args: LockdownArgs) -> Result<()> {
        let fd = self.open_fd()?;
        let mut raw = nvme_lockdown_args {
            result: std::ptr::null_mut(),
            args_size: std::mem::size_of::<nvme_lockdown_args>() as i32,
            fd,
            timeout: args.timeout_ms,
            scp: args.scope,
            prhbt: args.prohibit,
            ifc: args.interface,
            ofi: args.opcode_or_fid,
            uuidx: args.uuid_index,
        };
        // SAFETY: raw is fully-initialized on the stack; fd is a valid file
        // descriptor for this controller; no pointer fields are used.
        let ret = unsafe { nvme_lockdown(&mut raw) };
        check_ret(ret)
    }

    /// Send a Security Send command — transport-encapsulated bytes go to
    /// the controller's security protocol handler (TCG Opal, TCG Pyrite,
    /// etc.).
    ///
    /// `secp` is the security protocol identifier (0x01 = TCG, 0x02 = TCG
    /// Storage, etc.). `spsp0` and `spsp1` are protocol-specific parameter
    /// bytes. `nssf` is the security-protocol-specific field.
    #[allow(clippy::too_many_arguments)]
    pub fn security_send(
        &self,
        nsid: u32,
        secp: u8,
        spsp0: u8,
        spsp1: u8,
        nssf: u8,
        tl: u32,
        data: &mut [u8],
    ) -> Result<u32> {
        let fd = self.open_fd()?;
        let mut result = 0u32;
        let mut args = nvme_security_send_args {
            result: &mut result,
            data: data.as_mut_ptr() as *mut std::ffi::c_void,
            args_size: std::mem::size_of::<nvme_security_send_args>() as i32,
            fd,
            timeout: 0,
            nsid,
            tl,
            data_len: data.len() as u32,
            nssf,
            spsp0,
            spsp1,
            secp,
        };
        // SAFETY: args is fully-initialized on the stack; fd is a valid file
        // descriptor for this controller; data points into the caller's
        // mutable slice (alive for the call) and data_len matches its length;
        // result is a valid &mut u32.
        let ret = unsafe { nvme_security_send(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }

    /// Issue a Security Receive command — pulls security-protocol data
    /// back from the controller into `data`.
    #[allow(clippy::too_many_arguments)]
    pub fn security_receive(
        &self,
        nsid: u32,
        secp: u8,
        spsp0: u8,
        spsp1: u8,
        nssf: u8,
        al: u32,
        data: &mut [u8],
    ) -> Result<u32> {
        let fd = self.open_fd()?;
        let mut result = 0u32;
        let mut args = nvme_security_receive_args {
            result: &mut result,
            data: data.as_mut_ptr() as *mut std::ffi::c_void,
            args_size: std::mem::size_of::<nvme_security_receive_args>() as i32,
            fd,
            timeout: 0,
            nsid,
            al,
            data_len: data.len() as u32,
            nssf,
            spsp0,
            spsp1,
            secp,
        };
        // SAFETY: args is fully-initialized on the stack; fd is a valid file
        // descriptor for this controller; data points into the caller's
        // mutable slice (alive for the call) and data_len matches its length;
        // result is a valid &mut u32.
        let ret = unsafe { nvme_security_receive(&mut args) };
        check_ret(ret)?;
        Ok(result)
    }

    /// Issue a Get LBA Status admin command (NVMe 1.4+).
    ///
    /// Returns the raw response bytes; parsing the LBA Status Descriptor
    /// list per NVMe spec §5.13 is up to the caller. Buffer must be
    /// sized for at least `mndw * 4` bytes.
    pub fn get_lba_status(&self, args: GetLbaStatusArgs, buf: &mut [u8]) -> Result<u32> {
        let fd = self.open_fd()?;
        let mut result = 0u32;
        let mut raw = nvme_get_lba_status_args {
            lbas: buf.as_mut_ptr() as *mut _,
            result: &mut result,
            slba: args.slba,
            args_size: std::mem::size_of::<nvme_get_lba_status_args>() as i32,
            fd,
            timeout: args.timeout_ms,
            nsid: args.nsid,
            mndw: args.mndw,
            atype: args.atype,
            rl: args.rl,
        };
        // SAFETY: raw is fully-initialized on the stack; fd is valid; lbas
        // points into the caller's mutable buffer alive for the call;
        // result is a valid &mut u32.
        let ret = unsafe { nvme_get_lba_status(&mut raw) };
        check_ret(ret)?;
        Ok(result)
    }

    /// Set a Fabrics controller property — writes to a controller
    /// register (CC, CSTS, NSSR, AQA, ASQ, ACQ, etc.).
    ///
    /// Only valid on fabrics controllers. `offset` is the register byte
    /// offset, `value` is the 64-bit value to write.
    pub fn set_property(&self, offset: i32, value: u64) -> Result<()> {
        let fd = self.open_fd()?;
        let mut args = nvme_set_property_args {
            result: std::ptr::null_mut(),
            args_size: std::mem::size_of::<nvme_set_property_args>() as i32,
            fd,
            timeout: 0,
            offset,
            value,
        };
        // SAFETY: args is fully-initialized on the stack; fd is a valid file
        // descriptor for this controller; no pointer fields are used.
        let ret = unsafe { nvme_set_property(&mut args) };
        check_ret(ret)
    }

    /// Get a Fabrics controller property — reads a controller register.
    ///
    /// Returns the 64-bit register value. Only valid on fabrics
    /// controllers.
    pub fn get_property(&self, offset: i32) -> Result<u64> {
        let fd = self.open_fd()?;
        let mut value: u64 = 0;
        let mut args = nvme_get_property_args {
            value: &mut value,
            args_size: std::mem::size_of::<nvme_get_property_args>() as i32,
            fd,
            timeout: 0,
            offset,
        };
        // SAFETY: args is fully-initialized on the stack; fd is a valid file
        // descriptor for this controller; args.value points to local `value`
        // (alive for the call) where libnvme writes the register read.
        let ret = unsafe { nvme_get_property(&mut args) };
        check_ret(ret)?;
        Ok(value)
    }

    /// Issue an arbitrary admin-class command (Admin Passthru).
    ///
    /// Escape hatch for admin commands not exposed by typed helpers — pass
    /// the opcode and command-dwords, optionally with data/metadata buffers.
    /// Returns the 32-bit completion-queue result dword.
    pub fn admin_passthru(&self, args: PassthruArgs<'_>) -> Result<u32> {
        let fd = self.open_fd()?;
        let mut result = 0u32;
        let (data_ptr, data_len) = match args.data {
            Some(buf) => (buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        let (md_ptr, md_len) = match args.metadata {
            Some(buf) => (buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        // SAFETY: fd is a valid file descriptor for this controller; data_ptr
        // and md_ptr are either NULL with zero length or point into the
        // caller's mutable buffers alive for the call; result is a valid
        // &mut u32 alive for the call.
        let ret = unsafe {
            nvme_admin_passthru(
                fd,
                args.opcode,
                args.flags,
                args.rsvd,
                args.nsid,
                args.cdw2,
                args.cdw3,
                args.cdw10,
                args.cdw11,
                args.cdw12,
                args.cdw13,
                args.cdw14,
                args.cdw15,
                data_len,
                data_ptr,
                md_len,
                md_ptr,
                args.timeout_ms,
                &mut result,
            )
        };
        check_ret(ret)?;
        Ok(result)
    }

    /// Issue an arbitrary I/O-class command (I/O Passthru).
    ///
    /// Escape hatch for I/O commands. Same shape as
    /// [`Self::admin_passthru`] but goes to the I/O command set.
    pub fn io_passthru(&self, args: PassthruArgs<'_>) -> Result<u32> {
        let fd = self.open_fd()?;
        let mut result = 0u32;
        let (data_ptr, data_len) = match args.data {
            Some(buf) => (buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        let (md_ptr, md_len) = match args.metadata {
            Some(buf) => (buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        // SAFETY: fd is a valid file descriptor for this controller; data_ptr
        // and md_ptr are either NULL with zero length or point into the
        // caller's mutable buffers alive for the call; result is a valid
        // &mut u32 alive for the call.
        let ret = unsafe {
            nvme_io_passthru(
                fd,
                args.opcode,
                args.flags,
                args.rsvd,
                args.nsid,
                args.cdw2,
                args.cdw3,
                args.cdw10,
                args.cdw11,
                args.cdw12,
                args.cdw13,
                args.cdw14,
                args.cdw15,
                data_len,
                data_ptr,
                md_len,
                md_ptr,
                args.timeout_ms,
                &mut result,
            )
        };
        check_ret(ret)?;
        Ok(result)
    }
}
