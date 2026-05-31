//! Identify Controller and Identify Namespace data structures.
//!
//! These are the 4096-byte payloads returned by the NVMe Identify admin
//! command (CNS = 01h and 00h respectively). libnvme issues the command via
//! `nvme_ctrl_identify` / `nvme_ns_identify`; the returned struct is wrapped
//! here with decoded accessors.

use libnvme_sys::{nvme_id_ctrl, nvme_id_ns, nvme_lbaf};

use crate::util::fixed_ascii_to_str;
use crate::Result;

/// Decoded NVMe specification version, as reported by the controller's `VER`
/// register and the `vs` field of Identify Controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvmeVersion {
    /// Major version number (MJR).
    pub major: u16,
    /// Minor version number (MNR).
    pub minor: u8,
    /// Tertiary version number (TER).
    pub tertiary: u8,
}

impl NvmeVersion {
    pub(crate) fn from_raw(ver: u32) -> Self {
        NvmeVersion {
            major: (ver >> 16) as u16,
            minor: (ver >> 8) as u8,
            tertiary: ver as u8,
        }
    }
}

impl std::fmt::Display for NvmeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.tertiary)
    }
}

/// One LBA format entry from Identify Namespace.
///
/// A namespace declares up to 64 supported formats; the active one is selected
/// by the lower bits of `flbas`. `data_size_bytes` is `2 ^ ds` of the raw
/// descriptor; a value of `0` means the format is unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LbaFormat {
    /// Metadata size in bytes per LBA (MS).
    pub metadata_size: u16,
    /// Logical block data size in bytes (`2 ^ ds`); `0` if unsupported.
    pub data_size_bytes: u32,
    /// Relative Performance (RP) — lower is better (2-bit field).
    pub relative_performance: u8,
}

impl LbaFormat {
    pub(crate) fn from_raw(raw: nvme_lbaf) -> Self {
        let data_size_bytes = if raw.ds == 0 { 0 } else { 1u32 << raw.ds };
        LbaFormat {
            metadata_size: raw.ms,
            data_size_bytes,
            relative_performance: raw.rp & 0x3,
        }
    }
}

/// Decoded Identify Controller (CNS 01h) data structure.
///
/// Owns the 4096-byte buffer returned by libnvme; accessors decode individual
/// fields on demand. Field naming uses the NVMe spec abbreviations where
/// they are widely known (e.g. `vendor_id`, not `vid`).
pub struct IdentifyController {
    pub(crate) inner: Box<nvme_id_ctrl>,
}

impl IdentifyController {
    /// PCI vendor ID (VID).
    pub fn vendor_id(&self) -> u16 {
        self.inner.vid
    }

    /// PCI subsystem vendor ID (SSVID).
    pub fn subsystem_vendor_id(&self) -> u16 {
        self.inner.ssvid
    }

    /// 20-byte ASCII serial number, trimmed.
    pub fn serial_number(&self) -> Result<&str> {
        fixed_ascii_to_str(&self.inner.sn)
    }

    /// 40-byte ASCII model number, trimmed.
    pub fn model_number(&self) -> Result<&str> {
        fixed_ascii_to_str(&self.inner.mn)
    }

    /// 8-byte ASCII firmware revision, trimmed.
    pub fn firmware_revision(&self) -> Result<&str> {
        fixed_ascii_to_str(&self.inner.fr)
    }

    /// IEEE OUI (3-byte company identifier), in little-endian byte order.
    pub fn ieee_oui(&self) -> [u8; 3] {
        self.inner.ieee
    }

    /// Maximum data transfer size, as an exponent of the controller's minimum
    /// memory-page size. `0` means no limit.
    pub fn max_data_transfer_size_exp(&self) -> u8 {
        self.inner.mdts
    }

    /// 16-bit controller identifier.
    pub fn controller_id(&self) -> u16 {
        self.inner.cntlid
    }

    /// NVMe specification version supported by the controller.
    pub fn nvme_version(&self) -> NvmeVersion {
        NvmeVersion::from_raw(self.inner.ver)
    }

    /// Controller type: `1` = I/O, `2` = Discovery, `3` = Administrative.
    pub fn controller_type(&self) -> u8 {
        self.inner.cntrltype
    }

    /// 16-byte FRU Globally Unique Identifier.
    pub fn fru_guid(&self) -> [u8; 16] {
        self.inner.fguid
    }

    /// Optional Admin Command Support bitfield (OACS).
    pub fn optional_admin_command_support(&self) -> u16 {
        self.inner.oacs
    }

    /// Abort command limit (ACL).
    pub fn abort_command_limit(&self) -> u8 {
        self.inner.acl
    }

    /// Async event request limit (AERL).
    pub fn async_event_request_limit(&self) -> u8 {
        self.inner.aerl
    }

    /// Firmware updates bitfield (FRMW).
    pub fn firmware_updates(&self) -> u8 {
        self.inner.frmw
    }

    /// Log Page Attributes bitfield (LPA).
    pub fn log_page_attributes(&self) -> u8 {
        self.inner.lpa
    }

    /// Number of supported error log page entries (ELPE).
    pub fn error_log_page_entries(&self) -> u8 {
        self.inner.elpe
    }

    /// Number of power states supported, zero-based (NPSS).
    pub fn num_power_states(&self) -> u8 {
        self.inner.npss
    }

    /// Warning composite temperature threshold in Kelvin (WCTEMP).
    pub fn warning_temp_threshold_kelvin(&self) -> u16 {
        self.inner.wctemp
    }

    /// Critical composite temperature threshold in Kelvin (CCTEMP).
    pub fn critical_temp_threshold_kelvin(&self) -> u16 {
        self.inner.cctemp
    }

    /// Host Memory Buffer preferred size in 4 KiB units (HMPRE).
    pub fn host_memory_buffer_preferred_size(&self) -> u32 {
        self.inner.hmpre
    }

    /// Host Memory Buffer minimum size in 4 KiB units (HMMIN).
    pub fn host_memory_buffer_min_size(&self) -> u32 {
        self.inner.hmmin
    }

    /// Total NVM Capacity in bytes (TNVMCAP), as a 128-bit little-endian integer.
    pub fn total_nvm_capacity_bytes(&self) -> u128 {
        u128::from_le_bytes(self.inner.tnvmcap)
    }

    /// Unallocated NVM Capacity in bytes (UNVMCAP).
    pub fn unallocated_nvm_capacity_bytes(&self) -> u128 {
        u128::from_le_bytes(self.inner.unvmcap)
    }

    /// Submission Queue Entry Size encoding (SQES).
    ///
    /// Bits 0–3: required size; bits 4–7: maximum size. The actual byte size
    /// is `2 ^ value`.
    pub fn submission_queue_entry_size(&self) -> u8 {
        self.inner.sqes
    }

    /// Completion Queue Entry Size encoding (CQES).
    pub fn completion_queue_entry_size(&self) -> u8 {
        self.inner.cqes
    }

    /// Maximum outstanding commands (MAXCMD). `0` means no limit reported.
    pub fn max_commands_outstanding(&self) -> u16 {
        self.inner.maxcmd
    }

    /// Number of namespaces supported (NN).
    pub fn num_namespaces(&self) -> u32 {
        self.inner.nn
    }
}

impl std::fmt::Debug for IdentifyController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentifyController")
            .field("vendor_id", &format_args!("0x{:04x}", self.vendor_id()))
            .field("model_number", &self.model_number().ok())
            .field("serial_number", &self.serial_number().ok())
            .field("firmware_revision", &self.firmware_revision().ok())
            .field("nvme_version", &self.nvme_version())
            .field("num_namespaces", &self.num_namespaces())
            .finish()
    }
}

/// Decoded Identify Namespace (CNS 00h) data structure.
pub struct IdentifyNamespace {
    pub(crate) inner: Box<nvme_id_ns>,
}

impl IdentifyNamespace {
    /// Namespace size in logical blocks (NSZE).
    pub fn size_lbas(&self) -> u64 {
        self.inner.nsze
    }

    /// Namespace capacity in logical blocks (NCAP).
    pub fn capacity_lbas(&self) -> u64 {
        self.inner.ncap
    }

    /// Namespace utilization in logical blocks (NUSE).
    pub fn utilization_lbas(&self) -> u64 {
        self.inner.nuse
    }

    /// Namespace features bitfield (NSFEAT).
    pub fn features(&self) -> u8 {
        self.inner.nsfeat
    }

    /// Number of supported LBA formats, zero-based (NLBAF).
    pub fn num_lba_formats(&self) -> u8 {
        self.inner.nlbaf
    }

    /// Formatted LBA Size byte (FLBAS) — encodes which LBA format is active
    /// in the lower 4 bits (combined with NULBAF bits for extended encoding).
    pub fn formatted_lba_size(&self) -> u8 {
        self.inner.flbas
    }

    /// Metadata Capabilities bitfield (MC).
    pub fn metadata_capabilities(&self) -> u8 {
        self.inner.mc
    }

    /// End-to-end Data Protection Capabilities (DPC).
    pub fn data_protection_capabilities(&self) -> u8 {
        self.inner.dpc
    }

    /// End-to-end Data Protection Type Settings (DPS).
    pub fn data_protection_setting(&self) -> u8 {
        self.inner.dps
    }

    /// Namespace Multi-path I/O and Namespace Sharing Capabilities (NMIC).
    pub fn multipath_capabilities(&self) -> u8 {
        self.inner.nmic
    }

    /// Reservation Capabilities bitfield (RESCAP).
    pub fn reservation_capabilities(&self) -> u8 {
        self.inner.rescap
    }

    /// NVM Capacity in bytes (NVMCAP), 128-bit little-endian.
    pub fn nvm_capacity_bytes(&self) -> u128 {
        u128::from_le_bytes(self.inner.nvmcap)
    }

    /// Namespace Atomic Write Unit Normal (NAWUN), in LBAs minus one.
    pub fn atomic_write_unit_normal(&self) -> u16 {
        self.inner.nawun
    }

    /// 128-bit Namespace Globally Unique Identifier.
    pub fn nguid(&self) -> [u8; 16] {
        self.inner.nguid
    }

    /// 64-bit IEEE Extended Unique Identifier.
    pub fn eui64(&self) -> [u8; 8] {
        self.inner.eui64
    }

    /// Look up an LBA format by index. Returns `None` if `index` is out of
    /// range (>= 64) or the format slot is unsupported (`data_size_bytes == 0`).
    pub fn lba_format(&self, index: u8) -> Option<LbaFormat> {
        let raw = *self.inner.lbaf.get(usize::from(index))?;
        let format = LbaFormat::from_raw(raw);
        if format.data_size_bytes == 0 {
            None
        } else {
            Some(format)
        }
    }

    /// Active LBA format, as selected by the low bits of `flbas`.
    pub fn current_lba_format(&self) -> Option<LbaFormat> {
        // NVMe 2.0+: FLBAS bits 0..3 are the lower 4 bits of the index, bits 5..6
        // are bits 4..5 of the index for >16 formats. The classic 4-bit form
        // covers all common consumer SSDs.
        let idx = self.formatted_lba_size() & 0x0F;
        self.lba_format(idx)
    }
}

impl std::fmt::Debug for IdentifyNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentifyNamespace")
            .field("size_lbas", &self.size_lbas())
            .field("capacity_lbas", &self.capacity_lbas())
            .field("utilization_lbas", &self.utilization_lbas())
            .field("current_lba_format", &self.current_lba_format())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_decodes_bit_layout() {
        // NVMe 1.4.2 → VER register 0x00010402
        let v = NvmeVersion::from_raw(0x00010402);
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 4);
        assert_eq!(v.tertiary, 2);
        assert_eq!(format!("{v}"), "1.4.2");
    }

    #[test]
    fn version_decodes_nvme_2_0() {
        let v = NvmeVersion::from_raw(0x00020000);
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 0);
        assert_eq!(v.tertiary, 0);
    }

    #[test]
    fn lba_format_decodes_exponent() {
        let raw = nvme_lbaf {
            ms: 0,
            ds: 12, // 2^12 = 4096
            rp: 0,
        };
        let f = LbaFormat::from_raw(raw);
        assert_eq!(f.data_size_bytes, 4096);
        assert_eq!(f.relative_performance, 0);
    }

    #[test]
    fn lba_format_zero_ds_yields_zero_size() {
        let raw = nvme_lbaf {
            ms: 0,
            ds: 0,
            rp: 0,
        };
        assert_eq!(LbaFormat::from_raw(raw).data_size_bytes, 0);
    }

    #[test]
    fn lba_format_masks_relative_performance() {
        let raw = nvme_lbaf {
            ms: 0,
            ds: 9,
            rp: 0xFB, // upper bits should be ignored
        };
        assert_eq!(LbaFormat::from_raw(raw).relative_performance, 0x03);
    }
}
