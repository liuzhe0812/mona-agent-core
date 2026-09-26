use std::fs;
use workspace::Directory;

#[test]
fn metadata_has_the_same_root_and_private_path_authorization_without_loading_content() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("work"); let private = root.join("private");
    fs::create_dir_all(&private).unwrap();
    fs::write(root.join("report.md"), "confirmed").unwrap();
    fs::write(private.join("secret.txt"), "never displayed").unwrap();
    let directory = Directory::open(&root).unwrap().excluding([private]);
    let info = directory.stat("report.md").unwrap();
    assert_eq!(info.name, "report.md"); assert_eq!(info.kind, "file"); assert_eq!(info.bytes, 9);
    assert!(directory.stat("../outside").is_err());
    assert!(directory.stat("private/secret.txt").is_err());
    assert!(directory.stat("missing.md").is_err());
    assert_eq!(fs::read_to_string(root.join("report.md")).unwrap(), "confirmed");
}
