use std::marker::PhantomData;

#[cfg(has_subsystem_application)]
use libnvme_sys::nvme_subsystem_get_application;
#[cfg(has_subsystem_fw_rev)]
use libnvme_sys::nvme_subsystem_get_fw_rev;
#[cfg(has_subsystem_iopolicy)]
use libnvme_sys::nvme_subsystem_get_iopolicy;
#[cfg(has_subsystem_model)]
use libnvme_sys::nvme_subsystem_get_model;
#[cfg(has_subsystem_serial)]
use libnvme_sys::nvme_subsystem_get_serial;
use libnvme_sys::{
    nvme_first_subsystem, nvme_host_t, nvme_next_subsystem, nvme_subsystem_get_name,
    nvme_subsystem_get_nqn, nvme_subsystem_get_type, nvme_subsystem_t,
};

use crate::controller::Controllers;
use crate::util::cstr_to_str;
use crate::{Result, Root};

/// An NVMe subsystem.
///
/// A subsystem groups one or more controllers that share a common identity
/// (NQN, serial, model). For typical single-controller PCIe SSDs the subsystem
/// has exactly one controller; Fabrics and multipath setups can have several.
pub struct Subsystem<'r> {
    inner: nvme_subsystem_t,
    _marker: PhantomData<&'r Root>,
    _not_send_sync: PhantomData<*const ()>,
}

impl<'r> Subsystem<'r> {
    pub(crate) fn from_raw(inner: nvme_subsystem_t) -> Self {
        Subsystem {
            inner,
            _marker: PhantomData,
            _not_send_sync: PhantomData,
        }
    }

    /// The kernel-assigned subsystem name, e.g. `nvme-subsys0`.
    pub fn name(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_subsystem_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_subsystem_get_name(self.inner)) }
    }

    /// The subsystem NVMe Qualified Name (NQN).
    pub fn nqn(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_subsystem_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_subsystem_get_nqn(self.inner)) }
    }

    /// The subsystem type (e.g. `nvm`, `discovery`).
    pub fn subsystem_type(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_subsystem_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_subsystem_get_type(self.inner)) }
    }

    /// The subsystem-level serial number, when reported.
    ///
    /// Only present when built against a libnvme that exposes
    /// `nvme_subsystem_get_serial`. Older releases (notably the libnvme 1.8
    /// shipped in Ubuntu 24.04) do not have this symbol; on those builds the
    /// method is compiled out.
    #[cfg(has_subsystem_serial)]
    pub fn serial(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_subsystem_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_subsystem_get_serial(self.inner)) }
    }

    /// The subsystem-level model string, when reported.
    ///
    /// Only present when built against a libnvme that exposes
    /// `nvme_subsystem_get_model`.
    #[cfg(has_subsystem_model)]
    pub fn model(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_subsystem_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_subsystem_get_model(self.inner)) }
    }

    /// The subsystem-level firmware revision, when reported.
    ///
    /// Only present when built against a libnvme that exposes
    /// `nvme_subsystem_get_fw_rev`.
    #[cfg(has_subsystem_fw_rev)]
    pub fn firmware(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_subsystem_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_subsystem_get_fw_rev(self.inner)) }
    }

    /// ANA / multipath I/O policy for this subsystem, e.g. `numa`,
    /// `round-robin`.
    ///
    /// Only present when built against a libnvme that exposes
    /// `nvme_subsystem_get_iopolicy`.
    #[cfg(has_subsystem_iopolicy)]
    pub fn iopolicy(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_subsystem_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_subsystem_get_iopolicy(self.inner)) }
    }

    /// Application-set tag for this subsystem (used by tools like `nvme-cli`
    /// for grouping). Empty when unset.
    ///
    /// Only present when built against a libnvme that exposes
    /// `nvme_subsystem_get_application`.
    #[cfg(has_subsystem_application)]
    pub fn application(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_subsystem_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_subsystem_get_application(self.inner)) }
    }

    /// Iterate over the controllers in this subsystem.
    pub fn controllers(&self) -> Controllers<'r> {
        Controllers::new(self.inner)
    }
}

impl std::fmt::Debug for Subsystem<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subsystem")
            .field("name", &self.name().ok())
            .field("nqn", &self.nqn().ok())
            .finish()
    }
}

/// Iterator over [`Subsystem`] entries, returned by [`crate::Host::subsystems`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Subsystems<'r> {
    host: nvme_host_t,
    cursor: nvme_subsystem_t,
    _marker: PhantomData<&'r Root>,
}

impl<'r> Subsystems<'r> {
    pub(crate) fn new(host: nvme_host_t) -> Self {
        // SAFETY: host is a valid non-null nvme_host_t from the same libnvme
        // tree, tied to 'r. libnvme's iterator helpers tolerate any valid
        // parent handle and return NULL when there are no children.
        let cursor = unsafe { nvme_first_subsystem(host) };
        Subsystems {
            host,
            cursor,
            _marker: PhantomData,
        }
    }
}

impl<'r> Iterator for Subsystems<'r> {
    type Item = Subsystem<'r>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_null() {
            return None;
        }
        let current = self.cursor;
        // SAFETY: self.host and current are valid non-null handles from the same
        // libnvme tree, tied to 'r; libnvme returns NULL at end-of-list.
        self.cursor = unsafe { nvme_next_subsystem(self.host, current) };
        Some(Subsystem::from_raw(current))
    }
}
