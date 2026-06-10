//! Native tests for the engine-agnostic wacore adapters: prove that the `Host*`
//! traits wire correctly into the concrete `wacore::net` traits the core
//! consumes, with real `Send + Sync` (no faking). Run with `cargo test`.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use wacore::net::{
    HttpClient, HttpRequest, HttpResponse, Transport, TransportEvent, TransportFactory,
};

use bridge_core::adapters::{CoreHttpClient, CoreTransportFactory};
use bridge_core::host::{HostHttp, HostTransport, HostTransportFactory, HostTransportSink};
use bridge_core::value::{BridgeValue, LongParts};

// ── HTTP ─────────────────────────────────────────────────────────────────────

struct MockHttp;

#[async_trait]
impl HostHttp for MockHttp {
    async fn execute(&self, req: HttpRequest) -> anyhow::Result<HttpResponse> {
        assert_eq!(req.method, "GET");
        Ok(HttpResponse {
            status_code: 200,
            body: format!("ok:{}", req.url).into_bytes(),
        })
    }
}

#[tokio::test]
async fn http_adapter_delegates_to_host() {
    let client = CoreHttpClient::new(Arc::new(MockHttp));
    let resp = client
        .execute(HttpRequest::get("https://x/"))
        .await
        .expect("execute");
    assert_eq!(resp.status_code, 200);
    assert_eq!(resp.body, b"ok:https://x/");
    // Streaming stays at the trait defaults (JS bridge buffers).
    assert!(!client.supports_streaming());
}

// ── Transport ────────────────────────────────────────────────────────────────

struct MockTransport;

#[async_trait]
impl HostTransport for MockTransport {
    async fn send(&self, _data: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
    async fn disconnect(&self) {}
}

/// A factory that, on connect, immediately drives the sink with a Connected +
/// one data frame (as a real JS transport would from ws events).
struct MockFactory;

#[async_trait]
impl HostTransportFactory for MockFactory {
    async fn connect(
        &self,
        sink: Arc<dyn HostTransportSink>,
    ) -> anyhow::Result<Arc<dyn HostTransport>> {
        sink.on_connected();
        sink.on_data(Bytes::from_static(b"hello"));
        Ok(Arc::new(MockTransport))
    }
}

#[tokio::test]
async fn transport_adapter_pumps_events_through_channel() {
    let factory = CoreTransportFactory::new(Arc::new(MockFactory));
    let (transport, rx) = factory.create_transport().await.expect("create_transport");

    // Events the sink pushed during connect arrive in order on the channel.
    match rx.recv().await.expect("connected") {
        TransportEvent::Connected => {}
        other => panic!("expected Connected, got {other:?}"),
    }
    match rx.recv().await.expect("data") {
        TransportEvent::DataReceived(b) => assert_eq!(&b[..], b"hello"),
        other => panic!("expected DataReceived, got {other:?}"),
    }

    // The wrapped transport delegates send/disconnect without error.
    transport
        .send(Bytes::from_static(b"x"))
        .await
        .expect("send");
    transport.disconnect().await;
}

/// Compile-time proof the adapters are genuinely `Send + Sync` (napi requires it).
#[test]
fn adapters_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CoreHttpClient>();
    assert_send_sync::<CoreTransportFactory>();
}

// ── BridgeValue ──────────────────────────────────────────────────────────────

#[test]
fn long_parts_split_matches_protobufjs() {
    // 0x0000_0002_0000_0003 → low=3, high=2 (the {low,high,unsigned} shape).
    let p = LongParts::from_u64(0x0000_0002_0000_0003);
    assert_eq!(p.low, 3);
    assert_eq!(p.high, 2);
    assert!(p.unsigned);
    assert!(!p.is_zero());
    assert!(LongParts::from_i64(0).is_zero());

    // -1 i64 → low=-1, high=-1 (two's complement halves).
    let n = LongParts::from_i64(-1);
    assert_eq!(n.low, -1);
    assert_eq!(n.high, -1);
}

#[test]
fn bridge_value_object_preserves_order() {
    let v = BridgeValue::object([
        ("b".to_string(), BridgeValue::F64(1.0)),
        ("a".to_string(), BridgeValue::Bool(true)),
    ]);
    match v {
        BridgeValue::Object(fields) => {
            assert_eq!(fields[0].0, "b");
            assert_eq!(fields[1].0, "a");
        }
        _ => panic!("expected object"),
    }
}
