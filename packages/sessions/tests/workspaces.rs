//! The session/cwd contract works with no project registry or Web dependency.
use sessions::Store;
use std::{collections::BTreeMap, fs, path::Path};

#[test]
fn all_session_bindings_survive_default_change_without_project_registration() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let store = Store::open(&state, &a).unwrap();
    let ha = store.create("a").unwrap();
    let hb = store.create_in("b", &b, BTreeMap::new()).unwrap();
    let fake_project = store
        .create_in(
            "p",
            &b,
            BTreeMap::from([("project.id".into(), "removed-registration".into())]),
        )
        .unwrap();
    fs::write(a.join("keep"), "user data").unwrap();
    drop(store);
    let store = Store::open(&state, &b).unwrap();
    assert_eq!(store.header(&ha.id).unwrap().workspace, ha.workspace);
    assert_eq!(store.header(&hb.id).unwrap().workspace, hb.workspace);
    assert_eq!(
        store.header(&fake_project.id).unwrap().workspace,
        hb.workspace
    );
    assert_eq!(store.list(0, 50, "", false).unwrap().sessions.len(), 3);
    let new = store.create("new").unwrap();
    assert_eq!(new.workspace, hb.workspace);
    assert!(store.create("a").is_err()); // Current default cannot silently rebind an existing create key.
    store.delete(&ha.id, ha.revision).unwrap();
    assert!(a.join("keep").exists());
    fs::remove_dir_all(&b).unwrap();
    assert!(store.get(&fake_project.id).is_ok()); // History read does not require a live filesystem.
    assert!(Path::new(&fake_project.workspace).is_absolute());
}
