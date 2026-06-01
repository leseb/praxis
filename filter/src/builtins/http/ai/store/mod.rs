// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Response store persistence layer for the Responses API.
//!
//! Provides the [`ResponseStore`] trait, [`ResponseStoreRegistry`],
//! and a [`SqliteResponseStore`] backend. Used by Responses API
//! filters for persisting response records, conversation history,
//! and input items.
//!
//! # Registry pattern
//!
//! Stores are registered by name during server initialization.
//! Filters access them through the registry:
//!
//! ```ignore
//! let store = registry.get("default").expect("store not registered");
//! store.upsert_response(&record).await?;
//! ```

mod schemas;
mod sqlite;
mod store;
mod types;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "tests"
)]
mod tests;

use std::sync::Arc;

use dashmap::{DashMap, mapref::entry::Entry};

#[allow(unused_imports, reason = "re-exports for upcoming response store filter and tests")]
pub use self::types::{ConversationRecord, ListParams, Order, ResponsePage, ResponseRecord, StoreError};
pub use self::{sqlite::SqliteResponseStore, store::ResponseStore};

// -----------------------------------------------------------------------------
// ResponseStoreRegistry
// -----------------------------------------------------------------------------

/// Concurrent registry of named response store backends.
///
/// Stores are not auto-created on access — they must be explicitly
/// registered during server initialization with their database
/// configuration.
///
/// ```ignore
/// use std::sync::Arc;
///
/// let registry = ResponseStoreRegistry::new();
/// let store = SqliteResponseStore::new("sqlite::memory:").await?;
/// registry.register("default", Arc::new(store))?;
/// ```
pub struct ResponseStoreRegistry {
    /// Named store backends.
    stores: Arc<DashMap<Arc<str>, Arc<dyn ResponseStore>>>,
}

impl ResponseStoreRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stores: Arc::new(DashMap::new()),
        }
    }

    /// Register a named store instance.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Unavailable`] if a store with the same
    /// name is already registered.
    pub fn register(&self, name: &str, store: Arc<dyn ResponseStore>) -> Result<(), StoreError> {
        match self.stores.entry(Arc::from(name)) {
            Entry::Occupied(_) => Err(StoreError::Unavailable(format!("store already registered: {name}"))),
            Entry::Vacant(entry) => {
                entry.insert(store);
                tracing::info!(store = name, "response store registered");
                Ok(())
            },
        }
    }

    /// Get a store by name.
    ///
    /// Returns `None` if no store with `name` is registered.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ResponseStore>> {
        self.stores.get(name).map(|r| Arc::clone(r.value()))
    }
}

impl Default for ResponseStoreRegistry {
    fn default() -> Self {
        Self::new()
    }
}
