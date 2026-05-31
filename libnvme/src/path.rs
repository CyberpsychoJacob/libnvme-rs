//! Multipath / ANA paths.
//!
//! In multipath configurations (ANA, NVMe-oF), a single namespace can be
//! reachable through multiple controller "paths". This module exposes
//! libnvme's path handles via [`Path`] and the [`Paths`] iterator, accessible
//! from both [`Controller::paths`](crate::Controller::paths) and
//! [`Namespace::paths`](crate::Namespace::paths).

use std::marker::PhantomData;

#[cfg(has_path_numa_nodes)]
use libnvme_sys::nvme_path_get_numa_nodes;
#[cfg(has_path_queue_depth)]
use libnvme_sys::nvme_path_get_queue_depth;
use libnvme_sys::{
    nvme_ctrl_first_path, nvme_ctrl_next_path, nvme_ctrl_t, nvme_namespace_first_path,
    nvme_namespace_next_path, nvme_ns_t, nvme_path_get_ana_state, nvme_path_get_name, nvme_path_t,
};

use crate::util::cstr_to_str;
use crate::{Result, Root};

/// A single multipath route to a namespace.
pub struct Path<'r> {
    inner: nvme_path_t,
    _marker: PhantomData<&'r Root>,
    _not_send_sync: PhantomData<*const ()>,
}

impl<'r> Path<'r> {
    pub(crate) fn from_raw(inner: nvme_path_t) -> Self {
        Path {
            inner,
            _marker: PhantomData,
            _not_send_sync: PhantomData,
        }
    }

    /// Kernel-assigned path name.
    pub fn name(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_path_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_path_get_name(self.inner)) }
    }

    /// Asymmetric Namespace Access (ANA) state for this path: `optimized`,
    /// `non-optimized`, `inaccessible`, `persistent-loss`, or `change`.
    pub fn ana_state(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_path_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_path_get_ana_state(self.inner)) }
    }

    /// Comma-separated list of NUMA nodes reachable through this path.
    ///
    /// Only present when built against a libnvme that exposes
    /// `nvme_path_get_numa_nodes` (added after libnvme 1.8 / Ubuntu 24.04).
    #[cfg(has_path_numa_nodes)]
    pub fn numa_nodes(&self) -> Result<&'r str> {
        // SAFETY: self.inner is a non-null nvme_path_t tied to the Root tree via 'r.
        // libnvme returns either NULL or a valid NUL-terminated C string owned by
        // the tree, valid for 'r. cstr_to_str checks for NULL.
        unsafe { cstr_to_str(nvme_path_get_numa_nodes(self.inner)) }
    }

    /// Current queue depth on this path.
    ///
    /// Only present when built against a libnvme that exposes
    /// `nvme_path_get_queue_depth` (added after libnvme 1.8 / Ubuntu 24.04).
    #[cfg(has_path_queue_depth)]
    pub fn queue_depth(&self) -> i32 {
        // SAFETY: self.inner is a non-null nvme_path_t tied to the Root tree via 'r.
        unsafe { nvme_path_get_queue_depth(self.inner) }
    }
}

impl std::fmt::Debug for Path<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("Path");
        debug
            .field("name", &self.name().ok())
            .field("ana_state", &self.ana_state().ok());
        #[cfg(has_path_queue_depth)]
        debug.field("queue_depth", &self.queue_depth());
        debug.finish()
    }
}

enum PathParent {
    Controller(nvme_ctrl_t),
    Namespace(nvme_ns_t),
}

/// Iterator over [`Path`] entries reachable through a
/// [`Controller`](crate::Controller) or [`Namespace`](crate::Namespace).
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Paths<'r> {
    parent: PathParent,
    cursor: nvme_path_t,
    _marker: PhantomData<&'r Root>,
}

impl<'r> Paths<'r> {
    pub(crate) fn from_controller(ctrl: nvme_ctrl_t) -> Self {
        // SAFETY: ctrl is a valid non-null nvme_ctrl_t from the libnvme tree,
        // tied to 'r; iterator helpers return NULL when there are no paths.
        let cursor = unsafe { nvme_ctrl_first_path(ctrl) };
        Paths {
            parent: PathParent::Controller(ctrl),
            cursor,
            _marker: PhantomData,
        }
    }

    pub(crate) fn from_namespace(ns: nvme_ns_t) -> Self {
        // SAFETY: ns is a valid non-null nvme_ns_t from the libnvme tree,
        // tied to 'r; iterator helpers return NULL when there are no paths.
        let cursor = unsafe { nvme_namespace_first_path(ns) };
        Paths {
            parent: PathParent::Namespace(ns),
            cursor,
            _marker: PhantomData,
        }
    }
}

impl<'r> Iterator for Paths<'r> {
    type Item = Path<'r>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_null() {
            return None;
        }
        let current = self.cursor;
        self.cursor = match self.parent {
            // SAFETY: c and current are valid non-null handles from the same
            // libnvme tree, tied to 'r; libnvme returns NULL at end-of-list.
            PathParent::Controller(c) => unsafe { nvme_ctrl_next_path(c, current) },
            // SAFETY: n and current are valid non-null handles from the same
            // libnvme tree, tied to 'r; libnvme returns NULL at end-of-list.
            PathParent::Namespace(n) => unsafe { nvme_namespace_next_path(n, current) },
        };
        Some(Path::from_raw(current))
    }
}
