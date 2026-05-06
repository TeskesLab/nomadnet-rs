use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rns_core::destination::destination_hash;
use rns_crypto::identity::Identity;
use rns_net::{Destination, IdentityHash, RnsNode};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::micron::MicronBuilder;
use crate::types::{NodeConfig, NomadError};

/// Metadata for a served file.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub content: Vec<u8>,
    pub name: String,
}

/// Thread-safe file cache for NomadNet binary file serving.
///
/// Works like [`PageCache`] but stores [`FileEntry`] structs with file names.
#[derive(Clone, Debug)]
pub struct FileCache {
    inner: Arc<RwLock<HashMap<String, FileEntry>>>,
}

impl FileCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn set(&self, path: &str, entry: FileEntry) {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_string(), entry);
    }

    pub fn get(&self, path: &str) -> Option<FileEntry> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .cloned()
    }

    pub fn remove(&self, path: &str) {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(path);
    }

    pub fn paths(&self) -> Vec<String> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

impl Default for FileCache {
    fn default() -> Self {
        Self::new()
    }
}

pub const MAX_RESPONSE_BYTES: usize = 350;
pub const MAX_PAGES_PER_FILE: usize = 200;
pub const CHUNK_TARGET_BYTES: usize = 220;

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
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_string(), content);
    }

    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .cloned()
    }

    pub fn remove(&self, path: &str) {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(path);
    }

    pub fn paths(&self) -> Vec<String> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
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

pub fn paginate_path(base_path: &str, page_num: usize) -> String {
    if page_num <= 1 {
        return base_path.to_string();
    }
    let stem = base_path.strip_suffix(".mu").unwrap_or(base_path);
    format!("{stem}/{page_num}.mu")
}

pub fn split_into_chunks(content: &str, target_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        if current.len() + line.len() + 1 > target_bytes && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

pub fn base_path_from_paginated(path: &str) -> (String, Option<usize>) {
    let stem = path.strip_suffix(".mu").unwrap_or(path);
    if let Some(idx) = stem.rfind('/') {
        let base_stem = &stem[..idx];
        let page_str = &stem[idx + 1..];
        if let Ok(page_num) = page_str.parse::<usize>() {
            if (2..=MAX_PAGES_PER_FILE).contains(&page_num) {
                return (format!("{base_stem}.mu"), Some(page_num));
            }
        }
    }
    (path.to_string(), None)
}

pub fn build_paginated_page(
    chunk: &str,
    page_num: usize,
    total_pages: usize,
    base_path: &str,
    nomad_address: &str,
) -> Vec<u8> {
    let mut page = MicronBuilder::new();
    page.cache_directive(0);
    page.text_raw_line(chunk);

    if total_pages > 1 {
        page.blank_line();
        page.divider();
        page.blank_line();

        if page_num > 1 {
            let prev_link = paginate_path(base_path, page_num - 1);
            page.link("<< Previous page", &format!("{nomad_address}:{prev_link}"));
        } else {
            page.text_raw_line("  ");
        }

        page.text_raw_line(&format!("  Page {page_num} of {total_pages}"));

        if page_num < total_pages {
            let next_link = paginate_path(base_path, page_num + 1);
            page.link("Next page >>", &format!("{nomad_address}:{next_link}"));
        } else {
            page.text_raw_line("  ");
        }
    }

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
    file_cache: FileCache,
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
        node: Arc<RnsNode>,
        config: NodeConfig,
        paths: &[&str],
        file_paths: &[&str],
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

        for path in paths {
            let cache = page_cache.clone();
            let path_owned = path.to_string();
            let nomad_addr = nomad_address.clone();
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
                        build_404_page(req_path, &nomad_addr)
                    });
                    Some(page)
                },
            )
            .map_err(|_| NomadError::DestinationRegistrationFailed)?;

            info!("NomadNode: registered handler for {}", path);
        }

        // Register a static test page — always works, useful for debugging.
        {
            let page_bytes = {
                let mut p = MicronBuilder::new();
                p.cache_directive(0);
                p.heading(1, "Test Page");
                p.divider();
                p.text("If you can read this, the built-in response path works!");
                p.blank_line();
                p.link("Back to index", &format!("{nomad_address}:/page/index.mu"));
                p.build().into_bytes()
            };
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

        let file_cache = FileCache::new();
        for fpath in file_paths {
            let fc = file_cache.clone();
            let fpath_owned = fpath.to_string();
            node.register_request_handler(
                fpath,
                None,
                move |link_id, req_path, _data, _remote_identity| {
                    info!(
                        "NomadNode: file request on link {:02x?} for path={}",
                        &link_id[..4],
                        req_path
                    );
                    match fc.get(&fpath_owned) {
                        Some(entry) => {
                            info!(
                                "NomadNode: serving file {} ({} bytes)",
                                entry.name,
                                entry.content.len()
                            );
                            Some(entry.content.clone())
                        }
                        None => {
                            warn!("NomadNode: file cache miss for {}", fpath_owned);
                            None
                        }
                    }
                },
            )
            .map_err(|_| NomadError::DestinationRegistrationFailed)?;
            info!("NomadNode: registered FILE handler for {}", fpath);
        }

        info!(
            "NomadNode initialized: dest={} name=\"{}\" ({} pages + test, {} files)",
            hex::encode(dest_hash),
            config.node_name,
            paths.len(),
            file_paths.len()
        );

        Ok(Self {
            dest_hash,
            identity_hash: identity_hash_bytes,
            identity_prv: config.identity_prv,
            node_name: config.node_name,
            announce_interval_secs: config.announce_interval_secs,
            page_cache,
            file_cache,
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

    /// Get a clone of the file cache.  The binary uses this to populate files
    /// from its async main loop.
    pub fn file_cache(&self) -> FileCache {
        self.file_cache.clone()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paginate_path_base() {
        assert_eq!(paginate_path("/page/foo.mu", 1), "/page/foo.mu");
    }

    #[test]
    fn test_paginate_path_subpages() {
        assert_eq!(paginate_path("/page/foo.mu", 2), "/page/foo/2.mu");
        assert_eq!(paginate_path("/page/foo.mu", 5), "/page/foo/5.mu");
        assert_eq!(paginate_path("/page/bar baz.mu", 3), "/page/bar baz/3.mu");
    }

    #[test]
    fn test_base_path_from_paginated() {
        assert_eq!(
            base_path_from_paginated("/page/foo/2.mu"),
            ("/page/foo.mu".to_string(), Some(2))
        );
        assert_eq!(
            base_path_from_paginated("/page/foo/10.mu"),
            ("/page/foo.mu".to_string(), Some(10))
        );
        assert_eq!(
            base_path_from_paginated("/page/foo.mu"),
            ("/page/foo.mu".to_string(), None)
        );
        assert_eq!(
            base_path_from_paginated("/page/foo/1.mu"),
            ("/page/foo/1.mu".to_string(), None)
        );
    }

    #[test]
    fn test_split_into_chunks_small() {
        let content = "line1\nline2";
        let chunks = split_into_chunks(content, 280);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "line1\nline2");
    }

    #[test]
    fn test_split_into_chunks_splits_at_line_boundary() {
        let mut content = String::new();
        for i in 0..20 {
            content.push_str(&format!("line {i:04} — some content here\n"));
        }
        let chunks = split_into_chunks(&content, 100);
        assert!(chunks.len() > 1, "expected multiple chunks");
        for chunk in &chunks {
            assert!(chunk.len() <= 150, "chunk too large: {} bytes", chunk.len());
        }
        let reassembled: String = chunks.join("\n");
        for i in 0..20 {
            assert!(
                reassembled.contains(&format!("line {i:04}")),
                "missing line {i}"
            );
        }
    }

    #[test]
    fn test_build_paginated_page_single() {
        let page = build_paginated_page("Hello", 1, 1, "/page/test.mu", "abcd1234");
        let text = String::from_utf8_lossy(&page);
        assert!(text.contains("Hello"));
        assert!(!text.contains("Previous"), "no prev link on single page");
        assert!(!text.contains("Next"), "no next link on single page");
    }

    #[test]
    fn test_build_paginated_page_first_of_many() {
        let page = build_paginated_page("content", 1, 3, "/page/big.mu", "abcd1234");
        let text = String::from_utf8_lossy(&page);
        assert!(text.contains("content"));
        assert!(!text.contains("Previous"), "no prev on first page");
        assert!(text.contains("Next page >>"));
        assert!(text.contains("Page 1 of 3"));
    }

    #[test]
    fn test_build_paginated_page_middle() {
        let page = build_paginated_page("content", 2, 3, "/page/big.mu", "abcd1234");
        let text = String::from_utf8_lossy(&page);
        assert!(text.contains("<< Previous page"));
        assert!(text.contains("Next page >>"));
        assert!(text.contains("Page 2 of 3"));
    }

    #[test]
    fn test_build_paginated_page_last() {
        let page = build_paginated_page("content", 3, 3, "/page/big.mu", "abcd1234");
        let text = String::from_utf8_lossy(&page);
        assert!(text.contains("<< Previous page"));
        assert!(!text.contains("Next"), "no next on last page");
        assert!(text.contains("Page 3 of 3"));
    }

    #[test]
    fn test_paginated_pages_within_max_response() {
        let mut big_content = String::new();
        for i in 0..50 {
            big_content.push_str(&format!(
                "This is line number {i} with some padding text.\n"
            ));
        }
        let chunks = split_into_chunks(&big_content, CHUNK_TARGET_BYTES);
        for (idx, chunk) in chunks.iter().enumerate() {
            let page =
                build_paginated_page(chunk, idx + 1, chunks.len(), "/page/big.mu", "abcd1234");
            assert!(
                page.len() <= MAX_RESPONSE_BYTES,
                "page {} is {} bytes, exceeds max {}",
                idx + 1,
                page.len(),
                MAX_RESPONSE_BYTES
            );
        }
    }
}
