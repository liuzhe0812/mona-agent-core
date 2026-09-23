use api::*;
use models::{ModelCapabilities, ModelManager, Protocol, SettingsStore};
use serde_json::json;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct Store(Mutex<Option<Vec<u8>>>);
impl SettingsStore for Store {
    fn load(&self) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, bytes: &[u8]) -> Result<()> {
        *self.0.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
}
fn input(revision: u64, protocol: Protocol, key: Option<&str>) -> models::UpsertProvider {
    serde_json::from_value(json!({"revision":revision,"id":"one","name":"One","protocol":protocol,"api_base":"https://example.invalid/v1","api_key":key,"models":[{"id":"fixture","enabled":true,"context_window_tokens":8192,"capabilities":{"images":false,"tools":true,"max_output_tokens":4096}}]})).unwrap()
}
#[test]
fn protocols_generation_and_capabilities_roundtrip_without_version_fallback() {
    for protocol in [
        Protocol::ChatCompletions,
        Protocol::Responses,
        Protocol::Messages,
    ] {
        let store = Arc::new(Store::default());
        let manager = ModelManager::open(store.clone(), false).unwrap();
        let mut p = input(0, protocol, Some("private-key"));
        if protocol == Protocol::Responses {
            p.generation =
                Some(serde_json::from_value(json!({"reasoning":{"effort":"low"}})).unwrap());
        }
        if protocol == Protocol::Messages {
            p.generation =
                Some(serde_json::from_value(json!({"thinking":{"type":"adaptive"}})).unwrap());
        }
        manager.upsert(p).unwrap();
        let view = ModelManager::open(store.clone(), false).unwrap().view();
        assert_eq!(view.providers[0].protocol, protocol);
        assert_eq!(view.providers[0].models[0].capabilities.images, Some(false));
        assert!(!serde_json::to_string(&view)
            .unwrap()
            .contains("private-key"));
        let mut doc: serde_json::Value =
            serde_json::from_slice(&store.load().unwrap().unwrap()).unwrap();
        doc["version"] = json!(1);
        store.save(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert!(ModelManager::open(store, false).is_err());
    }
}
#[test]
fn endpoint_or_protocol_change_cannot_reuse_a_saved_key_without_explicit_authority() {
    let manager = ModelManager::open(Arc::new(Store::default()), false).unwrap();
    manager
        .upsert(input(0, Protocol::Responses, Some("key")))
        .unwrap();
    let mut changed = input(1, Protocol::Messages, None);
    assert!(manager.upsert(input(1, Protocol::Messages, None)).is_err());
    assert_eq!(manager.view().revision, 1);
    changed.clear_key = true;
    manager.upsert(changed).unwrap();
    assert!(!manager.view().providers[0].has_key);
    manager
        .upsert(input(2, Protocol::Messages, Some("new-key")))
        .unwrap();
    let mut changed = input(3, Protocol::Messages, None);
    changed.api_base = "https://elsewhere.invalid/v1".into();
    assert!(manager.upsert(changed).is_err());
    assert_eq!(manager.view().revision, 3);
}
#[test]
fn native_options_reject_hosted_execution_and_mismatched_endpoint_before_persistence() {
    let manager = ModelManager::open(Arc::new(Store::default()), false).unwrap();
    for protocol in [Protocol::Responses, Protocol::Messages] {
        let mut p = input(0, protocol, None);
        p.api_base = "https://example.invalid/v1/chat/completions".into();
        assert!(manager.upsert(p).is_err());
        for key in ["store", "tools", "previous_response_id", "headers"] {
            let mut p = input(0, protocol, None);
            p.generation = Some(serde_json::from_value(json!({key:true})).unwrap());
            assert!(manager.upsert(p).is_err());
        }
    }
    assert!(manager.view().providers.is_empty());
    assert_eq!(ModelCapabilities::default().tools, None);
    assert!(serde_json::from_value::<models::UpsertProvider>(json!({"revision":0,"id":"x","name":"x","api_base":"https://example.invalid/v1","models":[]})).is_err());
}
