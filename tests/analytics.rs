//! The analytics module through its public API, in a process of its own: it
//! installs a global `tracing` subscriber to read the log lines (including
//! the ones the Umami flusher thread writes) and uses the process-wide
//! client. One test function, for the same reason.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craftainer_observability::analytics::{
    self, AnalyticsClient, AnalyticsConfig, Backend, PropValue, RequestContext,
};
use serde_json::json;
use tracing_subscriber::fmt::MakeWriter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl MakeWriter<'_> for Capture {
    type Writer = Capture;

    fn make_writer(&self) -> Capture {
        self.clone()
    }
}

impl Capture {
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}

fn umami(uri: &str) -> AnalyticsConfig {
    AnalyticsConfig {
        backend: Backend::Umami,
        endpoint: uri.to_owned(),
        site_id: "site".into(),
        prop_keys: ["plan".to_owned()].into(),
        ..AnalyticsConfig::default()
    }
}

/// Track one event through a fresh client against a server answering `reply`
/// and return what the process logged meanwhile.
fn deliver(
    rt: &tokio::runtime::Runtime,
    logs: &Capture,
    reply: ResponseTemplate,
) -> (usize, String) {
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/batch"))
            .respond_with(reply)
            .mount(&server)
            .await;
        server
    });
    let client = AnalyticsClient::new(umami(&server.uri()));
    client.track("page.view", &[("plan", "SECRET-VALUE".into())], Some("/p"));
    client.shutdown();
    let seen = rt
        .block_on(server.received_requests())
        .unwrap_or_default()
        .len();
    (seen, logs.take())
}

/// What each kind of Umami reply logs.
fn reply_cases(rt: &tokio::runtime::Runtime, logs: &Capture) {
    // A clean 200 logs nothing at warn.
    let (seen, out) = deliver(
        rt,
        logs,
        ResponseTemplate::new(200).set_body_json(json!({"errors": 0})),
    );
    assert_eq!(seen, 1);
    assert!(!out.contains("WARN"), "{out}");
    // `errors` as a count or a list, a reply that is not JSON, other statuses.
    let (_, out) = deliver(
        rt,
        logs,
        ResponseTemplate::new(200).set_body_json(json!({"errors": 2})),
    );
    assert!(
        out.contains("reported errors") && out.contains("errors=2"),
        "{out}"
    );
    let (_, out) = deliver(
        rt,
        logs,
        ResponseTemplate::new(200).set_body_json(json!({"errors": ["a"]})),
    );
    assert!(out.contains("errors=1"), "{out}");
    let (_, out) = deliver(
        rt,
        logs,
        ResponseTemplate::new(200).set_body_json(json!({"ok": true})),
    );
    assert!(!out.contains("WARN"), "{out}");
    let (_, out) = deliver(
        rt,
        logs,
        ResponseTemplate::new(200).set_body_string("<html>"),
    );
    assert!(out.contains("not JSON"), "{out}");
    let (_, out) = deliver(rt, logs, ResponseTemplate::new(400));
    assert!(
        out.contains("rejected") && out.contains("status=400"),
        "{out}"
    );
    // A redirect is not followed: one request, the status and Location logged.
    let (seen, out) = deliver(
        rt,
        logs,
        ResponseTemplate::new(301).insert_header("Location", "https://elsewhere.example/api/batch"),
    );
    assert_eq!(seen, 1);
    assert!(out.contains("status=301"), "{out}");
    assert!(out.contains("https://elsewhere.example/api/batch"), "{out}");
    assert!(out.contains("final URL"), "{out}");
    // A redirect without a Location header.
    let (_, out) = deliver(rt, logs, ResponseTemplate::new(302));
    assert!(out.contains("status=302"), "{out}");
    // Never a property value in any of it.
    assert!(!out.contains("SECRET-VALUE"), "{out}");
}

#[test]
fn analytics_end_to_end() {
    let logs = Capture::default();
    tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::DEBUG)
        .init();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Before any init the environment decides: nothing set means `none`.
    analytics::track("page.view", &[], Some("/"));
    analytics::shutdown();

    // The `log` adapter: fixed fields, the props as one JSON string, and an
    // invalid name logged without any value.
    analytics::init(
        &AnalyticsConfig {
            backend: Backend::Log,
            prop_keys: ["plan".to_owned()].into(),
            ..AnalyticsConfig::default()
        },
        None,
    );
    analytics::track(
        "signup.done",
        &[("plan", "pro".into()), ("hidden", "LEAK".into())],
        Some("/signup?token=abc#x"),
    );
    analytics::track("bad name", &[("plan", "LEAK2".into())], None);
    let guard = RequestContext {
        gpc: true,
        consent: false,
    }
    .enter();
    analytics::track("gpc.skipped", &[], None);
    drop(guard);
    let out = logs.take();
    assert!(
        out.contains("craftainer_observability::analytics::events"),
        "{out}"
    );
    assert!(out.contains("analytics_event=signup.done"), "{out}");
    assert!(out.contains("analytics_url=/signup"), "{out}");
    assert!(out.contains(r#"analytics_props={"plan":"pro"}"#), "{out}");
    assert!(
        !out.contains("abc") && !out.contains("LEAK") && !out.contains("gpc.skipped"),
        "{out}"
    );
    assert!(out.contains("invalid name"), "{out}");

    // Re-init flushes the old client; the otel backend without telemetry
    // warns and is off.
    analytics::init(
        &AnalyticsConfig {
            backend: Backend::Otel,
            ..AnalyticsConfig::default()
        },
        None,
    );
    assert!(logs.take().contains("otel"));
    drop(logs.take());

    // Umami replies: the process-wide client delivers on shutdown.
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/batch"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"errors": 0})))
            .mount(&server)
            .await;
        server
    });
    analytics::init(&umami(&server.uri()), None);
    analytics::track("page.view", &[("plan", PropValue::from("pro"))], Some("/x"));
    analytics::shutdown();
    assert_eq!(rt.block_on(server.received_requests()).unwrap().len(), 1);
    drop(logs.take());

    reply_cases(&rt, &logs);

    // A timeout (the real 2 s one) is a transport failure.
    let (seen, out) = deliver(
        &rt,
        &logs,
        ResponseTemplate::new(200).set_delay(Duration::from_secs(3)),
    );
    assert_eq!(seen, 1);
    assert!(
        out.contains("delivery failed") && out.contains("timeout"),
        "{out}"
    );
    // A connection failure is the other kind.
    // A port nobody listens on: bound, noted, and released at once.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_uri = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let client = AnalyticsClient::new(umami(&dead_uri));
    client.track("page.view", &[], None);
    client.shutdown();
    assert!(logs.take().contains("transport"));

    // The default 5 s interval delivers without a shutdown.
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"errors": 0})))
            .mount(&server)
            .await;
        server
    });
    let client = AnalyticsClient::new(umami(&server.uri()));
    client.track("page.view", &[], None);
    std::thread::sleep(Duration::from_secs(7));
    assert_eq!(rt.block_on(server.received_requests()).unwrap().len(), 1);
    client.shutdown();
}
