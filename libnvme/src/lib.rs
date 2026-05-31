//! Safe, idiomatic Rust bindings for the Linux `libnvme` C library.
//!
//! `libnvme` is the userspace library that backs `nvme-cli`. This crate exposes
//! a memory-safe wrapper over its handle tree:
//!
//! ```text
//! Root → Host → Subsystem → Controller → Namespace
//! ```
//!
//! Every handle borrows from the [`Root`] via the `'r` lifetime, so dropping
//! the [`Root`] cascades-frees the entire tree.
//!
//! # Example
//!
//! ```no_run
//! use libnvme::Root;
//!
//! let root = Root::scan()?;
//! for host in root.hosts() {
//!     for subsys in host.subsystems() {
//!         for ctrl in subsys.controllers() {
//!             println!("{} {}", ctrl.name()?, ctrl.model()?);
//!             let id = ctrl.identify()?;
//!             println!("  NVMe spec: {}", id.nvme_version());
//!             for ns in ctrl.namespaces() {
//!                 println!("  {} ({} bytes)", ns.name()?, ns.size_bytes());
//!             }
//!         }
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod admin;
mod commands;
mod controller;
mod directives;
mod error;
mod fabrics;
mod features;
mod host;
mod identify;
mod io;
mod log;
mod namespace;
mod path;
mod reservations;
mod root;
mod subsystem;
mod util;
mod zns;

pub use admin::{
    FeatureSelect, FirmwareAction, MetadataSettings, ProtectionInfo, ProtectionLocation,
    SanitizeAction, SecureErase, SelfTestAction,
};
pub use commands::{GetLbaStatusArgs, LockdownArgs, PassthruArgs, Sanitize};
pub use controller::{Controller, Controllers};
pub use directives::{
    DirectiveRecv, DirectiveRecvOp, DirectiveSend, DirectiveSendOp, DirectiveType,
};
pub use error::{Error, Result};
pub use fabrics::{Connect, DiscoveryLog, DiscoveryLogEntry, Transport};
pub use features::{
    AutoPst, Features, HostBehavior, HostMemBufAttrs, LbaRangeType, PlmConfig, Timestamp,
};
pub use host::{Host, Hosts};
pub use identify::{IdentifyController, IdentifyNamespace, LbaFormat, NvmeVersion};
pub use io::{
    Compare, Copy, CopyRange, Dsm, DsmAttr, DsmRange, Read, Verify, Write, WriteUncorrectable,
    WriteZeroes,
};
pub use log::{ErrorLogEntry, FirmwareSlotLog, SmartLog};
pub use namespace::{Format, Namespace, Namespaces};
pub use path::{Path, Paths};
pub use reservations::{
    nvme_resv_status, PtplChange, ReservationAcquire, ReservationAcquireAction,
    ReservationRegister, ReservationRegisterAction, ReservationRelease, ReservationReleaseAction,
    ReservationReport, ReservationType,
};
#[cfg(has_hostid_generate)]
pub use root::generate_hostid;
#[cfg(has_hostid_from_file)]
pub use root::hostid_from_file;
pub use root::{generate_hostnqn, hostnqn_from_file, Root};
pub use subsystem::{Subsystem, Subsystems};
pub use zns::{
    ZnsAppend, ZnsMgmtRecv, ZnsMgmtSend, ZoneRecvAction, ZoneReportFilter, ZoneSendAction,
};
