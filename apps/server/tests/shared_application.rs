//! A composition-level test may import both bridges. Neither bridge depends on the other.
use application::*;
use axum::{body::Body, http::Request};
use demo::{text, ScriptedModel};
use http_body_util::BodyExt;
use runtime::HostBuilder;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri_bridge::{ChannelBridge, ChannelPacket, ChannelSink};
use tower::ServiceExt;
#[derive(Default)]
struct Sink(Mutex<Vec<ChannelPacket>>);
impl ChannelSink for Sink {
    fn send(&self, packet: ChannelPacket) -> ApplicationResult<()> {
        self.0.lock().unwrap().push(packet);
        Ok(())
    }
}
#[tokio::test]
async fn http_created_run_is_visible_through_tauri_without_another_runtime() {
    let mut host = HostBuilder::new()
        .model(Arc::new(ScriptedModel::new(vec![text("shared")])))
        .build()
        .await
        .unwrap();
    let app = AgentApplication::new(Arc::new(host.engine()), Default::default()).unwrap();
    let token = "test-only-composition-token-32-bytes";
    let router = http_bridge::router(app.clone(), http_bridge::HttpConfig::new(token)).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/runs")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(r#"{"request_id":"shared","prompt":"hello"}"#))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let created: StartResponse = serde_json::from_slice(&bytes).unwrap();
    let bridge = ChannelBridge::new(app.clone(), Duration::from_secs(2)).unwrap();
    let sink = Arc::new(Sink::default());
    let subscription = bridge
        .subscribe("main", &created.run_id, Some(0), sink.clone())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut index = 0;
        loop {
            let packet = sink.0.lock().unwrap().get(index).cloned();
            if let Some(packet) = packet {
                bridge
                    .acknowledge("main", &subscription.subscription_id, packet.delivery_id)
                    .unwrap();
                index += 1;
                let terminal = match packet.frame {
                    StreamFrame::Event { envelope } => {
                        matches!(envelope.event, api::RunEvent::RunFinished { .. })
                    }
                    StreamFrame::Snapshot { snapshot, .. } => snapshot.outcome.is_some(),
                    StreamFrame::Fault { .. } => panic!("unexpected protocol fault"),
                };
                if terminal {
                    break;
                }
            } else {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        bridge
            .application()
            .get_result(&created.run_id)
            .unwrap()
            .outcome
            .unwrap()
            .output
            .as_deref(),
        Some("shared")
    );
    let duplicate = bridge
        .application()
        .start_task(StartRequest {
            request_id: "shared".into(),
            prompt: "hello".into(),
        })
        .unwrap();
    assert!(duplicate.reused);
    assert_eq!(duplicate.run_id, created.run_id);
    bridge
        .unsubscribe("main", &subscription.subscription_id)
        .unwrap();
    host.shutdown().await.unwrap();
}
