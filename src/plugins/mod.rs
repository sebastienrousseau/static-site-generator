// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

pub mod accessibility;
pub mod agent_api;
pub mod ai;
pub mod assets;
pub mod audit;
pub mod csp;
pub mod drafts;
pub mod highlight;
pub mod i18n;
#[cfg(feature = "image-optimization")]
pub mod image_plugin;
pub mod islands;
pub mod isr_manifest;
pub mod listings;
pub mod llm;
pub mod llm_cache;
pub mod markdown_ext;
pub mod oembed;
pub mod og_image;
pub mod pagination;
pub mod plugin;
pub mod plugins;
pub mod postprocess;
pub mod rpc_schema;
pub mod sbom;
pub mod search;
pub mod search_index;
pub mod seo;
pub mod shortcodes;
pub mod taxonomy;
#[cfg(feature = "templates")]
pub mod template_plugin;
pub mod topic_clusters;
pub mod view_transitions;
