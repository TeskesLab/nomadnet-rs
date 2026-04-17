use rns_core::destination::destination_hash;
use rns_net::AnnouncedIdentity;
use tracing::debug;

use crate::types::{DirectoryEntry, MAX_DIRECTORY_ENTRIES};

pub struct NomadDirectory {
    entries: Vec<DirectoryEntry>,
}

impl NomadDirectory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn handle_announce(&mut self, announced: &AnnouncedIdentity) {
        let expected_dest =
            destination_hash("nomadnetwork", &["node"], Some(&announced.identity_hash.0));

        if announced.dest_hash.0 != expected_dest {
            return;
        }

        let node_name = announced
            .app_data
            .as_ref()
            .and_then(|d| std::str::from_utf8(d).ok())
            .map(|s| s.to_string());

        debug!(
            "NomadNet node announce: dest={} identity={} name={:?} hops={}",
            hex::encode(announced.dest_hash.0),
            hex::encode(announced.identity_hash.0),
            node_name,
            announced.hops
        );

        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.dest_hash == announced.dest_hash.0)
        {
            entry.node_name = node_name;
            entry.hops = announced.hops;
            entry.last_seen = announced.received_at;
        } else {
            self.entries.push(DirectoryEntry {
                dest_hash: announced.dest_hash.0,
                identity_hash: announced.identity_hash.0,
                node_name,
                hops: announced.hops,
                last_seen: announced.received_at,
            });

            if self.entries.len() > MAX_DIRECTORY_ENTRIES {
                self.entries
                    .sort_by(|a, b| a.last_seen.partial_cmp(&b.last_seen).unwrap());
                self.entries.truncate(MAX_DIRECTORY_ENTRIES);
            }
        }
    }

    pub fn known_nodes(&self) -> &[DirectoryEntry] {
        &self.entries
    }

    pub fn get_node(&self, dest_hash: &[u8; 16]) -> Option<&DirectoryEntry> {
        self.entries.iter().find(|e| &e.dest_hash == dest_hash)
    }

    pub fn get_node_by_identity(&self, identity_hash: &[u8; 16]) -> Option<&DirectoryEntry> {
        self.entries
            .iter()
            .find(|e| &e.identity_hash == identity_hash)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for NomadDirectory {
    fn default() -> Self {
        Self::new()
    }
}

pub fn is_nomadnet_announce(announced: &AnnouncedIdentity) -> bool {
    let expected_dest =
        destination_hash("nomadnetwork", &["node"], Some(&announced.identity_hash.0));
    announced.dest_hash.0 == expected_dest
}

pub fn associated_lxmf_dest_hash(identity_hash: &[u8; 16]) -> [u8; 16] {
    destination_hash("lxmf", &["delivery"], Some(identity_hash))
}

#[cfg(test)]
mod tests {
    use rns_net::{DestHash, IdentityHash, InterfaceId};

    use super::*;

    fn make_announced(identity_hash: [u8; 16], app_data: Option<Vec<u8>>) -> AnnouncedIdentity {
        let dest = destination_hash("nomadnetwork", &["node"], Some(&identity_hash));
        AnnouncedIdentity {
            dest_hash: DestHash(dest),
            identity_hash: IdentityHash(identity_hash),
            public_key: [0u8; 64],
            app_data,
            hops: 3,
            received_at: 1000.0,
            receiving_interface: InterfaceId(0),
        }
    }

    #[test]
    fn test_nomadnet_announce_accepted() {
        let mut dir = NomadDirectory::new();
        let identity = [0xAA; 16];
        let announced = make_announced(identity, Some(b"TestNode".to_vec()));
        dir.handle_announce(&announced);
        assert_eq!(dir.len(), 1);
        assert_eq!(dir.known_nodes()[0].node_name.as_deref(), Some("TestNode"));
        assert_eq!(dir.known_nodes()[0].hops, 3);
    }

    #[test]
    fn test_non_nomadnet_announce_rejected() {
        let mut dir = NomadDirectory::new();
        let identity = [0xBB; 16];
        let lxmf_dest = destination_hash("lxmf", &["delivery"], Some(&identity));
        let announced = AnnouncedIdentity {
            dest_hash: DestHash(lxmf_dest),
            identity_hash: IdentityHash(identity),
            public_key: [0u8; 64],
            app_data: Some(b"NotANode".to_vec()),
            hops: 1,
            received_at: 1000.0,
            receiving_interface: InterfaceId(0),
        };
        dir.handle_announce(&announced);
        assert_eq!(dir.len(), 0);
    }

    #[test]
    fn test_announce_updates_existing() {
        let mut dir = NomadDirectory::new();
        let identity = [0xCC; 16];

        dir.handle_announce(&make_announced(identity, Some(b"NodeV1".to_vec())));
        assert_eq!(dir.known_nodes()[0].node_name.as_deref(), Some("NodeV1"));

        dir.handle_announce(&make_announced(identity, Some(b"NodeV2".to_vec())));
        assert_eq!(dir.len(), 1);
        assert_eq!(dir.known_nodes()[0].node_name.as_deref(), Some("NodeV2"));
    }

    #[test]
    fn test_max_entries_eviction() {
        let mut dir = NomadDirectory::new();
        for i in 0..=MAX_DIRECTORY_ENTRIES {
            let mut identity = [0u8; 16];
            identity[0..2].copy_from_slice(&(i as u16).to_le_bytes());
            let announced = make_announced(identity, Some(format!("Node{i}").into_bytes()));
            dir.handle_announce(&announced);
        }
        assert!(dir.len() <= MAX_DIRECTORY_ENTRIES);
    }

    #[test]
    fn test_get_node() {
        let mut dir = NomadDirectory::new();
        let identity = [0xDD; 16];
        let dest = destination_hash("nomadnetwork", &["node"], Some(&identity));
        dir.handle_announce(&make_announced(identity, Some(b"Target".to_vec())));
        assert!(dir.get_node(&dest).is_some());
        assert_eq!(
            dir.get_node(&dest).unwrap().node_name.as_deref(),
            Some("Target")
        );
    }

    #[test]
    fn test_get_node_by_identity() {
        let mut dir = NomadDirectory::new();
        let identity = [0xEE; 16];
        dir.handle_announce(&make_announced(identity, Some(b"ByIdentity".to_vec())));
        assert!(dir.get_node_by_identity(&identity).is_some());
        assert_eq!(
            dir.get_node_by_identity(&identity)
                .unwrap()
                .node_name
                .as_deref(),
            Some("ByIdentity")
        );
    }

    #[test]
    fn test_is_nomadnet_announce() {
        let identity = [0xFF; 16];
        let nn_dest = destination_hash("nomadnetwork", &["node"], Some(&identity));
        let lxmf_dest = destination_hash("lxmf", &["delivery"], Some(&identity));

        let nn_announce = AnnouncedIdentity {
            dest_hash: DestHash(nn_dest),
            identity_hash: IdentityHash(identity),
            public_key: [0u8; 64],
            app_data: None,
            hops: 1,
            received_at: 0.0,
            receiving_interface: InterfaceId(0),
        };
        assert!(is_nomadnet_announce(&nn_announce));

        let lxmf_announce = AnnouncedIdentity {
            dest_hash: DestHash(lxmf_dest),
            identity_hash: IdentityHash(identity),
            public_key: [0u8; 64],
            app_data: None,
            hops: 1,
            received_at: 0.0,
            receiving_interface: InterfaceId(0),
        };
        assert!(!is_nomadnet_announce(&lxmf_announce));
    }

    #[test]
    fn test_associated_lxmf_dest_hash() {
        let identity = [0x42; 16];
        let expected = destination_hash("lxmf", &["delivery"], Some(&identity));
        assert_eq!(associated_lxmf_dest_hash(&identity), expected);
    }
}
