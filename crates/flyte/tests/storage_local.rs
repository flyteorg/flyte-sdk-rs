//! Local-filesystem storage roundtrip (bare paths and file:// URIs).

use flyte::storage::Storage;

#[test]
fn local_put_get_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap().to_string();
    let storage = Storage::new();

    let uri = Storage::join(&base, "sub/inputs.pb");
    let payload = bytes::Bytes::from_static(b"hello flyte");
    flyte::run(async {
        storage.put(&uri, payload.clone()).await.unwrap();
        let back = storage.get(&uri).await.unwrap();
        assert_eq!(back, payload);

        // file:// scheme resolves to the same object.
        let back2 = storage.get(&format!("file://{uri}")).await.unwrap();
        assert_eq!(back2, payload);
    });
}

#[test]
fn join_normalizes_trailing_slash() {
    assert_eq!(Storage::join("s3://b/prefix/", "x.pb"), "s3://b/prefix/x.pb");
    assert_eq!(Storage::join("s3://b/prefix", "x.pb"), "s3://b/prefix/x.pb");
}

#[test]
fn unsupported_scheme_errors() {
    let storage = Storage::new();
    let err = flyte::run(async { storage.get("ftp://nope/x").await }).unwrap_err();
    assert!(err.to_string().contains("unsupported storage scheme"), "{err}");
}
