//! Unit tests: the gate, the adapters and the Umami delivery against a mock
//! server. Log lines are asserted in `tests/analytics.rs` (a process of its
//! own with a global subscriber).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::umami::{BATCH_SIZE, QUEUE_CAPACITY, Settings, USER_AGENT};
use super::*;

/// A mock Umami on a runtime of its own, so the code under test runs on
/// plain threads outside any runtime.
struct Lane {
    rt: tokio::runtime::Runtime,
    server: MockServer,
}

impl Lane {
    fn new(reply: ResponseTemplate) -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/api/batch"))
                .respond_with(reply)
                .mount(&server)
                .await;
            server
        });
        Self { rt, server }
    }

    fn ok() -> Self {
        Self::new(ResponseTemplate::new(200).set_body_json(json!({"errors": 0})))
    }

    fn requests(&self) -> Vec<wiremock::Request> {
        self.rt
            .block_on(self.server.received_requests())
            .unwrap_or_default()
    }

    fn config(&self) -> AnalyticsConfig {
        AnalyticsConfig {
            backend: Backend::Umami,
            endpoint: self.server.uri(),
            site_id: "site-1".into(),
            hostname: "app.example".into(),
            prop_keys: keys(&["plan", "n"]),
            ..AnalyticsConfig::default()
        }
    }

    /// Items of every request, in order.
    fn batches(&self) -> Vec<Vec<Value>> {
        self.requests()
            .iter()
            .map(|r| serde_json::from_slice::<Vec<Value>>(&r.body).unwrap())
            .collect()
    }
}

fn keys(list: &[&str]) -> HashSet<String> {
    list.iter().map(|k| (*k).to_owned()).collect()
}

/// A client whose queue is only drained by `shutdown`.
fn manual(config: AnalyticsConfig) -> AnalyticsClient {
    AnalyticsClient::build(
        config,
        None,
        Some(&|s: &mut Settings| s.flush_interval = Duration::from_secs(3600)),
    )
}

#[test]
fn umami_posts_the_contract_payload() {
    let lane = Lane::ok();
    let client = manual(lane.config());
    client.track(
        "page.view",
        &[
            ("plan", "pro".into()),
            ("n", 2.into()),
            ("other", "x".into()),
        ],
        Some("/a/b?q=1#f"),
    );
    client.shutdown();
    let requests = lane.requests();
    assert_eq!(requests.len(), 1);
    let headers = &requests[0].headers;
    assert_eq!(headers.get("user-agent").unwrap(), USER_AGENT);
    assert_eq!(USER_AGENT, "Mozilla/5.0 (compatible; craftainer-rs)");
    assert_eq!(headers.get("content-type").unwrap(), "application/json");
    let batches = lane.batches();
    let item = &batches[0][0];
    assert_eq!(item["type"], "event");
    let payload = &item["payload"];
    assert_eq!(payload["website"], "site-1");
    assert_eq!(payload["url"], "/a/b");
    assert_eq!(payload["name"], "page.view");
    assert_eq!(payload["ip"], "server");
    assert_eq!(payload["hostname"], "app.example");
    assert_eq!(payload["data"], json!({"plan": "pro", "n": 2}));
    assert!(payload["timestamp"].is_u64());
    for absent in ["id", "title", "referrer"] {
        assert!(payload.get(absent).is_none(), "{absent}");
    }
}

#[test]
fn umami_omits_empty_hostname_and_data() {
    let lane = Lane::ok();
    let mut config = lane.config();
    config.hostname.clear();
    let client = manual(config);
    client.track("page.view", &[("unlisted", 1.into())], None);
    client.shutdown();
    let payload = lane.batches()[0][0]["payload"].clone();
    assert_eq!(payload["url"], "/");
    assert!(payload.get("hostname").is_none());
    assert!(payload.get("data").is_none());
}

#[test]
fn the_queue_is_bounded_and_batches_hold_at_most_fifty() {
    let lane = Lane::ok();
    let client = manual(lane.config());
    for _ in 0..QUEUE_CAPACITY + 5 {
        client.track("page.view", &[], Some("/"));
    }
    client.shutdown();
    let sizes: Vec<usize> = lane.batches().iter().map(Vec::len).collect();
    assert_eq!(sizes.iter().sum::<usize>(), QUEUE_CAPACITY);
    assert!(sizes.iter().all(|n| *n <= BATCH_SIZE), "{sizes:?}");
    assert_eq!(sizes.len(), QUEUE_CAPACITY / BATCH_SIZE);
}

#[test]
fn the_flusher_drains_on_its_interval() {
    let lane = Lane::ok();
    let client = AnalyticsClient::build(
        lane.config(),
        None,
        Some(&|s: &mut Settings| s.flush_interval = Duration::from_millis(50)),
    );
    client.track("page.view", &[], None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while lane.requests().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(lane.requests().len(), 1, "flushed without a shutdown");
}

#[test]
fn a_transport_failure_stops_the_flush_and_drops_the_rest() {
    let lane = Lane::new(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)));
    let client = AnalyticsClient::build(
        lane.config(),
        None,
        Some(&|s: &mut Settings| {
            s.flush_interval = Duration::from_secs(3600);
            s.timeout = Duration::from_millis(200);
        }),
    );
    for _ in 0..=(BATCH_SIZE * 2) {
        client.track("page.view", &[], None);
    }
    client.shutdown();
    assert_eq!(lane.requests().len(), 1, "the later batches were not tried");
}

#[test]
fn a_rejected_batch_does_not_stop_the_next_one() {
    let lane = Lane::new(ResponseTemplate::new(500));
    let client = manual(lane.config());
    for _ in 0..=BATCH_SIZE {
        client.track("page.view", &[], None);
    }
    client.shutdown();
    assert_eq!(lane.requests().len(), 2);
}

#[test]
fn an_unbuildable_http_client_drops_events_quietly() {
    let lane = Lane::ok();
    let client = AnalyticsClient::build(
        lane.config(),
        None,
        Some(&|s: &mut Settings| s.user_agent = "bad\nagent".into()),
    );
    client.track("page.view", &[], None);
    client.shutdown();
    assert!(lane.requests().is_empty());
}

#[test]
fn shutdown_is_idempotent_and_later_events_are_dropped() {
    let lane = Lane::ok();
    let client = manual(lane.config());
    client.track("page.view", &[], None);
    client.shutdown();
    client.shutdown();
    client.track("page.view", &[], None);
    drop(client);
    assert_eq!(lane.batches().iter().map(Vec::len).sum::<usize>(), 1);
}

#[test]
fn dropping_the_client_flushes() {
    let lane = Lane::ok();
    drop({
        let client = manual(lane.config());
        client.track("page.view", &[], None);
        client
    });
    assert_eq!(lane.requests().len(), 1);
}

#[test]
fn track_works_outside_any_tokio_runtime_and_inside_one() {
    let lane = Lane::ok();
    let config = lane.config();
    let client = std::thread::spawn(move || {
        let client = manual(config);
        client.track("page.view", &[], None);
        client
    })
    .join()
    .unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        client.track("page.view", &[], None);
    });
    let client = rt.block_on(async {
        tokio::task::spawn_blocking(move || {
            client.shutdown();
            client
        })
        .await
        .unwrap()
    });
    drop(client);
    assert_eq!(lane.batches().iter().map(Vec::len).sum::<usize>(), 2);
}

#[test]
fn bad_umami_settings_fall_back_to_off() {
    for (endpoint, site_id) in [
        ("", "site"),
        ("http://x", ""),
        ("not a url", "site"),
        ("ftp://host", "site"),
    ] {
        let client = AnalyticsClient::new(AnalyticsConfig {
            backend: Backend::Umami,
            endpoint: endpoint.into(),
            site_id: site_id.into(),
            ..AnalyticsConfig::default()
        });
        assert!(matches!(client.sink_name(), "none"), "{endpoint}");
        client.track("page.view", &[], None);
    }
}

#[test]
fn off_unless_enabled_and_a_backend_is_chosen() {
    let lane = Lane::ok();
    let mut config = lane.config();
    config.enabled = false;
    assert_eq!(AnalyticsClient::new(config).sink_name(), "none");
    assert_eq!(
        AnalyticsClient::new(AnalyticsConfig::default()).sink_name(),
        "none"
    );
    assert_eq!(AnalyticsClient::new(lane.config()).sink_name(), "umami");
    let log = AnalyticsConfig {
        backend: Backend::Log,
        ..AnalyticsConfig::default()
    };
    assert_eq!(AnalyticsClient::new(log).sink_name(), "log");
    // The otel backend needs a provider: none here.
    let otel = AnalyticsConfig {
        backend: Backend::Otel,
        ..AnalyticsConfig::default()
    };
    let none = AnalyticsClient::new(otel);
    assert_eq!(none.sink_name(), "none");
    // Tracking on an off client is accepted and goes nowhere.
    none.track("page.view", &[], None);
    none.track("bad name", &[], None);
}

#[test]
fn gpc_and_consent_gate_tracking() {
    let lane = Lane::ok();
    let count = |config: AnalyticsConfig, ctx: Option<RequestContext>| {
        let client = manual(config);
        let guard = ctx.map(RequestContext::enter);
        client.track("page.view", &[], None);
        drop(guard);
        client.shutdown();
        lane.batches().iter().map(Vec::len).sum::<usize>()
    };
    let gpc = Some(RequestContext {
        gpc: true,
        consent: false,
    });
    let plain = Some(RequestContext::default());
    let consent = Some(RequestContext {
        gpc: false,
        consent: true,
    });
    let mut total = 0;
    // Implied consent: only GPC stops it.
    assert_eq!(count(lane.config(), gpc), total);
    total += 1;
    assert_eq!(count(lane.config(), plain), total);
    total += 1;
    assert_eq!(count(lane.config(), None), total);
    let mut ignore_gpc = lane.config();
    ignore_gpc.respect_gpc = false;
    total += 1;
    assert_eq!(count(ignore_gpc, gpc), total);
    // Opt-in: needs the consent flag, and GPC still wins.
    let mut opt_in = lane.config();
    opt_in.consent = Consent::OptIn;
    assert_eq!(count(opt_in.clone(), plain), total);
    assert_eq!(count(opt_in.clone(), None), total);
    total += 1;
    assert_eq!(count(opt_in.clone(), consent), total);
    let both = Some(RequestContext {
        gpc: true,
        consent: true,
    });
    assert_eq!(count(opt_in, both), total);
}

#[test]
fn an_invalid_name_drops_the_event() {
    let lane = Lane::ok();
    let client = manual(lane.config());
    client.track("nodot", &[], None);
    client.track("Bad.Name", &[], None);
    client.shutdown();
    assert!(lane.requests().is_empty());
}

#[cfg(feature = "otel")]
mod otel {
    use opentelemetry::logs::AnyValue;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider, SimpleLogProcessor};

    use super::*;

    #[test]
    fn events_are_records_with_flat_primitive_attributes() {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_log_processor(SimpleLogProcessor::new(exporter.clone()))
            .build();
        let config = AnalyticsConfig {
            backend: Backend::Otel,
            prop_keys: keys(&["s", "i", "u", "big", "f", "b"]),
            ..AnalyticsConfig::default()
        };
        let client = AnalyticsClient::with_logger_provider(config, provider);
        assert_eq!(client.sink_name(), "otel");
        client.track(
            "checkout.done",
            &[
                ("s", "text".into()),
                ("i", (-3_i64).into()),
                ("u", 7_u64.into()),
                ("big", u64::MAX.into()),
                ("f", 1.5.into()),
                ("b", true.into()),
                ("dropped", 1.into()),
            ],
            Some("/cart?x=1"),
        );
        let logs = exporter.get_emitted_logs().unwrap();
        assert_eq!(logs.len(), 1);
        let record = &logs[0].record;
        assert_eq!(record.event_name(), Some("analytics_event"));
        let attrs: Vec<(String, AnyValue)> = record
            .attributes_iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.clone()))
            .collect();
        let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(
            get("analytics_event"),
            Some(AnyValue::String("checkout.done".into()))
        );
        assert_eq!(get("analytics_url"), Some(AnyValue::String("/cart".into())));
        assert_eq!(
            get("analytics_prop_s"),
            Some(AnyValue::String("text".into()))
        );
        assert_eq!(get("analytics_prop_i"), Some(AnyValue::Int(-3)));
        assert_eq!(get("analytics_prop_u"), Some(AnyValue::Int(7)));
        assert_eq!(
            get("analytics_prop_big"),
            Some(AnyValue::String(u64::MAX.to_string().into()))
        );
        assert_eq!(get("analytics_prop_f"), Some(AnyValue::Double(1.5)));
        assert_eq!(get("analytics_prop_b"), Some(AnyValue::Boolean(true)));
        assert!(get("analytics_prop_dropped").is_none());
        assert!(
            attrs
                .iter()
                .all(|(_, v)| !matches!(v, AnyValue::Map(_) | AnyValue::ListAny(_))),
            "attributes are primitives, never a map"
        );
    }
}

#[test]
fn the_log_backend_and_a_telemetry_handle_work_in_process() {
    let config = AnalyticsConfig {
        backend: Backend::Log,
        prop_keys: keys(&["plan"]),
        ..AnalyticsConfig::default()
    };
    let client = AnalyticsClient::new(config);
    client.track("page.view", &[("plan", "pro".into())], Some("/p"));
    // A handle without exported logs leaves the otel backend off.
    let telemetry = crate::logs::Telemetry::inert();
    let otel = AnalyticsConfig {
        backend: Backend::Otel,
        ..AnalyticsConfig::default()
    };
    let client = AnalyticsClient::with_telemetry(otel, &telemetry);
    assert_eq!(client.sink_name(), "none");
    client.track("page.view", &[], None);
}
