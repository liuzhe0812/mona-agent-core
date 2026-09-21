use application::*;
use runtime::HostBuilder;
use demo::{ScriptedModel, text};
use std::{sync::{Arc, Mutex}, time::Duration};
use tauri_plugin_bridge::*;
#[derive(Default)]
struct Sink(Mutex<Vec<ChannelPacket>>);
impl ChannelSink for Sink {
    fn send(&self, packet: ChannelPacket) -> ApplicationResult<()> { self.0.lock().unwrap().push(packet); Ok(()) }
}
async fn wait_packet(sink: &Sink, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while sink.0.lock().unwrap().len() < count { tokio::time::sleep(Duration::from_millis(1)).await; }
    }).await.unwrap();
}
#[tokio::test]
async fn channel_waits_for_ack_and_cannot_be_acked_by_another_webview() {
    let mut host = HostBuilder::new().model(Arc::new(ScriptedModel::new(vec![text("ok")]))).build().await.unwrap();
    let app = AgentApplication::new(Arc::new(host.engine()), Default::default()).unwrap();
    let id = app.start_task(StartRequest { request_id: "one".into(), prompt: "hi".into() }).unwrap().run_id;
    let bridge = ChannelBridge::new(app.clone(), Duration::from_secs(2)).unwrap();
    let sink = Arc::new(Sink::default());
    let stream = bridge.subscribe("main", &id, Some(0), sink.clone()).unwrap();
    wait_packet(&sink, 1).await;
    tokio::time::sleep(Duration::from_millis(10)).await; assert_eq!(sink.0.lock().unwrap().len(), 1);
    let packet = sink.0.lock().unwrap()[0].clone();
    assert!(bridge.acknowledge("other", &stream.subscription_id, packet.delivery_id).is_err());
    assert!(bridge.acknowledge("main", &stream.subscription_id, packet.delivery_id + 1).is_err());
    bridge.acknowledge("main", &stream.subscription_id, packet.delivery_id).unwrap();
    wait_packet(&sink, 2).await;
    bridge.unsubscribe("main", &stream.subscription_id).unwrap();
    app.shutdown(Duration::from_secs(2)).await.unwrap(); host.shutdown().await.unwrap();
}
#[tokio::test]
async fn disconnect_detaches_channel_but_does_not_cancel_task() {
    let mut host = HostBuilder::new().model(Arc::new(ScriptedModel::new(vec![text("ok")]))).build().await.unwrap();
    let app = AgentApplication::new(Arc::new(host.engine()), Default::default()).unwrap();
    let id = app.start_task(StartRequest { request_id: "one".into(), prompt: "hi".into() }).unwrap().run_id;
    let bridge = ChannelBridge::new(app.clone(), Duration::from_secs(1)).unwrap();
    bridge.subscribe("main", &id, None, Arc::new(Sink::default())).unwrap(); bridge.detach_owner("main");
    let mut stream = app.subscribe_events(&id, None).unwrap(); while stream.next().await.is_some() {}
    assert_eq!(app.get_result(&id).unwrap().outcome.unwrap().status, api::RunStatus::Completed);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn unacknowledged_channel_is_evicted_after_timeout() {
    let mut host = HostBuilder::new().model(Arc::new(ScriptedModel::new(vec![text("ok")]))).build().await.unwrap();
    let app = AgentApplication::new(Arc::new(host.engine()), Default::default()).unwrap();
    let id = app.start_task(StartRequest { request_id: "one".into(), prompt: "hi".into() }).unwrap().run_id;
    let bridge = ChannelBridge::new(app, Duration::from_millis(5)).unwrap(); let sink = Arc::new(Sink::default());
    let stream = bridge.subscribe("main", &id, None, sink.clone()).unwrap(); wait_packet(&sink, 1).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(bridge.acknowledge("main", &stream.subscription_id, 1).is_err()); host.shutdown().await.unwrap();
}
