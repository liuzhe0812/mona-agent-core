fn main() {
    #[cfg(feature = "tauri")]
    tauri_plugin::Builder::new(&[
        "start_task", "cancel_task", "send_input", "get_snapshot", "get_result",
        "forget_run", "subscribe_events", "ack_event", "unsubscribe_events",
    ]).build();
}
