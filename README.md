# nomadnet-rs

Rust library for [NomadNet](https://markqvist.github.io/Reticulum/network/nomadnet.html) node hosting and browsing over the [Reticulum Network Stack](https://reticulum.network/).

Zero-application-logic dependencies — works with any RNS-based project.

## Features

- **Node hosting** — serve `.mu` (Micron) pages over RNS Link request/response
- **Node discovery** — track announced NomadNet nodes from RNS transport
- **Node browsing** — fetch pages from remote NomadNet nodes via [`NomadBrowser`]
- **Micron markup** — fluent builder for NomadNet page content

## `nomadnet-serve` Binary

The `nomadnet-serve` binary serves static `.mu` files from a folder as a NomadNet node. Enable the `serve` feature:

```toml
[dependencies]
nomadnet-rs = { version = "0.1", features = ["serve"] }
```

```bash
# Install from source
cargo install nomadnet-rs --features serve

# Run
nomadnet-serve -p ./my-pages --rns-config ~/.config/reticulum/config --watch
```

Drop `.mu` files in the pages directory. If no `index.mu` exists, one is auto-generated listing all available pages.

| Flag | Default | Description |
|------|---------|-------------|
| `-p, --pages-dir` | `.` | Directory containing `.mu` files |
| `--rns-config` | `~/.config/reticulum/config` | RNS config file |
| `--identity` | `~/.nomadnet-serve/identity` | Identity file path |
| `--node-name` | `nomadnet-serve` | Node display name (for announces) |
| `--announce-interval` | `600` | Announce interval in seconds |
| `--watch` | off | Watch pages directory for changes |
| `--verbose` | off | Enable debug logging |

## Quick Start (Library)

```toml
[dependencies]
nomadnet-rs = "0.1"
rns-core = "0.1"
rns-crypto = "0.1"
rns-net = "0.5"
```

### Serve pages

```rust
use std::sync::Arc;
use nomadnet_rs::{NomadNode, NodeConfig, PageCache, MicronBuilder};
use rns_net::RnsNode;

let paths = ["/page/index.mu", "/page/about.mu"];
let nomad = NomadNode::new(&node, config, &paths)?;
let cache = nomad.page_cache();

let mut page = MicronBuilder::new();
page.heading(1, "Hello from NomadNet");
page.text("Served via Reticulum.");
cache.set("/page/index.mu", page.build().into_bytes());
```

### Build a Micron page

```rust
use nomadnet_rs::MicronBuilder;

let mut page = MicronBuilder::new();
page.cache_directive(300);
page.heading(1, "My Page");
page.divider();
page.bold("Status: ");
page.text("online");
page.blank_line();
page.link("Index", "aabbccdd:/page/index.mu");
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

// In your event loop:
if let Some(BrowseEvent::PageReceived { content, .. }) = events.recv().await {
    println!("Received {} bytes", content.len());
}
```

## Modules

| Module | Description |
|--------|-------------|
| [`node`] | `NomadNode` + `PageCache` — serve pages via RNS Link request/response |
| [`micron`] | `MicronBuilder` — fluent API for NomadNet page markup |
| [`directory`] | `NomadDirectory` — track discovered NomadNet nodes from announces |
| [`browser`] | `NomadBrowser` — fetch pages from remote nodes |
| [`types`] | Shared types: `NodeConfig`, `BrowseEvent`, `DirectoryEntry`, `NomadError` |

## Architecture Notes

- NomadNet uses RNS Link request/response (not LXMF messaging) on aspect `nomadnetwork.node`
- Page handlers run synchronously on the RNS driver thread — use `PageCache` to decouple async page generation from sync request handling
- A built-in `/page/test.mu` handler is always registered for connectivity debugging
- Cache misses return auto-generated 404 pages

## License

MIT
