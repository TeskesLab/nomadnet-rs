use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rns_net::{LinkId, RnsNode};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tracing::debug;

use crate::types::{BrowseEvent, NomadError};

struct PendingRequest {
    #[allow(dead_code)]
    dest_hash: [u8; 16],
    path: String,
}

/// Fetches pages from remote NomadNet nodes via RNS Link request/response.
///
/// Maintains link state and emits [`BrowseEvent`]s through a channel. Use
/// [`NomadBrowser::fetch`] to request a page, then listen on the events channel
/// for the response.
pub struct NomadBrowser {
    pending: Arc<Mutex<HashMap<[u8; 16], PendingRequest>>>,
    link_to_dest: Arc<Mutex<HashMap<[u8; 16], [u8; 16]>>>,
    dest_to_link: Arc<Mutex<HashMap<[u8; 16], [u8; 16]>>>,
    event_tx: Sender<BrowseEvent>,
    event_rx: Arc<Mutex<Option<Receiver<BrowseEvent>>>>,
}

impl NomadBrowser {
    pub fn new() -> Self {
        let (event_tx, event_rx) = channel(64);
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            link_to_dest: Arc::new(Mutex::new(HashMap::new())),
            dest_to_link: Arc::new(Mutex::new(HashMap::new())),
            event_tx,
            event_rx: Arc::new(Mutex::new(Some(event_rx))),
        }
    }

    pub fn events(&self) -> Receiver<BrowseEvent> {
        let mut guard = self.event_rx.lock().unwrap();
        guard.take().expect("events() called more than once")
    }

    pub fn handle_link_established(&self, link_id: LinkId, dest_hash: [u8; 16]) {
        debug!(
            "NomadBrowser: link established link_id={} dest={}",
            link_id,
            hex::encode(dest_hash)
        );

        {
            let mut link_to_dest = self.link_to_dest.lock().unwrap();
            link_to_dest.insert(link_id.0, dest_hash);
        }
        {
            let mut dest_to_link = self.dest_to_link.lock().unwrap();
            dest_to_link.insert(dest_hash, link_id.0);
        }

        let event = BrowseEvent::LinkEstablished {
            dest_hash,
            link_id: link_id.0,
        };
        let _ = self.event_tx.try_send(event);
    }

    pub fn handle_response(&self, link_id: LinkId, _request_id: [u8; 16], data: Vec<u8>) {
        let dest_hash = {
            let link_to_dest = self.link_to_dest.lock().unwrap();
            link_to_dest.get(&link_id.0).copied()
        };

        let dest_hash = match dest_hash {
            Some(dh) => dh,
            None => {
                debug!(
                    "NomadBrowser: received response on unknown link_id={}",
                    link_id
                );
                return;
            }
        };

        let path = {
            let pending = self.pending.lock().unwrap();
            pending.get(&link_id.0).map(|p| p.path.clone())
        };

        let path = match path {
            Some(p) => p,
            None => {
                debug!(
                    "NomadBrowser: received response for link_id={} but no pending request",
                    link_id
                );
                return;
            }
        };

        debug!(
            "NomadBrowser: page received dest={} path={} size={}",
            hex::encode(dest_hash),
            path,
            data.len()
        );

        {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(&link_id.0);
        }

        let event = BrowseEvent::PageReceived {
            dest_hash,
            path,
            content: data,
        };
        let _ = self.event_tx.try_send(event);
    }

    pub fn handle_link_closed(&self, link_id: LinkId, reason: Option<String>) {
        let dest_hash = {
            let link_to_dest = self.link_to_dest.lock().unwrap();
            link_to_dest.get(&link_id.0).copied()
        };

        {
            let mut link_to_dest = self.link_to_dest.lock().unwrap();
            link_to_dest.remove(&link_id.0);
        }

        if let Some(dest_hash) = dest_hash {
            {
                let mut dest_to_link = self.dest_to_link.lock().unwrap();
                dest_to_link.remove(&dest_hash);
            }

            let _ = self.event_tx.try_send(BrowseEvent::LinkClosed {
                dest_hash,
                link_id: link_id.0,
                reason,
            });
        }
    }

    pub fn fetch(
        &self,
        node: &Arc<RnsNode>,
        dest_hash: [u8; 16],
        sig_pub_bytes: [u8; 32],
        path: &str,
    ) -> Result<(), NomadError> {
        {
            let dest_to_link = self.dest_to_link.lock().unwrap();
            if let Some(link_id) = dest_to_link.get(&dest_hash) {
                let request = PendingRequest {
                    dest_hash,
                    path: path.to_string(),
                };
                {
                    let mut pending = self.pending.lock().unwrap();
                    pending.insert(*link_id, request);
                }
                return node
                    .send_request(*link_id, path, &[])
                    .map_err(NomadError::from);
            }
        }

        let link_id = node.create_link(dest_hash, sig_pub_bytes)?;
        let request = PendingRequest {
            dest_hash,
            path: path.to_string(),
        };
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(link_id, request);
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn has_active_link(&self, dest_hash: &[u8; 16]) -> bool {
        let dest_to_link = self.dest_to_link.lock().unwrap();
        dest_to_link.contains_key(dest_hash)
    }
}

impl Default for NomadBrowser {
    fn default() -> Self {
        Self::new()
    }
}
