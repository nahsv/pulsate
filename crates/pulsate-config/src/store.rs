//! The live configuration store: lock-free publication, hot reload, rollback.
//!
//! The control plane owns a [`ConfigStore`]; the data plane reads the current
//! [`ConfigSnapshot`] through it with a single atomic load and never blocks
//! (`docs/02-architecture.md#hot-reload-architecture`). A reload validates and
//! builds a new snapshot off to the side; only on success does it atomically
//! swap the pointer, keeping the previous generation for one-step rollback.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use pulsate_core::ConfigSnapshot;
use pulsate_flow::Diagnostic;

use crate::compile::{compile, Compiled};
use crate::model::Config;

/// One published generation: the typed config and its compiled snapshot, held
/// together so a reader sees a consistent pair.
#[derive(Debug)]
pub struct Generation {
    /// The typed config for this generation.
    pub config: Config,
    /// The immutable snapshot the data plane runs against.
    pub snapshot: ConfigSnapshot,
}

/// A lock-free, reloadable holder of the live configuration.
#[derive(Debug)]
pub struct ConfigStore {
    current: ArcSwap<Generation>,
    /// The immediately-previous generation, kept for one-step auto-rollback.
    previous: ArcSwap<Option<Arc<Generation>>>,
    next_generation: AtomicU64,
}

impl ConfigStore {
    /// Build a store by compiling the initial config text.
    ///
    /// # Errors
    /// Returns the compilation diagnostics if the initial config is invalid.
    pub fn load(name: &str, text: &str) -> Result<Self, Vec<Diagnostic>> {
        let compiled = compile(name, text, 0)?;
        Ok(Self {
            current: ArcSwap::from_pointee(generation_of(compiled)),
            previous: ArcSwap::from_pointee(None),
            next_generation: AtomicU64::new(1),
        })
    }

    /// Load the current generation. This is the data plane's hot-path read: a
    /// single atomic load returning an owned `Arc` for the request's duration.
    #[must_use]
    pub fn current(&self) -> Arc<Generation> {
        self.current.load_full()
    }

    /// The current snapshot's generation number.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.current().snapshot.generation()
    }

    /// Validate and publish a new config text, swapping atomically on success.
    ///
    /// On failure the running snapshot is untouched and the diagnostics are
    /// returned — the *validation is total before publish* guarantee.
    ///
    /// # Errors
    /// Returns the compilation diagnostics if the new config is invalid.
    pub fn reload(&self, name: &str, text: &str) -> Result<Arc<Generation>, Vec<Diagnostic>> {
        let gen_num = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let compiled = compile(name, text, gen_num)?;
        let new_gen = Arc::new(generation_of(compiled));

        // Demote the current generation to "previous" before swapping in the new
        // one, so a single rollback can restore it.
        let outgoing = self.current.load_full();
        self.previous.store(Arc::new(Some(outgoing)));
        self.current.store(Arc::clone(&new_gen));
        Ok(new_gen)
    }

    /// Roll back to the previous generation, if one exists. Returns the restored
    /// generation, or `None` if there is nothing to roll back to.
    pub fn rollback(&self) -> Option<Arc<Generation>> {
        let prev = self.previous.load_full();
        let restored = (*prev).clone()?;
        // The current becomes the new "previous" so rollback is reversible once.
        let outgoing = self.current.load_full();
        self.current.store(Arc::clone(&restored));
        self.previous.store(Arc::new(Some(outgoing)));
        Some(restored)
    }
}

fn generation_of(compiled: Compiled) -> Generation {
    Generation {
        config: compiled.config,
        snapshot: compiled.snapshot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "site a.com { route /* ~> respond(status=200) }";
    const B: &str =
        "upstream api { target http://127.0.0.1:8080 }\nsite b.com { route /* ~> proxy(@api) }";

    #[test]
    fn load_then_reload_swaps_snapshot() {
        let store = ConfigStore::load("t.flow", A).unwrap();
        assert_eq!(store.generation(), 0);
        let id_a = store.current().snapshot.id();

        store.reload("t.flow", B).unwrap();
        assert_eq!(store.generation(), 1);
        assert_ne!(store.current().snapshot.id(), id_a);
        assert_eq!(store.current().config.sites[0].hosts[0].pattern, "b.com");
    }

    #[test]
    fn failed_reload_leaves_running_snapshot_untouched() {
        let store = ConfigStore::load("t.flow", A).unwrap();
        let before = store.current().snapshot.id();

        // References an undefined upstream → must fail and not publish.
        let err = store
            .reload("t.flow", "site x.com { route /* ~> proxy(@missing) }")
            .unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(store.current().snapshot.id(), before);
        assert_eq!(store.generation(), 0);
    }

    #[test]
    fn rollback_restores_previous_generation() {
        let store = ConfigStore::load("t.flow", A).unwrap();
        let id_a = store.current().snapshot.id();
        store.reload("t.flow", B).unwrap();

        let restored = store.rollback().expect("has a previous");
        assert_eq!(restored.snapshot.id(), id_a);
        assert_eq!(store.current().snapshot.id(), id_a);
    }

    #[test]
    fn rollback_without_history_is_none() {
        let store = ConfigStore::load("t.flow", A).unwrap();
        assert!(store.rollback().is_none());
    }
}
