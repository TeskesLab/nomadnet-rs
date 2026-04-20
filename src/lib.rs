#![cfg_attr(docsrs, feature(doc_auto_cfg))]
//! # nomadnet-rs
//!
//! Rust library for [NomadNet](https://markqvist.github.io/Reticulum/network/nomadnet.html)
//! node hosting and browsing over the [Reticulum Network Stack](https://reticulum.network/).
//!
//! ## Features
//!
//! - **Node hosting** — serve `.mu` (Micron) pages over RNS Link request/response
//! - **Node discovery** — track announced NomadNet nodes from RNS transport
//! - **Node browsing** — fetch pages from remote NomadNet nodes
//! - **Micron markup** — fluent builder for NomadNet page content
//!
//! ## Example: Serve static pages
//!
//! ```no_run
//! use std::sync::Arc;
//! use nomadnet_rs::{NomadNode, NodeConfig, PageCache, MicronBuilder};
//! use rns_net::RnsNode;
//!
//! # let node: Arc<RnsNode> = unimplemented!();
//! # let identity_prv: [u8; 64] = [0; 64];
//! # let identity_pub: [u8; 32] = [0; 32];
//!
//! let config = NodeConfig {
//!     identity_prv,
//!     identity_pub,
//!     node_name: "my-node".into(),
//!     announce_interval_secs: 600,
//! };
//!
//! let paths = ["/page/index.mu", "/page/about.mu"];
//! let nomad = NomadNode::new(node, config, &paths)?;
//! let cache = nomad.page_cache();
//!
//! // Populate pages from your async context
//! let mut page = MicronBuilder::new();
//! page.heading(1, "Hello from NomadNet");
//! page.text("This page is served via Reticulum.");
//! cache.set("/page/index.mu", page.build().into_bytes());
//!
//! // Start periodic announces
//! // nomad.start_announcing(node, cancel)?;
//! # Ok::<(), nomadnet_rs::NomadError>(())
//! ```
//!
//! ## Example: Build a Micron page
//!
//! ```
//! use nomadnet_rs::MicronBuilder;
//!
//! let mut page = MicronBuilder::new();
//! page.cache_directive(300);
//! page.heading(1, "My Page");
//! page.divider();
//! page.bold("Status: ");
//! page.text("online");
//! page.blank_line();
//! page.link("Go to index", "aabbccdd:/page/index.mu");
//!
//! let markup = page.build();
//! assert!(markup.starts_with("#!c=300"));
//! assert!(markup.contains("> My Page"));
//! ```

pub mod browser;
pub mod directory;
pub mod micron;
pub mod node;
pub mod types;

pub use browser::NomadBrowser;
pub use directory::{associated_lxmf_dest_hash, is_nomadnet_announce, NomadDirectory};
pub use micron::MicronBuilder;
pub use node::{
    base_path_from_paginated, build_paginated_page, paginate_path, split_into_chunks, NomadNode,
    PageCache,
};
pub use node::{CHUNK_TARGET_BYTES, MAX_PAGES_PER_FILE, MAX_RESPONSE_BYTES};
pub use types::DirectoryEntry;
pub use types::{BrowseEvent, NodeConfig, NomadError};
