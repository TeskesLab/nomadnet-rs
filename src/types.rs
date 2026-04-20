pub const DEFAULT_ANNOUNCE_INTERVAL_SECS: u64 = 600;
pub const MAX_DIRECTORY_ENTRIES: usize = 256;

/// Configuration for creating a [`NomadNode`](crate::NomadNode).
pub struct NodeConfig {
    pub identity_prv: [u8; 64],
    pub identity_pub: [u8; 32],
    pub node_name: String,
    pub announce_interval_secs: u64,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            identity_prv: [0u8; 64],
            identity_pub: [0u8; 32],
            node_name: String::from("Unnamed Node"),
            announce_interval_secs: DEFAULT_ANNOUNCE_INTERVAL_SECS,
        }
    }
}

/// A discovered NomadNet node in the directory.
pub struct DirectoryEntry {
    pub dest_hash: [u8; 16],
    pub identity_hash: [u8; 16],
    pub node_name: Option<String>,
    pub hops: u8,
    pub last_seen: f64,
}

/// Events emitted by [`NomadBrowser`](crate::NomadBrowser).
pub enum BrowseEvent {
    PageReceived {
        dest_hash: [u8; 16],
        path: String,
        content: Vec<u8>,
    },
    LinkEstablished {
        dest_hash: [u8; 16],
        link_id: [u8; 16],
    },
    LinkFailed {
        dest_hash: [u8; 16],
        error: String,
    },
    LinkClosed {
        dest_hash: [u8; 16],
        link_id: [u8; 16],
        reason: Option<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum NomadError {
    #[error("RNS send error")]
    SendFailed(#[from] rns_net::SendError),

    #[error("No path to destination {0}")]
    NoPath(String),

    #[error("Identity not found for destination {0}")]
    IdentityNotFound(String),

    #[error("Link creation failed for destination {0}")]
    LinkFailed(String),

    #[error("Request failed for path {0}")]
    RequestFailed(String),

    #[error("Destination registration failed")]
    DestinationRegistrationFailed,

    #[error(
        "Identity public key mismatch: expected {expected_sig_pub_hex}, got {provided_sig_pub_hex}"
    )]
    IdentityKeyMismatch {
        expected_sig_pub_hex: String,
        provided_sig_pub_hex: String,
    },
}
