use std::marker::PhantomData;

use libnvme_sys::{
    nvme_first_host, nvme_host_get_hostid, nvme_host_get_hostnqn, nvme_host_t, nvme_next_host,
    nvme_root_t,
};

use crate::fabrics::{Connect, Transport};
use crate::subsystem::Subsystems;
use crate::util::cstr_to_str;
use crate::{Result, Root};

/// A host entry in the libnvme tree.
///
/// In the libnvme model, a "host" represents a local or fabrics identity
/// (NVMe Qualified Name and ID) under which subsystems are attached. Most
/// systems have exactly one. Borrows from the parent [`Root`] via `'r`.
pub struct Host<'r> {
    inner: nvme_host_t,
    root: nvme_root_t,
    _marker: PhantomData<&'r Root>,
    _not_send_sync: PhantomData<*const ()>,
}

impl<'r> Host<'r> {
    pub(crate) fn from_raw(inner: nvme_host_t, root: nvme_root_t) -> Self {
        Host {
            inner,
            root,
            _marker: PhantomData,
            _not_send_sync: PhantomData,
        }
    }

    pub(crate) fn as_ptr(&self) -> nvme_host_t {
        self.inner
    }

    pub(crate) fn root_ptr(&self) -> nvme_root_t {
        self.root
    }

    /// The Host NVMe Qualified Name (HostNQN), e.g. `nqn.2014-08.org.nvmexpress:uuid:...`.
    pub fn hostnqn(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_host_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_host_get_hostnqn(self.inner)) }
    }

    /// The host identifier (HostID) as a UUID-formatted string.
    pub fn hostid(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_host_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_host_get_hostid(self.inner)) }
    }

    /// Iterate over the subsystems attached to this host.
    pub fn subsystems(&self) -> Subsystems<'r> {
        Subsystems::new(self.inner)
    }

    /// Build a fabrics Connect operation for this host.
    ///
    /// `transport` and `subsysnqn` are required; everything else (traddr,
    /// trsvcid, host_traddr, queue sizes, digests, TLS, etc.) is set via
    /// chained methods on the returned [`Connect`].
    ///
    /// ```no_run
    /// # use libnvme::{Root, Transport};
    /// let root = Root::scan()?;
    /// let host = root.default_host()?;
    /// let ctrl = host
    ///     .connect(Transport::Tcp, "nqn.2014-08.org.nvmexpress:target")
    ///     .traddr("10.0.0.1")
    ///     .trsvcid("4420")
    ///     .keep_alive_tmo(120)
    ///     .execute()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn connect<'a>(&'a self, transport: Transport<'_>, subsysnqn: &str) -> Connect<'a, 'r> {
        Connect::new(self, transport, subsysnqn)
    }
}

impl std::fmt::Debug for Host<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("hostnqn", &self.hostnqn().ok())
            .finish()
    }
}

/// Iterator over [`Host`] entries, returned by [`Root::hosts`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Hosts<'r> {
    root: nvme_root_t,
    cursor: nvme_host_t,
    _marker: PhantomData<&'r Root>,
}

impl<'r> Hosts<'r> {
    pub(crate) fn new(root: nvme_root_t) -> Self {
        // SAFETY: root is a non-null nvme_root_t obtained from libnvme's scan,
        // kept alive by the borrow of Root via 'r.
        let cursor = unsafe { nvme_first_host(root) };
        Hosts {
            root,
            cursor,
            _marker: PhantomData,
        }
    }
}

impl<'r> Iterator for Hosts<'r> {
    type Item = Host<'r>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_null() {
            return None;
        }
        let current = self.cursor;
        // SAFETY: self.root and current are valid non-null handles from the same
        // libnvme tree, tied to 'r; libnvme's iterator helpers tolerate any
        // valid parent handle and return NULL at end-of-list.
        self.cursor = unsafe { nvme_next_host(self.root, current) };
        Some(Host::from_raw(current, self.root))
    }
}
