// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! AI filters for HTTP workloads: inference routing, prompt enrichment,
//! and agentic protocol classification.

pub(crate) mod agentic;
#[cfg(feature = "ai-inference")]
mod inference;
#[cfg(feature = "ai-inference")]
pub(crate) mod openai;
#[cfg(feature = "ai-inference")]
mod prompt_enrich;
#[cfg(feature = "ai-inference")]
#[allow(
    dead_code,
    reason = "store module is the foundation for upcoming response store filter"
)]
pub(crate) mod store;

pub use agentic::{A2aFilter, JsonRpcFilter, McpFilter};
#[cfg(feature = "ai-inference")]
pub use inference::ModelToHeaderFilter;
#[cfg(feature = "ai-inference")]
pub use openai::ResponsesFormatFilter;
#[cfg(feature = "ai-inference")]
pub use prompt_enrich::PromptEnrichFilter;
#[cfg(feature = "ai-inference")]
#[allow(unused_imports, reason = "re-exports for upcoming response store filter")]
pub use store::{ResponseStoreRegistry, SqliteResponseStore};
