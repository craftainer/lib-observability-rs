//! The Umami adapter: validated events posted to `{endpoint}/api/batch` from
//! a bounded queue drained by one background thread.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy;
use serde_json::{Value, json};

use super::Event;

/// The most events in one request (Umami's batch limit).
pub(crate) const BATCH_SIZE: usize = 50;
/// The most events waiting for delivery; one more is dropped.
pub(crate) const QUEUE_CAPACITY: usize = 1000;
/// How often the queue is drained.
pub(crate) const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// The request timeout.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(2);
/// Umami silently drops events from bot-like user agents.
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (compatible; craftainer-rs)";
/// The constant `payload.ip` of a backend event: never a client address.
const SERVER_IP: &str = "server";

/// What the sender needs; also the test seam for the timings.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub(crate) url: String,
    pub(crate) site_id: String,
    pub(crate) hostname: String,
    pub(crate) timeout: Duration,
    pub(crate) user_agent: String,
    pub(crate) flush_interval: Duration,
}

impl Settings {
    /// The settings for `endpoint` (a base URL), or `None` when it is not a
    /// valid http(s) URL.
    pub(crate) fn new(endpoint: &str, site_id: &str, hostname: &str) -> Option<Self> {
        let url = format!("{}/api/batch", endpoint.trim_end_matches('/'));
        let valid = reqwest::Url::parse(&url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        valid.then(|| Self {
            url,
            site_id: site_id.to_owned(),
            hostname: hostname.to_owned(),
            timeout: TIMEOUT,
            user_agent: USER_AGENT.to_owned(),
            flush_interval: FLUSH_INTERVAL,
        })
    }
}

/// Posts batches. Built on the flusher thread: the blocking client runs its
/// own runtime and must not be created or dropped inside an async context.
pub(crate) struct Sender {
    client: Client,
    settings: Settings,
}

impl Sender {
    pub(crate) fn new(settings: Settings) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .timeout(settings.timeout)
            .user_agent(settings.user_agent.as_str())
            .redirect(Policy::none())
            .build()?;
        Ok(Self { client, settings })
    }

    fn item(&self, event: &Event) -> Value {
        let mut payload = json!({
            "website": self.settings.site_id,
            "url": event.url,
            "name": event.name,
            "ip": SERVER_IP,
            "timestamp": event.timestamp,
        });
        if !self.settings.hostname.is_empty() {
            payload["hostname"] = json!(self.settings.hostname);
        }
        if !event.props.is_empty() {
            payload["data"] = json!(event.props);
        }
        json!({ "type": "event", "payload": payload })
    }

    /// Post `events` in requests of at most 50. A rejected batch is logged
    /// (counts and status only, never event data) and the next one is tried;
    /// a transport failure (connect, timeout) stops here and drops the rest,
    /// rather than wait out the timeout once per batch.
    pub(crate) fn send(&self, events: &[Event]) -> Result<(), reqwest::Error> {
        for chunk in events.chunks(BATCH_SIZE) {
            let items: Vec<Value> = chunk.iter().map(|e| self.item(e)).collect();
            let response = self
                .client
                .post(&self.settings.url)
                .header(CONTENT_TYPE, "application/json")
                .body(Value::Array(items).to_string())
                .send()?;
            check(response, chunk.len());
        }
        Ok(())
    }
}

/// Log what Umami answered; nothing is retried.
fn check(response: reqwest::blocking::Response, sent: usize) {
    let status = response.status();
    let code = status.as_u16();
    if status.is_redirection() {
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        tracing::warn!(
            status = code,
            location,
            "umami batch dropped: redirect; ANALYTICS_ENDPOINT must be the final URL"
        );
    } else if code != 200 {
        tracing::warn!(status = code, "umami batch rejected");
    } else {
        let body = response
            .bytes()
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let errors = match body.as_ref().map(|b| &b["errors"]) {
            None => {
                tracing::warn!("umami batch reply was not JSON");
                0
            }
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
            Some(Value::Array(a)) => u64::try_from(a.len()).unwrap_or(0),
            Some(_) => 0,
        };
        if errors > 0 {
            tracing::warn!(errors, sent, "umami batch reported errors");
        }
    }
}

struct State {
    events: VecDeque<Event>,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The bounded queue and its flusher thread, started on the first event.
pub(crate) struct Queue {
    shared: Arc<Shared>,
    settings: Settings,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Queue {
    pub(crate) fn new(settings: Settings) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    events: VecDeque::new(),
                    stop: false,
                }),
                wake: Condvar::new(),
            }),
            settings,
            thread: Mutex::new(None),
        }
    }

    /// Queue `event`; dropped when the queue is full or after `shutdown`.
    /// Never blocks beyond a short lock and needs no async runtime.
    pub(crate) fn push(&self, event: Event) {
        {
            let mut state = lock(&self.shared);
            if state.stop || state.events.len() >= QUEUE_CAPACITY {
                tracing::debug!("analytics queue full or stopped; event dropped");
                return;
            }
            state.events.push_back(event);
        }
        self.ensure_thread();
    }

    fn ensure_thread(&self) {
        let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        if thread.is_none() {
            let shared = Arc::clone(&self.shared);
            let settings = self.settings.clone();
            *thread = std::thread::Builder::new()
                .name("analytics-flush".to_owned())
                .spawn(move || run(&shared, settings))
                .ok();
        }
    }

    /// Stop the flusher after a final flush; blocks until it is done (at most
    /// one timeout per failed batch). Calling it again is harmless.
    pub(crate) fn shutdown(&self) {
        lock(&self.shared).stop = true;
        self.shared.wake.notify_all();
        let handle = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

/// The flusher: every interval (or at once on stop) drain the queue and send.
fn run(shared: &Shared, settings: Settings) {
    let interval = settings.flush_interval;
    let sender = Sender::new(settings)
        .inspect_err(|_| tracing::warn!("umami HTTP client could not be built; events dropped"))
        .ok();
    loop {
        let (batch, stop) = {
            let (mut state, _) = shared
                .wake
                .wait_timeout_while(lock(shared), interval, |s| !s.stop)
                .unwrap_or_else(PoisonError::into_inner);
            (Vec::from(std::mem::take(&mut state.events)), state.stop)
        };
        if let Some(sender) = &sender
            && let Err(err) = sender.send(&batch)
        {
            let kind = if err.is_timeout() {
                "timeout"
            } else {
                "transport"
            };
            tracing::warn!(kind, "umami delivery failed; remaining batches dropped");
        }
        if stop {
            return;
        }
    }
}
