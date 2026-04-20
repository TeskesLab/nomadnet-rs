mod config;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use config::RnsConfig;
use nomadnet_rs::{MicronBuilder, NodeConfig, NomadNode, PageCache};
use rns_core::transport::types::IngressControlConfig;
use rns_crypto::identity::Identity;
use rns_net::{
    Callbacks, InterfaceConfig as RnsInterfaceConfig, NodeConfig as RnsNodeConfig, RnsNode,
    TcpClientConfig, TcpServerConfig, UdpConfig, MODE_FULL,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "nomadnet-serve",
    about = "Serve static .mu pages as a NomadNet node"
)]
struct Args {
    #[arg(long, default_value = "~/.nomadnet-serve/identity")]
    identity: String,

    #[arg(long, default_value = "~/.nomadnet-serve/storage")]
    storage: String,

    #[arg(long)]
    rns_config: Option<String>,

    #[arg(short, long, default_value = ".")]
    pages_dir: String,

    #[arg(long, default_value = "nomadnet-serve")]
    node_name: String,

    #[arg(long, default_value = "600")]
    announce_interval: u64,

    #[arg(long)]
    verbose: bool,

    #[arg(long)]
    watch: bool,
}

struct NoopCallbacks;

impl Callbacks for NoopCallbacks {
    fn on_announce(&mut self, _: rns_net::common::destination::AnnouncedIdentity) {}
    fn on_path_updated(&mut self, _: rns_net::DestHash, _: u8) {}
    fn on_local_delivery(&mut self, _: rns_net::DestHash, _: Vec<u8>, _: rns_net::PacketHash) {}
}

fn expand_path(path: &str) -> PathBuf {
    if path.starts_with('~') {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

fn load_or_create_identity(
    path: &Path,
) -> Result<(Identity, [u8; 64], [u8; 32]), Box<dyn std::error::Error>> {
    if path.exists() {
        let bytes = std::fs::read(path)?;
        let prv = if bytes.len() == 64 {
            let mut arr = [0u8; 64];
            arr.copy_from_slice(&bytes);
            arr
        } else {
            let content = String::from_utf8(bytes)
                .map_err(|e| format!("Identity file is neither binary nor valid UTF-8: {e}"))?;
            let trimmed = content.trim();
            if trimmed.len() == 128 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                let decoded = hex::decode(trimmed)
                    .map_err(|e| format!("Failed to decode hex identity: {e}"))?;
                let mut arr = [0u8; 64];
                arr.copy_from_slice(&decoded);
                arr
            } else {
                return Err("Identity file must be 64 bytes (binary) or 128 hex characters".into());
            }
        };
        let identity = Identity::from_private_key(&prv);
        let full_pub = identity
            .get_public_key()
            .ok_or("Failed to get public key")?;
        let mut pub_arr = [0u8; 32];
        pub_arr.copy_from_slice(&full_pub[32..64]);
        Ok((identity, prv, pub_arr))
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let identity = Identity::new(&mut rns_crypto::OsRng);
        let full_pub = identity
            .get_public_key()
            .ok_or("Failed to get public key")?;
        let mut pub_arr = [0u8; 32];
        pub_arr.copy_from_slice(&full_pub[32..64]);
        let prv_bytes = identity
            .get_private_key()
            .ok_or("Failed to get private key")?;
        let mut prv_arr = [0u8; 64];
        prv_arr.copy_from_slice(&prv_bytes);
        let hex_str = hex::encode(prv_arr);
        std::fs::write(path, hex_str)?;
        info!("Created new identity at {}", path.display());
        Ok((identity, prv_arr, pub_arr))
    }
}

fn build_interfaces(rns_config_path: &Option<PathBuf>) -> Vec<RnsInterfaceConfig> {
    let config_path = match rns_config_path {
        Some(p) => p.clone(),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                let default = PathBuf::from(home).join(".config/reticulum/config");
                if default.exists() {
                    default
                } else {
                    return Vec::new();
                }
            } else {
                return Vec::new();
            }
        }
    };

    let rns_config = match RnsConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!(
                "Failed to parse RNS config {}: {}",
                config_path.display(),
                e
            );
            return Vec::new();
        }
    };

    let mut interfaces = Vec::new();

    for iface in &rns_config.interfaces {
        if !iface.enabled {
            continue;
        }
        match &iface.iface_type {
            config::InterfaceType::TcpClient {
                target_host,
                target_port,
            } => {
                interfaces.push(RnsInterfaceConfig {
                    name: iface.name.clone(),
                    type_name: "TCPClientInterface".to_string(),
                    config_data: Box::new(TcpClientConfig {
                        name: iface.name.clone(),
                        target_host: target_host.clone(),
                        target_port: *target_port,
                        ..Default::default()
                    }),
                    mode: MODE_FULL,
                    ifac: None,
                    discovery: None,
                    ingress_control: IngressControlConfig::enabled(),
                });
            }
            config::InterfaceType::TcpServer { listen_port } => {
                interfaces.push(RnsInterfaceConfig {
                    name: iface.name.clone(),
                    type_name: "TCPServerInterface".to_string(),
                    config_data: Box::new(TcpServerConfig {
                        name: iface.name.clone(),
                        listen_ip: "0.0.0.0".to_string(),
                        listen_port: *listen_port,
                        ingress_control: IngressControlConfig::enabled(),
                        ..Default::default()
                    }),
                    mode: MODE_FULL,
                    ifac: None,
                    discovery: None,
                    ingress_control: IngressControlConfig::enabled(),
                });
            }
            config::InterfaceType::Udp { bind_addr } => {
                let parts: Vec<&str> = bind_addr.splitn(2, ':').collect();
                let listen_ip = parts
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "0.0.0.0".to_string());
                let listen_port = parts
                    .get(1)
                    .and_then(|p| p.parse::<u16>().ok())
                    .unwrap_or(4242);
                interfaces.push(RnsInterfaceConfig {
                    name: iface.name.clone(),
                    type_name: "UDPInterface".to_string(),
                    config_data: Box::new(UdpConfig {
                        name: iface.name.clone(),
                        listen_ip: Some(listen_ip),
                        listen_port: Some(listen_port),
                        forward_ip: Some("255.255.255.255".to_string()),
                        forward_port: Some(listen_port),
                        ..Default::default()
                    }),
                    mode: MODE_FULL,
                    ifac: None,
                    discovery: None,
                    ingress_control: IngressControlConfig::enabled(),
                });
            }
        }
    }

    interfaces
}

fn scan_pages(pages_dir: &Path) -> Vec<String> {
    fn recurse_collect(base: &Path, current: &Path, out: &mut Vec<String>) {
        let entries = match std::fs::read_dir(current) {
            Ok(e) => e,
            Err(err) => {
                warn!("Failed to read pages directory {}: {}", current.display(), err);
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                recurse_collect(base, &path, out);
                continue;
            }

            if !path.is_file() {
                continue;
            }

            let is_mu = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mu"))
                .unwrap_or(false);
            if !is_mu {
                continue;
            }

            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    let mut pages = Vec::new();
    if !pages_dir.is_dir() {
        return pages;
    }

    recurse_collect(pages_dir, pages_dir, &mut pages);
    pages.sort();
    pages
}

fn build_auto_index(pages: &[String], nomad_address: &str) -> Vec<u8> {
    let mut page = MicronBuilder::new();
    page.cache_directive(30);
    page.heading(1, "nomadnet-serve");
    page.divider();
    page.text_raw_line("Pages served from this node:");
    page.blank_line();

    if pages.is_empty() {
        page.text("No pages available.");
    } else {
        for name in pages {
            let safe_name = MicronBuilder::escape(name);
            let link = format!("{nomad_address}:/page/{safe_name}");
            page.text_raw_line(&format!("`[{safe_name}`{link}]"));
        }
    }

    page.build().into_bytes()
}

fn replace_self(content: &str, nomad_address: &str) -> String {
    content.replace("$SELF", nomad_address)
}

fn populate_cache(cache: &PageCache, pages_dir: &Path, nomad_address: &str) {
    let pages = scan_pages(pages_dir);
    let has_index = pages.iter().any(|p| p == "index.mu");

    for name in &pages {
        let file_path = pages_dir.join(name);
        if let Ok(content) = std::fs::read(&file_path) {
            let page_path = format!("/page/{name}");
            let replaced = replace_self(&String::from_utf8_lossy(&content), nomad_address);
            cache.set(&page_path, replaced.into_bytes());
        }
    }

    if !has_index {
        let index = build_auto_index(&pages, nomad_address);
        cache.set("/page/index.mu", index);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if args.verbose {
            tracing_subscriber::EnvFilter::new("debug")
        } else {
            tracing_subscriber::EnvFilter::new("info,rns_net=warn,rns_core=warn")
        }
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("Starting nomadnet-serve...");

    let identity_path = expand_path(&args.identity);
    let storage_path = expand_path(&args.storage);
    let pages_dir = expand_path(&args.pages_dir);
    let rns_config_path = args.rns_config.as_ref().map(|s| expand_path(s));

    let (identity, identity_prv, identity_pub) = load_or_create_identity(&identity_path)?;
    let identity_hash_hex = hex::encode(identity.hash());
    info!("Identity hash: {}", identity_hash_hex);

    std::fs::create_dir_all(&storage_path)?;

    if !pages_dir.is_dir() {
        std::fs::create_dir_all(&pages_dir)?;
        info!("Created pages directory at {}", pages_dir.display());
    }

    let interfaces = build_interfaces(&rns_config_path);
    info!("Configured {} interface(s)", interfaces.len());

    let node_config = RnsNodeConfig {
        identity: Some(identity),
        interfaces,
        transport_enabled: true,
        cache_dir: Some(storage_path.clone()),
        ..Default::default()
    };

    let node = Arc::new(RnsNode::start(node_config, Box::new(NoopCallbacks))?);
    info!("RNS node started");

    let cancel = CancellationToken::new();

    let page_paths: Vec<String> = {
        let all_files = scan_pages(&pages_dir);
        let has_index = all_files.iter().any(|p| p == "index.mu");
        let mut paths: Vec<String> = all_files.iter().map(|f| format!("/page/{f}")).collect();
        if !has_index {
            paths.push("/page/index.mu".to_string());
        }
        paths
    };

    let page_path_refs: Vec<&str> = page_paths.iter().map(|s| s.as_str()).collect();

    let nomad_node = {
        let config = NodeConfig {
            identity_prv,
            identity_pub,
            node_name: args.node_name.clone(),
            announce_interval_secs: args.announce_interval,
        };
        let nn = NomadNode::new(&node, config, &page_path_refs)?;
        nn.start_announcing(node.clone(), cancel.clone())?;
        nn
    };

    let page_cache = nomad_node.page_cache();
    let nomad_address = hex::encode(nomad_node.dest_hash());

    info!("NomadNet node dest: {}", nomad_address);
    info!("Pages directory: {}", pages_dir.display());

    populate_cache(&page_cache, &pages_dir, &nomad_address);
    info!("Loaded {} pages", page_cache.paths().len());

    let watch_rx = if args.watch {
        use notify::{recommended_watcher, RecursiveMode, Watcher};
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = recommended_watcher(tx)?;
        watcher.watch(&pages_dir, RecursiveMode::Recursive)?;
        info!("Watching {} for changes", pages_dir.display());
        Some(rx)
    } else {
        None
    };

    if let Some(rx) = watch_rx {
        let watch_cancel = cancel.clone();
        std::thread::spawn(move || {
            loop {
                if watch_cancel.is_cancelled() {
                    break;
                }
                match rx.recv_timeout(std::time::Duration::from_secs(1)) {
                    Ok(Ok(event)) => {
                        if let notify::EventKind::Modify(_)
                        | notify::EventKind::Create(_)
                        | notify::EventKind::Remove(_) = event.kind
                        {
                            populate_cache(&page_cache, &pages_dir, &nomad_address);
                            info!(
                                "Page cache refreshed ({} pages)",
                                page_cache.paths().len()
                            );
                        }
                    }
                    Ok(Err(e)) => {
                        warn!("File watch error: {}", e);
                    }
                    Err(_) => {}
                }
            }
        });
    }

    loop {
        if cancel.is_cancelled() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    info!("Shutting down...");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{replace_self, scan_pages};
    use std::fs;
    use std::path::PathBuf;

    fn make_temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nomadnet-serve-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("failed to create temp dir");
        path
    }

    #[test]
    fn replace_self_rewrites_all_placeholders() {
        let in_text = "`[Home`$SELF:/page/index.mu]\n`[Users`$SELF:/page/users.mu]";
        let out = replace_self(in_text, "deadbeefcafebabe");
        assert!(!out.contains("$SELF"));
        assert!(out.contains("deadbeefcafebabe:/page/index.mu"));
        assert!(out.contains("deadbeefcafebabe:/page/users.mu"));
    }

    #[test]
    fn scan_pages_recurses_and_returns_relative_paths() {
        let root = make_temp_dir("scan-recursive");
        let nested = root.join("docs/sub");
        fs::create_dir_all(&nested).expect("failed to create nested dir");

        fs::write(root.join("index.mu"), b"index").expect("failed to write index.mu");
        fs::write(root.join("README.txt"), b"ignore").expect("failed to write README.txt");
        fs::write(root.join("docs/guide.mu"), b"guide").expect("failed to write guide.mu");
        fs::write(nested.join("deep.mu"), b"deep").expect("failed to write deep.mu");

        let pages = scan_pages(&root);
        assert!(pages.contains(&"index.mu".to_string()));
        assert!(pages.contains(&"docs/guide.mu".to_string()));
        assert!(pages.contains(&"docs/sub/deep.mu".to_string()));
        assert!(!pages.contains(&"README.txt".to_string()));

        let _ = fs::remove_dir_all(root);
    }
}
