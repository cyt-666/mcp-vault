//! Deterministic in-process Vault path locking.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use mcp_vault_domain::{PathCaseSensitivity, VaultContext, VaultPath};
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
