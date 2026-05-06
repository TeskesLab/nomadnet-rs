use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use rns_net::{LinkId, RnsNode};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tracing::{debug, warn};

use crate::types::{BrowseEvent, NomadError};

struct PendingRequest {
    path: String,
    data: Vec<u8>,
}

/// Parsed result of a URL containing backtick-delimited field data.
///
/// NomadNet URLs can embed form fields using the format
/// `url\`field1=val1|field2=val2`. Fields are converted to
/// `var_field1=val1` key-value pairs in the request data.
pub struct ParsedUrlFields {
    pub url: String,
    pub fields: Vec<(String, String)>,
}

/// Parse backtick-delimited fields from a NomadNet URL.
///
/// # Examples
///
/// ```
/// use nomadnet_rs::browser::parse_url_fields;
///
/// let result = parse_url_fields("abc123:/page/index.mu`name=Alice|age=30");
/// assert_eq!(result.url, "abc123:/page/index.mu");
/// assert_eq!(result.fields, vec![
///     ("var_name".into(), "Alice".into()),
///     ("var_age".into(), "30".into()),
/// ]);
/// ```
pub fn parse_url_fields(url: &str) -> ParsedUrlFields {
    match url.split_once('`') {
        Some((base, fields_str)) => {
            let fields = fields_str
                .split('|')
                .filter_map(|entry| {
                    let (k, v) = entry.split_once('=')?;
                    Some((format!("var_{k}"), v.to_string()))
                })
                .collect();
            ParsedUrlFields {
                url: base.to_string(),
                fields,
            }
        }
        None => ParsedUrlFields {
            url: url.to_string(),
            fields: Vec::new(),
        },
    }
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

    fn emit_event(&self, event: BrowseEvent, name: &str) {
        match self.event_tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                warn!("NomadBrowser: dropping {name} event because event queue is full")
            }
            Err(TrySendError::Closed(_)) => {
                warn!("NomadBrowser: dropping {name} event because event queue is closed")
            }
        }
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
        self.emit_event(event, "LinkEstablished");

        let queued: Vec<(String, Vec<u8>)> = {
            let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending
                .get(&link_id.0)
                .map(|q| {
                    q.iter()
                        .map(|req| (req.path.clone(), req.data.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        if queued.is_empty() {
            return Ok(());
        }

        let Some(node) = node else {
            warn!(
                "NomadBrowser: {} queued request(s) for link {} dropped — no node provided",
                queued.len(),
                link_id
            );
            return Ok(());
        };

        for (path, data) in queued {
            node.send_request(link_id.0, &path, &data)
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
            "NomadBrowser: response received dest={} path={} size={}",
            hex::encode(dest_hash),
            path,
            data.len()
        );

        let is_file = path.starts_with("/file/");
        let event = if is_file {
            BrowseEvent::FileReceived {
                dest_hash,
                path,
                content: data,
            }
        } else {
            BrowseEvent::PageReceived {
                dest_hash,
                path,
                content: data,
            }
        };
        self.emit_event(
            event,
            if is_file {
                "FileReceived"
            } else {
                "PageReceived"
            },
        );
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

            self.emit_event(
                BrowseEvent::LinkClosed {
                    dest_hash,
                    link_id: link_id.0,
                    reason,
                },
                "LinkClosed",
            );
        }
    }

    pub fn fetch(
        &self,
        node: &Arc<RnsNode>,
        dest_hash: [u8; 16],
        sig_pub_bytes: [u8; 32],
        path: &str,
    ) -> Result<(), NomadError> {
        self.fetch_with_data(node, dest_hash, sig_pub_bytes, path, &[])
    }

    /// Fetch a page from a remote node with custom request data.
    ///
    /// If a link to `dest_hash` already exists the request is sent immediately;
    /// otherwise a new link is created and the request is queued until the link
    /// is established.
    ///
    /// The request `data` is sent as the RNS Link request payload.  Use
    /// [`fetch_file`] for `/file/*` paths.
    pub fn fetch_with_data(
        &self,
        node: &Arc<RnsNode>,
        dest_hash: [u8; 16],
        sig_pub_bytes: [u8; 32],
        path: &str,
        data: &[u8],
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
                            data: data.to_vec(),
                        });
                }
                return node
                    .send_request(*link_id, path, data)
                    .map_err(NomadError::from);
            }
        }

        let link_id = match node.create_link(dest_hash, sig_pub_bytes) {
            Ok(link_id) => link_id,
            Err(err) => {
                self.emit_event(
                    BrowseEvent::LinkFailed {
                        dest_hash,
                        error: format!("{err:?}"),
                    },
                    "LinkFailed",
                );
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
                    data: data.to_vec(),
                });
        }

        Ok(())
    }

    /// Fetch a file from a remote node.
    ///
    /// Convenience wrapper around [`fetch_with_data`] for `/file/*` paths.
    /// If `data` is `None`, any embedded field data in the path URL is parsed
    /// using [`parse_url_fields`] and sent as request data.
    pub fn fetch_file(
        &self,
        node: &Arc<RnsNode>,
        dest_hash: [u8; 16],
        sig_pub_bytes: [u8; 32],
        path: &str,
        data: Option<&[u8]>,
    ) -> Result<(), NomadError> {
        let request_data = match data {
            Some(d) => d.to_vec(),
            None => {
                let parsed = parse_url_fields(path);
                if parsed.fields.is_empty() {
                    Vec::new()
                } else {
                    parsed
                        .fields
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("\0")
                        .into_bytes()
                }
            }
        };
        self.fetch_with_data(node, dest_hash, sig_pub_bytes, path, &request_data)
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
        browser
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                link_id,
                VecDeque::from([
                    PendingRequest {
                        path: "/page/first.mu".to_string(),
                        data: Vec::new(),
                    },
                    PendingRequest {
                        path: "/page/second.mu".to_string(),
                        data: Vec::new(),
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
        browser
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                link_id,
                VecDeque::from([PendingRequest {
                    path: "/page/index.mu".to_string(),
                    data: Vec::new(),
                }]),
            );

        browser.handle_link_closed(LinkId(link_id), Some("test".to_string()));

        assert!(browser
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&link_id)
            .is_none());
        assert!(browser
            .link_to_dest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&link_id)
            .is_none());
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

        assert!(!browser
            .dest_to_link
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&dest_hash));

        browser
            .dest_to_link
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(dest_hash, link_id);

        assert!(browser
            .dest_to_link
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&dest_hash));
    }

    #[test]
    fn handle_link_established_records_mapping() {
        let browser = NomadBrowser::new();
        let link_id = LinkId([0x11; 16]);
        let dest_hash = [0x22; 16];

        browser.handle_link_established(link_id, dest_hash);

        assert!(browser
            .dest_to_link
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&dest_hash));
        let link_to_dest = browser
            .link_to_dest
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

    #[test]
    fn event_queue_is_bounded_and_overflow_drops_new_events() {
        let browser = NomadBrowser::new();
        let mut events = browser.events();

        for i in 0..65u8 {
            let link_id = LinkId([i; 16]);
            let dest_hash = [i; 16];
            browser.handle_link_established(link_id, dest_hash);
        }

        let mut count = 0usize;
        while events.try_recv().is_ok() {
            count += 1;
        }

        assert_eq!(count, 64);
    }

    #[test]
    fn parse_url_fields_no_fields() {
        let result = parse_url_fields("abc123:/page/index.mu");
        assert_eq!(result.url, "abc123:/page/index.mu");
        assert!(result.fields.is_empty());
    }

    #[test]
    fn parse_url_fields_with_key_values() {
        let result = parse_url_fields("abc123:/page/index.mu`name=Alice|age=30");
        assert_eq!(result.url, "abc123:/page/index.mu");
        assert_eq!(
            result.fields,
            vec![
                ("var_name".into(), "Alice".into()),
                ("var_age".into(), "30".into()),
            ]
        );
    }

    #[test]
    fn parse_url_fields_ignores_entries_without_equals() {
        let result = parse_url_fields("abc123:/page/index.mu`invalid|name=Bob");
        assert_eq!(result.url, "abc123:/page/index.mu");
        assert_eq!(result.fields, vec![("var_name".into(), "Bob".into())]);
    }

    #[test]
    fn file_path_emits_file_received() {
        let browser = NomadBrowser::new();
        let mut events = browser.events();
        let link_id = LinkId([0x11; 16]);
        let dest_hash = [0x22; 16];

        browser
            .link_to_dest
            .lock()
            .unwrap()
            .insert(link_id.0, dest_hash);
        browser
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                link_id.0,
                VecDeque::from([PendingRequest {
                    path: "/file/readme.txt".to_string(),
                    data: Vec::new(),
                }]),
            );

        browser.handle_response(link_id, [0u8; 16], b"file content".to_vec());

        let event = events.try_recv().expect("expected event");
        match event {
            BrowseEvent::FileReceived { path, content, .. } => {
                assert_eq!(path, "/file/readme.txt");
                assert_eq!(content, b"file content");
            }
            _ => panic!("expected FileReceived event, got {event:?}"),
        }
    }

    #[test]
    fn page_path_emits_page_received() {
        let browser = NomadBrowser::new();
        let mut events = browser.events();
        let link_id = LinkId([0x11; 16]);
        let dest_hash = [0x22; 16];

        browser
            .link_to_dest
            .lock()
            .unwrap()
            .insert(link_id.0, dest_hash);
        browser
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                link_id.0,
                VecDeque::from([PendingRequest {
                    path: "/page/index.mu".to_string(),
                    data: Vec::new(),
                }]),
            );

        browser.handle_response(link_id, [0u8; 16], b"page content".to_vec());

        let event = events.try_recv().expect("expected event");
        match event {
            BrowseEvent::PageReceived { path, .. } => {
                assert_eq!(path, "/page/index.mu");
            }
            _ => panic!("expected PageReceived event, got {event:?}"),
        }
    }
}
