//! Deterministic in-process Vault path locking.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use mcp_vault_domain::{PathCaseSensitivity, VaultContext, VaultId, VaultPath};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct LockKey {
    vault: mcp_vault_domain::VaultId,
    path: String,
}

/// Lock manager shared by Core service clones.
#[derive(Clone, Default)]
pub(crate) struct PathLockManager {
    locks: Arc<Mutex<HashMap<LockKey, Weak<AsyncMutex<()>>>>>,
}

/// Vault-scoped serializer for mutations that claim an absent namespace path.
///
/// Path locks retain fine-grained content concurrency. This separate lock
/// closes the check-then-`renameat` window on mounts that cannot perform a
/// native no-replace rename. It is deliberately keyed by Vault identity so
/// unrelated Vaults never block one another.
#[derive(Clone, Default)]
pub(crate) struct NamespaceLockManager {
    locks: Arc<Mutex<HashMap<VaultId, Weak<AsyncMutex<()>>>>>,
}

impl NamespaceLockManager {
    pub(crate) async fn acquire(&self, context: &VaultContext) -> OwnedMutexGuard<()> {
        let mutex = {
            let mut locks = self
                .locks
                .lock()
                .expect("namespace lock map is not poisoned");
            locks.retain(|_, mutex| mutex.strong_count() != 0);
            if let Some(mutex) = locks.get(&context.id()).and_then(Weak::upgrade) {
                mutex
            } else {
                let mutex = Arc::new(AsyncMutex::new(()));
                locks.insert(context.id(), Arc::downgrade(&mutex));
                mutex
            }
        };
        mutex.lock_owned().await
    }
}

impl PathLockManager {
    pub(crate) async fn acquire(
        &self,
        context: &VaultContext,
        paths: &[&VaultPath],
        sensitivity: PathCaseSensitivity,
    ) -> Vec<OwnedMutexGuard<()>> {
        let mut keys: Vec<LockKey> = paths
            .iter()
            .map(|path| LockKey {
                vault: context.id(),
                path: path.comparison_key(sensitivity).as_str().to_owned(),
            })
            .collect();
        keys.sort();
        keys.dedup();

        let mutexes = {
            let mut locks = self.locks.lock().expect("path lock map is not poisoned");
            locks.retain(|_, mutex| mutex.strong_count() != 0);
            keys.into_iter()
                .map(|key| {
                    if let Some(mutex) = locks.get(&key).and_then(Weak::upgrade) {
                        mutex
                    } else {
                        let mutex = Arc::new(AsyncMutex::new(()));
                        locks.insert(key, Arc::downgrade(&mutex));
                        mutex
                    }
                })
                .collect::<Vec<_>>()
        };

        let mut guards = Vec::with_capacity(mutexes.len());
        for mutex in mutexes {
            guards.push(mutex.lock_owned().await);
        }
        guards
    }
}
