//! Process-wide maintenance coordination state.
//!
//! This gate is deliberately separate from `VaultContext`: it coordinates
//! backup/restore and shutdown across protocol adapters, while Vault-owned
//! authorization and storage operations still require their normal context.

use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};

use serde::{Deserialize, Serialize};

/// Process maintenance mode exposed to protocol adapters and diagnostics.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum MaintenanceMode {
    /// Normal reads and writes are allowed.
    Normal = 0,
    /// Reads/search/recall remain available; canonical writes are rejected.
    ReadOnly = 1,
    /// Data-plane operations are temporarily unavailable while recovery runs.
    Offline = 2,
}

impl MaintenanceMode {
    /// Return the stable wire/configuration label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::ReadOnly => "read_only",
            Self::Offline => "offline",
        }
    }

    /// Whether a read-only operation may proceed in this mode.
    pub const fn allows_read(self) -> bool {
        !matches!(self, Self::Offline)
    }

    /// Whether a canonical or operational mutation may proceed.
    pub const fn allows_write(self) -> bool {
        matches!(self, Self::Normal)
    }
}

impl std::fmt::Display for MaintenanceMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A cloneable process gate used by the service composition root.
#[derive(Clone, Debug)]
pub struct MaintenanceGate {
    state: Arc<MaintenanceState>,
}

#[derive(Debug)]
struct MaintenanceState {
    mode: AtomicU8,
    active_operations: AtomicUsize,
    active_writes: AtomicUsize,
}

/// RAII admission held for the full lifetime of one process operation.
///
/// A write admission also counts as an active operation. Maintenance changes
/// the mode first and then waits for these counters to reach zero, while the
/// post-increment mode check closes the admission race.
#[derive(Debug)]
#[must_use = "dropping the maintenance admission ends operation tracking"]
pub struct MaintenanceOperationGuard {
    state: Arc<MaintenanceState>,
    write: bool,
}

impl Default for MaintenanceGate {
    fn default() -> Self {
        Self::new()
    }
}

impl MaintenanceGate {
    /// Create a gate in normal operation mode.
    pub fn new() -> Self {
        Self {
            state: Arc::new(MaintenanceState {
                mode: AtomicU8::new(MaintenanceMode::Normal as u8),
                active_operations: AtomicUsize::new(0),
                active_writes: AtomicUsize::new(0),
            }),
        }
    }

    /// Read the current mode.
    pub fn mode(&self) -> MaintenanceMode {
        match self.state.mode.load(Ordering::Acquire) {
            value if value == MaintenanceMode::ReadOnly as u8 => MaintenanceMode::ReadOnly,
            value if value == MaintenanceMode::Offline as u8 => MaintenanceMode::Offline,
            _ => MaintenanceMode::Normal,
        }
    }

    /// Set the process mode.
    pub fn set(&self, mode: MaintenanceMode) {
        self.state.mode.store(mode as u8, Ordering::Release);
    }

    /// Return whether a data-plane request may start in the current mode.
    pub fn allows_read(&self) -> bool {
        self.mode().allows_read()
    }

    /// Return whether a mutating request may start in the current mode.
    pub fn allows_write(&self) -> bool {
        self.mode().allows_write()
    }

    /// Admit one read/request operation unless the process is offline.
    pub fn try_start_operation(&self) -> Option<MaintenanceOperationGuard> {
        self.try_start(false)
    }

    /// Admit one mutating operation only while the process is normal.
    pub fn try_start_write(&self) -> Option<MaintenanceOperationGuard> {
        self.try_start(true)
    }

    /// Return the number of admitted operations that have not completed.
    pub fn active_operations(&self) -> usize {
        self.state.active_operations.load(Ordering::Acquire)
    }

    /// Return the number of admitted mutations that have not completed.
    pub fn active_writes(&self) -> usize {
        self.state.active_writes.load(Ordering::Acquire)
    }

    /// Return whether two handles coordinate the same process state.
    pub fn is_same_gate(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    fn try_start(&self, write: bool) -> Option<MaintenanceOperationGuard> {
        let allowed = |mode: MaintenanceMode| {
            if write {
                mode.allows_write()
            } else {
                mode.allows_read()
            }
        };
        if !allowed(self.mode()) {
            return None;
        }
        self.state.active_operations.fetch_add(1, Ordering::AcqRel);
        if write {
            self.state.active_writes.fetch_add(1, Ordering::AcqRel);
        }
        if allowed(self.mode()) {
            return Some(MaintenanceOperationGuard {
                state: self.state.clone(),
                write,
            });
        }
        if write {
            self.state.active_writes.fetch_sub(1, Ordering::AcqRel);
        }
        self.state.active_operations.fetch_sub(1, Ordering::AcqRel);
        None
    }
}

impl Drop for MaintenanceOperationGuard {
    fn drop(&mut self) {
        if self.write {
            self.state.active_writes.fetch_sub(1, Ordering::AcqRel);
        }
        self.state.active_operations.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::{MaintenanceGate, MaintenanceMode};

    #[test]
    fn modes_have_explicit_read_and_write_boundaries() {
        let gate = MaintenanceGate::new();
        assert_eq!(gate.mode(), MaintenanceMode::Normal);
        assert!(gate.allows_read());
        assert!(gate.allows_write());

        gate.set(MaintenanceMode::ReadOnly);
        assert!(gate.allows_read());
        assert!(!gate.allows_write());

        gate.set(MaintenanceMode::Offline);
        assert!(!gate.allows_read());
        assert!(!gate.allows_write());
    }

    #[test]
    fn admissions_track_active_work_and_close_after_mode_change() {
        let gate = MaintenanceGate::new();
        let request = gate.try_start_operation().unwrap();
        let write = gate.try_start_write().unwrap();
        assert_eq!(gate.active_operations(), 2);
        assert_eq!(gate.active_writes(), 1);

        gate.set(MaintenanceMode::ReadOnly);
        assert!(gate.try_start_operation().is_some());
        assert!(gate.try_start_write().is_none());
        drop(write);
        assert_eq!(gate.active_writes(), 0);
        drop(request);
    }
}
