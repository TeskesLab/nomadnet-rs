use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rns_core::destination::destination_hash;
use rns_crypto::identity::Identity;
use rns_net::{Destination, IdentityHash, RnsNode};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::micron::MicronBuilder;
use crate::types::{NodeConfig, NomadError};

/// Thread-safe page cache for NomadNet pages.
///
/// Handlers (running synchronously on the RNS driver thread) read from the cache,
/// while the application writes to it from async or sync context. Cloning is cheap
/// — it's `Arc` under the hood.
#[derive(Clone, Debug)]
pub struct PageCache {
    inner: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl PageCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert or update a page.  Called from the async main loop.
    pub fn set(&self, path: &str, content: Vec<u8>) {
        self.inner
            .write()
            .unwrap()
            .insert(path.to_string(), content);
    }

    /// Read a page.  Called from sync request handlers on the RNS driver thread.
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.inner.read().unwrap().get(path).cloned()
    }

    /// Remove a page from the cache.
    pub fn remove(&self, path: &str) {
        self.inner.write().unwrap().remove(path);
    }

    /// List all cached paths.
    pub fn paths(&self) -> Vec<String> {
        self.inner.read().unwrap().keys().cloned().collect()
    }
}

impl Default for PageCache {
    fn default() -> Self {
        Self::new()
    }
}

fn build_404_page(path: &str, nomad_address: &str) -> Vec<u8> {
    let mut page = MicronBuilder::new();
    page.cache_directive(0);
    page.heading(1, "404 — Page Not Found");
    page.divider();
    let escaped = MicronBuilder::escape(path);
    page.text_raw_line(&format!(
        "The page `{escaped}` does not exist on this node."
    ));
    page.blank_line();
    page.link("Back to index", &format!("{nomad_address}:/page/index.mu"));
    page.build().into_bytes()
}

/// A NomadNet node that serves pages via RNS Link request/response.
///
/// Register page paths at construction time. Handlers read from a shared
/// [`PageCache`] synchronously — populate the cache from your async context
/// (e.g. a periodic timer or state-change hook).
///
/// A built-in `/page/test.mu` handler is always registered for debugging.
pub struct NomadNode {
    dest_hash: [u8; 16],
    identity_hash: [u8; 16],
    identity_prv: [u8; 64],
    node_name: String,
    announce_interval_secs: u64,
    page_cache: PageCache,
}

impl NomadNode {
    /// Create a NomadNode that serves pages from a `PageCache`.
    ///
    /// `paths` lists the page paths to register handlers for (e.g.,
    /// `["/page/index.mu", "/page/channels.mu"]`).  A `/page/test.mu` test
    /// handler is always registered automatically.
    ///
    /// Handlers read from the `PageCache` synchronously.  The application is
    /// responsible for populating the cache from its async context (e.g., a
    /// periodic timer in the main loop).
    pub fn new(
        node: &Arc<RnsNode>,
        config: NodeConfig,
        paths: &[&str],
    ) -> Result<Self, NomadError> {
        let identity = Identity::from_private_key(&config.identity_prv);
        let identity_hash_bytes = *identity.hash();
        let identity_hash = IdentityHash(identity_hash_bytes);

        let dest_hash = destination_hash("nomadnetwork", &["node"], Some(&identity_hash_bytes));

        debug!(
            "NomadNode: identity={} dest={}",
            hex::encode(identity_hash_bytes),
            hex::encode(dest_hash)
        );

        let sig_prv_bytes: [u8; 32] = config.identity_prv[32..64]
            .try_into()
            .map_err(|_| NomadError::DestinationRegistrationFailed)?;
        let derived_pub = identity
            .get_public_key()
            .ok_or(NomadError::DestinationRegistrationFailed)?;
        let sig_pub_bytes: [u8; 32] = derived_pub[32..64]
            .try_into()
            .map_err(|_| NomadError::DestinationRegistrationFailed)?;

        if config.identity_pub != sig_pub_bytes {
            return Err(NomadError::IdentityKeyMismatch {
                expected_sig_pub_hex: hex::encode(sig_pub_bytes),
                provided_sig_pub_hex: hex::encode(config.identity_pub),
            });
        }

        let inbound_dest = Destination::single_in("nomadnetwork", &["node"], identity_hash);

        node.register_destination_with_proof(&inbound_dest, Some(config.identity_prv))
            .map_err(|_| NomadError::DestinationRegistrationFailed)?;

        node.register_link_destination(dest_hash, sig_prv_bytes, sig_pub_bytes, 0)
            .map_err(|_| NomadError::DestinationRegistrationFailed)?;

        let page_cache = PageCache::new();
        let nomad_address = hex::encode(dest_hash);

        // Register a handler for each path — reads from the shared cache.
        for path in paths {
            let cache = page_cache.clone();
            let path_owned = path.to_string();
            let nomad_address = nomad_address.clone();
            node.register_request_handler(
                path,
                None,
                move |link_id, req_path, data, _remote_identity| {
                    info!(
                        "NomadNode: request on link {:02x?} for path={} ({} bytes data)",
                        &link_id[..4],
                        req_path,
                        data.len()
                    );
                    let page = cache.get(&path_owned).unwrap_or_else(|| {
                        warn!("NomadNode: cache miss for {}, returning 404", path_owned);
                        build_404_page(&path_owned, &nomad_address)
                    });
                    Some(page)
                },
            )
            .map_err(|_| NomadError::DestinationRegistrationFailed)?;

            info!("NomadNode: registered handler for {}", path);
        }

        // Register a static test page — always works, useful for debugging.
        {
            let static_page = {
                let mut p = MicronBuilder::new();
                p.cache_directive(0);
                p.heading(1, "Test Page");
                p.divider();
                p.text("If you can read this, the built-in response path works!");
                p.blank_line();
                p.link("Back to index", &format!("{nomad_address}:/page/index.mu"));
                p.build().into_bytes()
            };
            let page_bytes = static_page.clone();
            node.register_request_handler(
                "/page/test.mu",
                None,
                move |link_id, req_path, _data, _remote_identity| {
                    info!(
                        "NomadNode: TEST handler on link {:02x?} for path={} — returning static page ({} bytes)",
                        &link_id[..4],
                        req_path,
                        page_bytes.len()
                    );
                    Some(page_bytes.clone())
                },
            )
            .map_err(|_| NomadError::DestinationRegistrationFailed)?;
            info!("NomadNode: registered TEST handler for /page/test.mu");
        }

        info!(
            "NomadNode initialized: dest={} name=\"{}\" ({} pages + test)",
            hex::encode(dest_hash),
            config.node_name,
            paths.len()
        );

        Ok(Self {
            dest_hash,
            identity_hash: identity_hash_bytes,
            identity_prv: config.identity_prv,
            node_name: config.node_name,
            announce_interval_secs: config.announce_interval_secs,
            page_cache,
        })
    }

    pub fn dest_hash(&self) -> &[u8; 16] {
        &self.dest_hash
    }

    pub fn identity_hash(&self) -> &[u8; 16] {
        &self.identity_hash
    }

    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Get a clone of the page cache.  The binary uses this to populate pages
    /// from its async main loop.
    pub fn page_cache(&self) -> PageCache {
        self.page_cache.clone()
    }

    pub fn start_announcing(
        &self,
        node: Arc<RnsNode>,
        cancel: CancellationToken,
    ) -> Result<(), NomadError> {
        let identity = Identity::from_private_key(&self.identity_prv);
        let dest =
            Destination::single_in("nomadnetwork", &["node"], IdentityHash(self.identity_hash));
        let node_name = self.node_name.clone();
        let dest_hash = self.dest_hash;
        let interval_secs = self.announce_interval_secs;

        tokio::spawn(async move {
            let initial_delay = tokio::time::Duration::from_secs(2);

            if tokio::time::timeout(initial_delay, cancel.cancelled())
                .await
                .is_ok()
            {
                return;
            }

            loop {
                let app_data = node_name.as_bytes();
                match node.announce(&dest, &identity, Some(app_data)) {
                    Ok(()) => {
                        info!(
                            "NomadNode announced: dest={} name=\"{}\"",
                            hex::encode(dest_hash),
                            node_name
                        );
                    }
                    Err(e) => {
                        warn!("NomadNode announce failed: {:?}", e);
                    }
                }

                let interval = tokio::time::Duration::from_secs(interval_secs);
                if tokio::time::timeout(interval, cancel.cancelled())
                    .await
                    .is_ok()
                {
                    break;
                }
            }
        });

        Ok(())
    }
}
