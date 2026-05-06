# nomadnet-rs

[![CI](https://github.com/TeskesLab/nomadnet-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/TeskesLab/nomadnet-rs/actions/workflows/ci.yml)
[![crate](https://img.shields.io/crates/v/nomadnet-rs.svg?label=nomadnet-rs)](https://crates.io/crates/nomadnet-rs)

Rust library for [NomadNet](https://markqvist.github.io/Reticulum/network/nomadnet.html) node hosting and browsing over the [Reticulum Network Stack](https://reticulum.network/).

Zero-application-logic dependencies — works with any RNS-based project.

## Features

- **Node hosting** — serve `.mu` (Micron) pages and binary files over RNS Link request/response
- **Node discovery** — track announced NomadNet nodes from RNS transport
- **Node browsing** — fetch pages and files from remote NomadNet nodes via [`NomadBrowser`]
- **Micron markup** — fluent builder for NomadNet page content, including tables and partials

## `nomadnet-serve` Binary

The `nomadnet-serve` binary serves static `.mu` files from a folder as a NomadNet node. Enable the `serve` feature:

```toml
[dependencies]
nomadnet-rs = { version = "0.1", features = ["serve"] }
```

```bash
# Install from crates.io
cargo install nomadnet-rs --features serve --bin nomadnet-serve

# Run
nomadnet-serve -p ./my-pages --rns-config ~/.config/reticulum/config --watch
```

Drop `.mu` files in the pages directory. If no `index.mu` exists, one is auto-generated listing all available pages. Use `--files-dir` to serve binary files alongside pages.

| Flag | Default | Description |
|------|---------|-------------|
| `-p, --pages-dir` | `.` | Directory containing `.mu` files |
| `-f, --files-dir` | off | Directory containing binary files to serve |
| `--rns-config` | `~/.config/reticulum/config` | RNS config file |
| `--identity` | `~/.nomadnet-serve/identity` | Identity file path |
| `--node-name` | `nomadnet-serve` | Node display name (for announces) |
| `--announce-interval` | `600` | Announce interval in seconds |
| `--watch` | off | Watch pages/files directories for changes |
| `--verbose` | off | Enable debug logging |

## Quick Start (Library)

```toml
[dependencies]
nomadnet-rs = "0.1"
rns-core = "0.1"
rns-crypto = "0.1"
rns-net = "0.5"
```

### Serve pages and files

```rust
use std::sync::Arc;
use nomadnet_rs::{NomadNode, NodeConfig, PageCache, FileCache, MicronBuilder};
use rns_net::RnsNode;

let pages = ["/page/index.mu", "/page/about.mu"];
let files = ["/file/readme.txt", "/file/data.json"];
let nomad = NomadNode::new(node.clone(), config, &pages, &files)?;
let page_cache = nomad.page_cache();
let file_cache = nomad.file_cache();

let mut page = MicronBuilder::new();
page.heading(1, "Hello from NomadNet");
page.text("Served via Reticulum.");
page_cache.set("/page/index.mu", page.build().into_bytes());
```

### Build a Micron page

```rust
use nomadnet_rs::{MicronBuilder, TableAlign};

let mut page = MicronBuilder::new();
page.cache_directive(300);
page.heading(1, "My Page");
page.divider();
page.bold("Status: ");
page.text("online");
page.blank_line();

// Table
page.table_start(Some(TableAlign::Center), Some(40));
page.table_row(&["Name", "Value"]);
page.table_row(&["----", "-----"]);
page.table_row(&["Users", "5"]);
page.table_end();

page.blank_line();

// Auto-updating partial
page.partial("aabbccdd:/page/stats.mu", Some(10.0), "channel=general");

// Truecolor
page.truecolor_fg("ff5500");
page.text("Bright orange text");
page.reset_fg();
```

### Discover nodes

```rust
use nomadnet_rs::{NomadDirectory, directory::is_nomadnet_announce};

let mut directory = NomadDirectory::new();

// In your RNS callback:
fn on_announce(&mut self, announced: AnnouncedIdentity) {
    if is_nomadnet_announce(&announced) {
        directory.handle_announce(&announced);
    }
}
```

### Browse remote nodes

```rust
use nomadnet_rs::{NomadBrowser, BrowseEvent};

let browser = NomadBrowser::new();
let mut events = browser.events();

browser.fetch(&node, dest_hash, sig_pub, "/page/index.mu")?;

// Fetch with request data
browser.fetch_with_data(&node, dest_hash, sig_pub, "/page/search.mu", b"query=hello")?;

// Fetch a file
browser.fetch_file(&node, dest_hash, sig_pub, "/file/readme.txt", None)?;

// In your event loop:
match events.recv().await {
    Some(BrowseEvent::PageReceived { content, .. }) => { /* handle page */ },
    Some(BrowseEvent::FileReceived { content, path, .. }) => { /* handle file */ },
    _ => {}
}
```

## Modules

| Module | Description |
|--------|-------------|
| [`node`] | `NomadNode` + `PageCache` + `FileCache` — serve pages and files via RNS Link request/response |
| [`micron`] | `MicronBuilder` + `TableAlign` — fluent API for NomadNet page markup |
| [`directory`] | `NomadDirectory` — track discovered NomadNet nodes from announces |
| [`browser`] | `NomadBrowser` — fetch pages and files from remote nodes, with URL field parsing |
| [`types`] | Shared types: `NodeConfig`, `BrowseEvent`, `DirectoryEntry`, `NomadError` |

## Architecture Notes

- NomadNet uses RNS Link request/response (not LXMF messaging) on aspect `nomadnetwork.node`
- Page handlers run synchronously on the RNS driver thread — use `PageCache`/`FileCache` to decouple async page generation from sync request handling
- A built-in `/page/test.mu` handler is always registered for connectivity debugging
- Cache misses for pages return auto-generated 404 pages; file cache misses return no response
- `NomadBrowser` distinguishes page vs file responses via the path prefix (`/page/` vs `/file/`)
- URL field parsing (`url\`field=val`) is supported via `parse_url_fields()` and `fetch_with_data()`

## License

MIT
