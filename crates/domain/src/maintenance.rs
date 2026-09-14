//! Process-wide maintenance coordination state.
//!
//! This gate is deliberately separate from `VaultContext`: it coordinates
//! backup/restore and shutdown across protocol adapters, while Vault-owned
//! authorization and storage operations still require their normal context.

use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
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
    maintenance_active: AtomicBool,
    transition: Mutex<()>,
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

/// Exclusive transition into Offline mode. The lease stops new admissions;
/// callers must drain the counters before touching canonical state. A caller
/// must explicitly restore the prior mode after a safe completion; dropping
/// an unreleased lease keeps Offline for explicit recovery.
#[derive(Debug)]
#[must_use = "explicitly restore or retain Offline maintenance ownership"]
pub struct MaintenanceLease {
    state: Arc<MaintenanceState>,
    previous: MaintenanceMode,
    released: bool,
    token: MaintenancePermitToken,
}

/// Opaque, read-only capability checked by the State initialization boundary.
/// Callers cannot reactivate or forge a released lease.
#[derive(Clone, Debug)]
pub struct MaintenancePermitToken {
    active: Arc<AtomicBool>,
    state: Weak<MaintenanceState>,
}

impl MaintenancePermitToken {
    pub fn belongs_to(&self, gate: &MaintenanceGate) -> bool {
        self.state.ptr_eq(&Arc::downgrade(&gate.state))
    }

    pub fn is_valid(&self) -> bool {
        self.active.load(Ordering::Acquire)
            && self.state.upgrade().is_some_and(|state| {
                state.mode.load(Ordering::Acquire) == MaintenanceMode::Offline as u8
                    && state.active_operations.load(Ordering::Acquire) == 0
                    && state.active_writes.load(Ordering::Acquire) == 0
            })
    }
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
                maintenance_active: AtomicBool::new(false),
                transition: Mutex::new(()),
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
        let _transition = self
            .state
            .transition
            .lock()
            .expect("maintenance transition lock poisoned");
        if self.state.maintenance_active.load(Ordering::Acquire) && mode != MaintenanceMode::Offline
        {
            return;
        }
        self.state.mode.store(mode as u8, Ordering::Release);
        if mode != MaintenanceMode::Offline {
            self.state
                .maintenance_active
                .store(false, Ordering::Release);
        }
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

    /// Atomically stop new admissions and return an exclusive Offline lease.
    /// Existing operations are still counted and must be drained by the
    /// caller with a bounded timeout.
    pub fn try_begin_offline(&self) -> Option<MaintenanceLease> {
        self.try_begin_mode(MaintenanceMode::Offline)
    }

    /// Atomically own ReadOnly mode for backup creation while allowing
    /// admitted reads to continue; canonical writes remain blocked.
    pub fn try_begin_read_only(&self) -> Option<MaintenanceLease> {
        self.try_begin_mode(MaintenanceMode::ReadOnly)
    }

    fn try_begin_mode(&self, target: MaintenanceMode) -> Option<MaintenanceLease> {
        let _transition = self.state.transition.lock().ok()?;
        if self
            .state
            .maintenance_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        let previous = self.mode();
        if previous == MaintenanceMode::Offline && target != MaintenanceMode::Offline {
            self.state
                .maintenance_active
                .store(false, Ordering::Release);
            return None;
        }
        if previous != target
            && self
                .state
                .mode
                .compare_exchange(
                    previous as u8,
                    target as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
        {
            self.state
                .maintenance_active
                .store(false, Ordering::Release);
            return None;
        }
        Some(MaintenanceLease {
            state: self.state.clone(),
            previous,
            released: false,
            token: MaintenancePermitToken {
                active: Arc::new(AtomicBool::new(true)),
                state: Arc::downgrade(&self.state),
            },
        })
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

impl MaintenanceLease {
    /// Mode that was active before this lease.
    pub const fn previous_mode(&self) -> MaintenanceMode {
        self.previous
    }

    /// Return whether all pre-existing operations have drained.
    pub fn is_drained(&self) -> bool {
        self.state.active_operations.load(Ordering::Acquire) == 0
            && self.state.active_writes.load(Ordering::Acquire) == 0
    }

    /// Restore the prior mode and release maintenance ownership.
    pub fn restore(mut self) {
        self.restore_to(self.previous);
    }

    /// Restore an explicit mode and release maintenance ownership. This is
    /// used when an explicitly resumed task completes after an earlier
    /// failed attempt left the process Offline.
    pub fn restore_to(&mut self, mode: MaintenanceMode) {
        let _transition = self
            .state
            .transition
            .lock()
            .expect("maintenance transition lock poisoned");
        self.state.mode.store(mode as u8, Ordering::Release);
        self.state
            .maintenance_active
            .store(false, Ordering::Release);
        self.token.active.store(false, Ordering::Release);
        self.released = true;
    }

    /// Internal capability token checked by the State initialization boundary.
    pub fn permit_token(&self) -> MaintenancePermitToken {
        self.token.clone()
    }
}

impl Drop for MaintenanceLease {
    fn drop(&mut self) {
        if !self.released {
            // A mutation failure remains Offline for explicit recovery. A
            // drain/setup failure must call restore() before dropping.
            let _transition = self
                .state
                .transition
                .lock()
                .expect("maintenance transition lock poisoned");
            self.state
                .maintenance_active
                .store(false, Ordering::Release);
            self.token.active.store(false, Ordering::Release);
        }
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

    #[test]
    fn offline_lease_owns_gate_and_invalidates_initialization_token() {
        let gate = MaintenanceGate::new();
        let lease = gate.try_begin_offline().unwrap();
        assert_eq!(gate.mode(), MaintenanceMode::Offline);
        assert!(gate.try_begin_offline().is_none());
        assert!(lease.is_drained());
        let token = lease.permit_token();
        assert!(token.is_valid());
        let foreign = MaintenanceGate::new();
        let foreign_lease = foreign.try_begin_offline().unwrap();
        assert!(!foreign_lease.permit_token().belongs_to(&gate));
        foreign_lease.restore();
        let busy_gate = MaintenanceGate::new();
        let active = busy_gate.try_start_operation().unwrap();
        let busy_lease = busy_gate.try_begin_offline().unwrap();
        assert!(!busy_lease.permit_token().is_valid());
        drop(active);
        busy_lease.restore();
        gate.set(MaintenanceMode::Normal);
        assert_eq!(gate.mode(), MaintenanceMode::Offline);
        lease.restore();
        assert_eq!(gate.mode(), MaintenanceMode::Normal);
        assert!(!token.is_valid());
    }
}
