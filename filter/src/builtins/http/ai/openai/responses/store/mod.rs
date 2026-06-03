// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! `OpenAI` Responses API store utilities.
//!
//! Helpers that operate on the generic [`ResponseStore`] but are
//! specific to the `OpenAI` Responses API (e.g., input item
//! pagination for the `/v1/responses/{id}/input_items` endpoint).
//!
//! [`ResponseStore`]: crate::builtins::http::ai::store::ResponseStore

mod input_items;

#[allow(unused_imports, reason = "re-exports for upcoming Responses API CRUD endpoints")]
pub use input_items::{InputItemPage, ListParams, list_input_items};
