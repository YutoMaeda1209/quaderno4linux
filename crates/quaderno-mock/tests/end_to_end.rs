//! モック端末とペアリングし、クライアントのすべてのファイル操作を試す。

use std::net::{IpAddr, Ipv4Addr};

use quaderno_core::{no_progress, CredentialStore, DeviceAddress, DeviceClient, Registration, ROOT_FOLDER_ID};
use quaderno_mock::MockDevice;

#[test]
fn pairing_and_file_operations() {
    // 127.0.0.2 を使い、開発者が 127.0.0.1 で動かしているものと干渉しないようにする。
    let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
    let device = MockDevice::start(ip, "87654321").expect("start mock");
    let addr = DeviceAddress::new(ip, 0);

    let info = quaderno_core::device_information(&addr, std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(info.serial_number.as_deref(), Some("MOCK0001"));

    // 間違った PIN は拒否される。
    let bad = Registration::start(&addr).unwrap().finish("00000000");
    assert!(bad.is_err());

    // 正しい PIN でクライアントが登録される。
    let creds = Registration::start(&addr).unwrap().finish("87654321").expect("pairing");
    let dir = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(dir.path().join("cfg"));
    store.save(&creds).unwrap();

    let client = DeviceClient::connect(addr, creds).expect("auth");
    client.ping().unwrap();
    assert_eq!(client.battery().unwrap().level, Some(87));

    // フォルダを作成してアップロードする。
    let a = client.create_folder(ROOT_FOLDER_ID, "A").unwrap();
    let b = client.create_folder(ROOT_FOLDER_ID, "B").unwrap();
    let local = dir.path().join("doc.pdf");
    std::fs::write(&local, b"%PDF-1.4 hello").unwrap();
    client.upload_file(&local, &a, &no_progress()).unwrap();
    assert!(client.upload_file(&local, &a, &no_progress()).is_err(), "duplicate name must conflict");

    let tree = dir.path().join("tree");
    std::fs::create_dir_all(tree.join("sub")).unwrap();
    std::fs::write(tree.join("x.pdf"), b"x").unwrap();
    std::fs::write(tree.join("sub/y.pdf"), b"y").unwrap();
    client.upload_dir(&tree, ROOT_FOLDER_ID, &no_progress()).unwrap();

    let docs = device.documents();
    assert_eq!(docs["Document/A/doc.pdf"], b"%PDF-1.4 hello");
    assert_eq!(docs["Document/tree/sub/y.pdf"], b"y");

    // 名前の変更、コピー、移動。
    let doc = client.list_folder(&a).unwrap().into_iter().find(|e| e.entry_name == "doc.pdf").unwrap();
    assert_eq!(doc.file_size, Some(14));
    client.rename(&doc, "renamed.pdf").unwrap();
    let doc = client.resolve_path("Document/A/renamed.pdf").unwrap();
    client.copy_to(&doc, &b, None).unwrap();
    client.copy_to(&doc, &a, Some("renamed (コピー).pdf")).unwrap();
    let folder_a = client.resolve_path("Document/A").unwrap();
    client.copy_to(&folder_a, &b, None).unwrap();
    let tree_entry = client.resolve_path("Document/tree").unwrap();
    client.move_to(&tree_entry, &b).unwrap();
    let folder_b = client.resolve_path("Document/B").unwrap();
    client.rename(&folder_b, "B2").unwrap();

    let docs = device.documents();
    for p in [
        "Document/A/renamed.pdf",
        "Document/A/renamed (コピー).pdf",
        "Document/B2/renamed.pdf",
        "Document/B2/A/renamed.pdf",
        "Document/B2/tree/x.pdf",
        "Document/B2/tree/sub/y.pdf",
    ] {
        assert!(docs.contains_key(p), "missing {p}: {:?}", docs.keys());
    }

    // フォルダを再帰的にダウンロードする。
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let tree_entry = client.resolve_path("Document/B2/tree").unwrap();
    client.download_entry(&tree_entry, &out, &no_progress()).unwrap();
    assert_eq!(std::fs::read(out.join("tree/sub/y.pdf")).unwrap(), b"y");

    // 削除 (再帰的)。
    let b2 = client.resolve_path("Document/B2").unwrap();
    client.delete(&b2).unwrap();
    assert_eq!(device.folders(), vec!["Document".to_string(), "Document/A".to_string()]);
    let renamed = client.resolve_path("Document/A/renamed.pdf").unwrap();
    client.delete(&renamed).unwrap();
    assert_eq!(client.list_folder(&a).unwrap().len(), 1);

    // 保存した認証情報で再び認証できる。
    let creds = store.load().unwrap().unwrap();
    DeviceClient::connect(addr, creds).unwrap().ping().unwrap();
}

/// USB 接続は IPv6 だけを使うので、クライアントが IPv6 アドレスを扱えることを確かめる。
#[test]
fn ipv6_connection() {
    let ip: IpAddr = "::1".parse().unwrap();
    let device = match MockDevice::start(ip, "11112222") {
        Ok(d) => d,
        Err(e) => {
            eprintln!("skipping: IPv6 loopback unavailable ({e})");
            return;
        }
    };
    device.add_document(ROOT_FOLDER_ID, "v6.pdf", b"6");
    let addr = DeviceAddress::parse("[::1]").unwrap();
    let creds = Registration::start(&addr).unwrap().finish("11112222").unwrap();
    let client = DeviceClient::connect(addr, creds).unwrap();
    let names: Vec<_> = client.list_root().unwrap().into_iter().map(|e| e.entry_name).collect();
    assert_eq!(names, vec!["v6.pdf".to_string()]);
}
