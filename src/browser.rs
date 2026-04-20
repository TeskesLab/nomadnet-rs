use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use rns_net::{LinkId, RnsNode};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tracing::{debug, warn};

use crate::types::{BrowseEvent, NomadError};

struct PendingRequest {
    path: String,
}

/// Fetches pages from remote NomadNet nodes via RNS Link request/response.
///
/// Maintains link state and emits [`BrowseEvent`]s through a channel. Use
/// [`NomadBrowser::fetch`] to request a page, then listen on the events channel
/// for the response.
pub struct NomadBrowser {
    pending: Arc<Mutex<HashMap<[u8; 16], VecDeque<PendingRequest>>>>,
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
        let mut guard = self.event_rx.lock().unwrap_or_else(|e| e.into_inner());
        guard.take().expect("events() called more than once")
    }

    pub fn handle_link_established(&self, link_id: LinkId, dest_hash: [u8; 16]) {
        let _ = self.handle_link_established_with_node(None, link_id, dest_hash);
    }

    pub fn handle_link_established_with_node(
        &self,
        node: Option<&Arc<RnsNode>>,
        link_id: LinkId,
        dest_hash: [u8; 16],
    ) -> Result<(), NomadError> {
        debug!(
            "NomadBrowser: link established link_id={} dest={}",
            link_id,
            hex::encode(dest_hash)
        );

        {
            let mut link_to_dest = self.link_to_dest.lock().unwrap_or_else(|e| e.into_inner());
            link_to_dest.insert(link_id.0, dest_hash);
        }
        {
            let mut dest_to_link = self.dest_to_link.lock().unwrap_or_else(|e| e.into_inner());
            dest_to_link.insert(dest_hash, link_id.0);
        }

        let event = BrowseEvent::LinkEstablished {
            dest_hash,
            link_id: link_id.0,
        };
        let _ = self.event_tx.try_send(event);

        let queued_paths = {
            let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending
                .get(&link_id.0)
                .map(|q| q.iter().map(|req| req.path.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        };

        if queued_paths.is_empty() {
            return Ok(());
        }

        let Some(node) = node else {
            warn!(
                "NomadBrowser: {} queued request(s) for link {} dropped — no node provided",
                queued_paths.len(),
                link_id
            );
            return Ok(());
        };

        for path in queued_paths {
            node.send_request(link_id.0, &path, &[])
                .map_err(NomadError::from)?;
        }

        Ok(())
    }

    pub fn handle_response(&self, link_id: LinkId, _request_id: [u8; 16], data: Vec<u8>) {
        let dest_hash = {
            let link_to_dest = self.link_to_dest.lock().unwrap_or_else(|e| e.into_inner());
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
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            let queue = match pending.get_mut(&link_id.0) {
                Some(q) => q,
                None => return,
            };
            let req = match queue.pop_front() {
                Some(r) => r,
                None => return,
            };
            if queue.is_empty() {
                pending.remove(&link_id.0);
            }
            req.path
        };

        debug!(
            "NomadBrowser: page received dest={} path={} size={}",
            hex::encode(dest_hash),
            path,
            data.len()
        );

        let event = BrowseEvent::PageReceived {
            dest_hash,
            path,
            content: data,
        };
        let _ = self.event_tx.try_send(event);
    }

    pub fn handle_link_closed(&self, link_id: LinkId, reason: Option<String>) {
        let dest_hash = {
            let link_to_dest = self.link_to_dest.lock().unwrap_or_else(|e| e.into_inner());
            link_to_dest.get(&link_id.0).copied()
        };

        {
            let mut link_to_dest = self.link_to_dest.lock().unwrap_or_else(|e| e.into_inner());
            link_to_dest.remove(&link_id.0);
        }

        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.remove(&link_id.0);
        }

        if let Some(dest_hash) = dest_hash {
            {
                let mut dest_to_link = self.dest_to_link.lock().unwrap_or_else(|e| e.into_inner());
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
            let dest_to_link = self.dest_to_link.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(link_id) = dest_to_link.get(&dest_hash) {
                {
                    let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
                    pending
                        .entry(*link_id)
                        .or_default()
                        .push_back(PendingRequest {
                            path: path.to_string(),
                        });
                }
                return node
                    .send_request(*link_id, path, &[])
                    .map_err(NomadError::from);
            }
        }

        let link_id = match node.create_link(dest_hash, sig_pub_bytes) {
            Ok(link_id) => link_id,
            Err(err) => {
                let _ = self.event_tx.try_send(BrowseEvent::LinkFailed {
                    dest_hash,
                    error: format!("{err:?}"),
                });
                return Err(NomadError::from(err));
            }
        };

        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending
                .entry(link_id)
                .or_default()
                .push_back(PendingRequest {
                    path: path.to_string(),
                });
        }

        Ok(())
    }

    pub(crate) fn has_active_link(&self, dest_hash: &[u8; 16]) -> bool {
        let dest_to_link = self.dest_to_link.lock().unwrap_or_else(|e| e.into_inner());
        dest_to_link.contains_key(dest_hash)
    }
}

impl Default for NomadBrowser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_consumes_oldest_pending_path() {
        let browser = NomadBrowser::new();
        let mut events = browser.events();

        let link_id = [0x11; 16];
        let dest_hash = [0x22; 16];

        browser
            .link_to_dest
            .lock()
            .unwrap()
            .insert(link_id, dest_hash);
        browser.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(
            link_id,
            VecDeque::from([
                PendingRequest {
                    path: "/page/first.mu".to_string(),
                },
                PendingRequest {
                    path: "/page/second.mu".to_string(),
                },
            ]),
        );

        browser.handle_response(LinkId(link_id), [0u8; 16], b"ok".to_vec());

        let page_event = events.try_recv().expect("expected page event");
        match page_event {
            BrowseEvent::PageReceived { path, .. } => assert_eq!(path, "/page/first.mu"),
            _ => panic!("expected page-received event"),
        }

        let pending = browser.pending.lock().unwrap_or_else(|e| e.into_inner());
        let queue = pending.get(&link_id).expect("queue should still exist");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.front().unwrap().path, "/page/second.mu");
    }

    #[test]
    fn link_close_clears_pending_requests() {
        let browser = NomadBrowser::new();
        let mut events = browser.events();

        let link_id = [0x33; 16];
        let dest_hash = [0x44; 16];

        browser
            .link_to_dest
            .lock()
            .unwrap()
            .insert(link_id, dest_hash);
        browser
            .dest_to_link
            .lock()
            .unwrap()
            .insert(dest_hash, link_id);
        browser.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(
            link_id,
            VecDeque::from([PendingRequest {
                path: "/page/index.mu".to_string(),
            }]),
        );

        browser.handle_link_closed(LinkId(link_id), Some("test".to_string()));

        assert!(browser.pending.lock().unwrap_or_else(|e| e.into_inner()).get(&link_id).is_none());
        assert!(browser.link_to_dest.lock().unwrap_or_else(|e| e.into_inner()).get(&link_id).is_none());
        assert!(browser
            .dest_to_link
            .lock()
            .unwrap()
            .get(&dest_hash)
            .is_none());

        let close_event = events.try_recv().expect("expected link-closed event");
        match close_event {
            BrowseEvent::LinkClosed {
                dest_hash: got_dest,
                ..
            } => assert_eq!(got_dest, dest_hash),
            _ => panic!("expected link-closed event"),
        }
    }

    #[test]
    #[should_panic(expected = "events() called more than once")]
    fn events_called_twice_panics() {
        let browser = NomadBrowser::new();
        let _first = browser.events();
        let _second = browser.events();
    }

    #[test]
    fn has_active_link_reflects_state() {
        let browser = NomadBrowser::new();
        let dest_hash = [0xaa; 16];
        let link_id = [0xbb; 16];

        assert!(!browser.has_active_link(&dest_hash));

        browser
            .dest_to_link
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(dest_hash, link_id);

        assert!(browser.has_active_link(&dest_hash));
    }

    #[test]
    fn handle_link_established_records_mapping() {
        let browser = NomadBrowser::new();
        let link_id = LinkId([0x11; 16]);
        let dest_hash = [0x22; 16];

        browser.handle_link_established(link_id, dest_hash);

        assert!(browser.has_active_link(&dest_hash));
        let link_to_dest = browser.link_to_dest.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(link_to_dest.get(&link_id.0), Some(&dest_hash));
    }

    #[test]
    fn handle_response_ignores_unknown_link() {
        let browser = NomadBrowser::new();
        let _events = browser.events();

        browser.handle_response(LinkId([0xff; 16]), [0u8; 16], vec![1, 2, 3]);

        let pending = browser.pending.lock().unwrap_or_else(|e| e.into_inner());
        assert!(pending.is_empty());
    }
}
