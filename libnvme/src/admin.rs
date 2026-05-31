//! Typed parameters for NVMe admin commands.
//!
//! Each enum corresponds to one of libnvme's `nvme_cmd_*` typedefs (which
//! bindgen exposes as `c_uint` aliases). Using these instead of raw integers
//! at the API surface catches typos at compile time and makes call sites
//! self-documenting.

use libnvme_sys::{
    nvme_cmd_format_mset, nvme_cmd_format_pi, nvme_cmd_format_pil, nvme_cmd_format_ses,
    nvme_dst_stc, nvme_fw_commit_ca, nvme_get_features_sel, nvme_sanitize_sanact,
};

/// How user data should be erased during a Format NVM operation.
///
/// `#[non_exhaustive]`: the NVMe `SES` field has reserved encodings the
/// spec may assign later, so match with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u8)]
pub enum SecureErase {
    /// No secure erase — only metadata is overwritten.
    #[default]
    None = 0,
    /// User-data block-by-block erase. Time scales with namespace size.
    UserData = 1,
    /// Cryptographic erase — destroys the media-encryption key.
    /// Effectively instantaneous regardless of capacity.
    Cryptographic = 2,
}

impl SecureErase {
    pub(crate) fn as_raw(self) -> nvme_cmd_format_ses {
        self as u8 as nvme_cmd_format_ses
    }
}

/// End-to-end Data Protection type to apply when formatting.
///
/// `#[non_exhaustive]`: the NVMe `PI` field may gain types; match with a
/// `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u8)]
pub enum ProtectionInfo {
    /// PI disabled (the common case for consumer SSDs).
    #[default]
    Disabled = 0,
    /// PI Type 1 (Guard tag, Application tag, Reference tag).
    Type1 = 1,
    /// PI Type 2.
    Type2 = 2,
    /// PI Type 3.
    Type3 = 3,
}

impl ProtectionInfo {
    pub(crate) fn as_raw(self) -> nvme_cmd_format_pi {
        self as u8 as nvme_cmd_format_pi
    }
}

/// Where the PI guard bytes sit within each LBA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ProtectionLocation {
    /// PI in the last 8 bytes of metadata (the common choice).
    #[default]
    Last = 0,
    /// PI in the first 8 bytes of metadata.
    First = 1,
}

impl ProtectionLocation {
    pub(crate) fn as_raw(self) -> nvme_cmd_format_pil {
        self as u8 as nvme_cmd_format_pil
    }
}

/// How metadata is transferred relative to LBA data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum MetadataSettings {
    /// Metadata passed through a separate buffer.
    #[default]
    Separate = 0,
    /// Metadata interleaved with LBA data (extended LBA format).
    Extended = 1,
}

impl MetadataSettings {
    pub(crate) fn as_raw(self) -> nvme_cmd_format_mset {
        self as u8 as nvme_cmd_format_mset
    }
}

/// What the firmware-commit admin command should do with the slot it targets.
///
/// `#[non_exhaustive]`: the NVMe `CA` field has reserved encodings; match
/// with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u8)]
pub enum FirmwareAction {
    /// Replace the image in the slot. The image takes effect on the next
    /// controller reset.
    Replace = 0,
    /// Replace the image and mark this slot to be activated on next reset.
    ReplaceAndActivate = 1,
    /// Mark an already-loaded slot as the one to activate on next reset.
    SetActive = 2,
    /// Replace and activate the image *immediately*, with no reset.
    /// Only supported on controllers that advertise that capability in OACS.
    ReplaceAndActivateImmediate = 3,
    /// Replace a boot-partition image.
    ReplaceBootPartition = 6,
    /// Activate a boot-partition image already in a slot.
    ActivateBootPartition = 7,
}

impl FirmwareAction {
    pub(crate) fn as_raw(self) -> nvme_fw_commit_ca {
        self as u8 as nvme_fw_commit_ca
    }
}

/// Which "view" of a feature value the Get Features command should return.
///
/// `#[non_exhaustive]`: the NVMe `SEL` field has reserved encodings; match
/// with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
#[repr(u8)]
pub enum FeatureSelect {
    /// The currently-active value for the feature.
    #[default]
    Current = 0,
    /// The default value the controller ships with.
    Default = 1,
    /// The most recently saved value (if the feature supports save).
    Saved = 2,
    /// The set of supported capabilities for the feature, encoded in the
    /// result dword. See the NVMe spec for the per-feature encoding.
    Supported = 3,
}

impl FeatureSelect {
    pub(crate) fn as_raw(self) -> nvme_get_features_sel {
        self as u8 as nvme_get_features_sel
    }
}

/// Sanitize action — what the Sanitize admin command should do to the
/// drive's user data area.
///
/// `#[non_exhaustive]`: the NVMe `SANACT` field has reserved encodings;
/// match with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u8)]
pub enum SanitizeAction {
    /// Exit a Sanitize Failure state (after a previous sanitize errored).
    ExitFailure = 1,
    /// Block-erase all user data (per spec, fast).
    BlockErase = 2,
    /// Multi-pass overwrite with a configurable pattern. Slow.
    Overwrite = 3,
    /// Cryptographic erase — destroys the media-encryption key.
    /// Effectively instantaneous.
    CryptoErase = 4,
    /// Exit Media Verification (NVMe 2.0+).
    ExitMediaVerification = 5,
}

impl SanitizeAction {
    pub(crate) fn as_raw(self) -> nvme_sanitize_sanact {
        self as u8 as nvme_sanitize_sanact
    }
}

/// Self-test action for the Device Self-Test admin command.
///
/// `#[non_exhaustive]`: the NVMe `STC` field has reserved encodings; match
/// with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u8)]
pub enum SelfTestAction {
    /// Short self-test (typically completes in minutes).
    Short = 1,
    /// Extended self-test (hours).
    Extended = 2,
    /// Host-initiated self-test (NVMe 2.0+).
    HostInitiated = 3,
    /// Vendor-specific self-test.
    VendorSpecific = 14,
    /// Abort a currently-running self-test.
    Abort = 15,
}

impl SelfTestAction {
    pub(crate) fn as_raw(self) -> nvme_dst_stc {
        self as u8 as nvme_dst_stc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lock the spec discriminant values: a typo'd variant value would
    // silently send the wrong command field. as_raw() is `self as u8`,
    // so these double as discriminant assertions.

    #[test]
    fn secure_erase_discriminants() {
        assert_eq!(SecureErase::None.as_raw(), 0);
        assert_eq!(SecureErase::UserData.as_raw(), 1);
        assert_eq!(SecureErase::Cryptographic.as_raw(), 2);
    }

    #[test]
    fn protection_info_discriminants() {
        assert_eq!(ProtectionInfo::Disabled.as_raw(), 0);
        assert_eq!(ProtectionInfo::Type1.as_raw(), 1);
        assert_eq!(ProtectionInfo::Type2.as_raw(), 2);
        assert_eq!(ProtectionInfo::Type3.as_raw(), 3);
    }

    #[test]
    fn firmware_action_discriminants() {
        assert_eq!(FirmwareAction::Replace.as_raw(), 0);
        assert_eq!(FirmwareAction::ReplaceAndActivate.as_raw(), 1);
        assert_eq!(FirmwareAction::SetActive.as_raw(), 2);
        assert_eq!(FirmwareAction::ReplaceAndActivateImmediate.as_raw(), 3);
        assert_eq!(FirmwareAction::ReplaceBootPartition.as_raw(), 6);
        assert_eq!(FirmwareAction::ActivateBootPartition.as_raw(), 7);
    }

    #[test]
    fn feature_select_discriminants() {
        assert_eq!(FeatureSelect::Current.as_raw(), 0);
        assert_eq!(FeatureSelect::Default.as_raw(), 1);
        assert_eq!(FeatureSelect::Saved.as_raw(), 2);
        assert_eq!(FeatureSelect::Supported.as_raw(), 3);
    }

    #[test]
    fn sanitize_action_discriminants() {
        assert_eq!(SanitizeAction::ExitFailure.as_raw(), 1);
        assert_eq!(SanitizeAction::BlockErase.as_raw(), 2);
        assert_eq!(SanitizeAction::Overwrite.as_raw(), 3);
        assert_eq!(SanitizeAction::CryptoErase.as_raw(), 4);
        assert_eq!(SanitizeAction::ExitMediaVerification.as_raw(), 5);
    }

    #[test]
    fn self_test_action_discriminants() {
        assert_eq!(SelfTestAction::Short.as_raw(), 1);
        assert_eq!(SelfTestAction::Extended.as_raw(), 2);
        assert_eq!(SelfTestAction::HostInitiated.as_raw(), 3);
        assert_eq!(SelfTestAction::VendorSpecific.as_raw(), 14);
        assert_eq!(SelfTestAction::Abort.as_raw(), 15);
    }

    #[test]
    fn defaults_are_conservative() {
        assert_eq!(SecureErase::default(), SecureErase::None);
        assert_eq!(ProtectionInfo::default(), ProtectionInfo::Disabled);
        assert_eq!(ProtectionLocation::default(), ProtectionLocation::Last);
        assert_eq!(MetadataSettings::default(), MetadataSettings::Separate);
        assert_eq!(FeatureSelect::default(), FeatureSelect::Current);
    }
}
