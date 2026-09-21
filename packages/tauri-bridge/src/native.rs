use crate::{ChannelBridge, ChannelPacket, ChannelSink, ChannelSubscription};
use application::*;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tauri::{Manager, Runtime, State, Webview, ipc::Channel, plugin::{Builder, TauriPlugin}};

struct NativeState { bridge: ChannelBridge, allowed_webviews: BTreeSet<String> }
impl NativeState {
    fn authorize(&self, label: &str) -> ApplicationResult<()> {
        if self.allowed_webviews.contains(label) { Ok(()) }
        else { Err(ApplicationError::new(ApplicationErrorCode::NotFound, "webview is not authorized for this agent")) }
    }
}
struct NativeSink(Channel<ChannelPacket>);
impl ChannelSink for NativeSink {
    fn send(&self, packet: ChannelPacket) -> ApplicationResult<()> {
        self.0.send(packet).map_err(|_| ApplicationError::new(ApplicationErrorCode::Closed, "Tauri channel is closed"))
    }
}

/// Trusted local webviews only. Pair the allow-list with Tauri capabilities and a restrictive CSP.
/// Keep the Core Host alive in the composition root; this plugin never constructs an Engine.
pub fn init<R: Runtime>(application: AgentApplication, allowed_webviews: impl IntoIterator<Item = String>) -> ApplicationResult<TauriPlugin<R>> {
    let state = NativeState { bridge: ChannelBridge::new(application, Duration::from_secs(30))?, allowed_webviews: allowed_webviews.into_iter().collect() };
    if state.allowed_webviews.is_empty() { return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "at least one trusted local webview label is required")); }
    Ok(Builder::new("bridge")
        .setup(move |app, _api| {
            if !app.manage(state) { return Err("agent bridge already installed".into()); }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_task, cancel_task, send_input, get_snapshot,
            get_result, forget_run, subscribe_events, ack_event, unsubscribe_events])
        .on_event(|app, event| {
            if let tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } = event {
                if let Some(state) = app.try_state::<NativeState>() { state.bridge.detach_owner(label); }
            }
        })
        .build())
}
#[tauri::command]
async fn start_task<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, request: StartRequest) -> ApplicationResult<StartResponse> {
    state.authorize(webview.label())?; state.bridge.application().start_task(request)
}
#[tauri::command]
async fn cancel_task<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, run_id: String) -> ApplicationResult<CancelReceipt> {
    state.authorize(webview.label())?; state.bridge.application().cancel_task(&run_id)
}
#[tauri::command]
async fn send_input<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, run_id: String, request: InputRequest) -> ApplicationResult<InputReceipt> {
    state.authorize(webview.label())?; state.bridge.application().send_input(&run_id, request).await
}
#[tauri::command]
async fn get_snapshot<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, run_id: String) -> ApplicationResult<api::RunSnapshot> {
    state.authorize(webview.label())?; state.bridge.application().get_snapshot(&run_id)
}
#[tauri::command]
async fn get_result<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, run_id: String) -> ApplicationResult<ResultResponse> {
    state.authorize(webview.label())?; state.bridge.application().get_result(&run_id)
}
#[tauri::command]
async fn forget_run<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, run_id: String) -> ApplicationResult<()> {
    state.authorize(webview.label())?; state.bridge.application().forget(&run_id)
}
#[tauri::command]
async fn subscribe_events<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, run_id: String, after: Option<u64>, on_event: Channel<ChannelPacket>) -> ApplicationResult<ChannelSubscription> {
    state.authorize(webview.label())?; state.bridge.subscribe(webview.label(), &run_id, after, Arc::new(NativeSink(on_event)))
}
#[tauri::command]
async fn ack_event<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, subscription_id: String, delivery_id: u64) -> ApplicationResult<()> {
    state.authorize(webview.label())?; state.bridge.acknowledge(webview.label(), &subscription_id, delivery_id)
}
#[tauri::command]
async fn unsubscribe_events<R: Runtime>(webview: Webview<R>, state: State<'_, NativeState>, subscription_id: String) -> ApplicationResult<()> {
    state.authorize(webview.label())?; state.bridge.unsubscribe(webview.label(), &subscription_id)
}
