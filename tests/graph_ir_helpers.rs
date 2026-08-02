#![allow(dead_code, unused_imports)]
pub use bindb::{
    AggFunc, BinderStore, CmpOp, CmpValue, EdgeId, GraphIrChainedQuery, GraphIrQuery,
    GraphIrScanQuery, GraphIrUnionQuery, GraphIrWrite, GraphIrWriteError, NodeIdPred, ObjectId,
    OrderByField, OrderDirection, Transaction, WithAggregation,
};
pub use tempfile::TempDir;

pub fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}
pub fn obj(s: &str) -> ObjectId {
    ObjectId::new(s)
}
pub fn eid(s: &str) -> EdgeId {
    EdgeId::new(s)
}

pub fn build_social_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(obj("person://alice"), b"alice".to_vec());
    tx.put_object(obj("memory://m1"), b"memory1".to_vec());
    tx.put_object(obj("memory://m2"), b"memory2".to_vec());
    tx.put_object(obj("memory://m3"), b"memory3".to_vec());
    tx.put_object(obj("tag://t1"), b"tag1".to_vec());
    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e1"),
        obj("person://alice"),
        obj("memory://m1"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e2"),
        obj("person://alice"),
        obj("memory://m2"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e3"),
        obj("person://alice"),
        obj("tag://t1"),
        "TAGGED",
        None,
    );
    tx2.put_edge(
        eid("e4"),
        obj("memory://m1"),
        obj("memory://m3"),
        "RELATES_TO",
        None,
    );
    store.commit(tx2, "test", "edges").unwrap();
}

pub fn build_sortable_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(obj("root://alice"), b"alice".to_vec());
    // IDs chosen so alphabetical order is c < a < b (reverse of name order).
    tx.put_object(obj("node://charlie"), b"c".to_vec());
    tx.put_object(obj("node://alpha"), b"a".to_vec());
    tx.put_object(obj("node://bravo"), b"b".to_vec());
    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ea"),
        obj("root://alice"),
        obj("node://charlie"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("eb"),
        obj("root://alice"),
        obj("node://alpha"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("ec"),
        obj("root://alice"),
        obj("node://bravo"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "edges").unwrap();
}

pub fn build_optional_graph(store: &BinderStore) {
    let mut tx = Transaction::new();

    // People
    tx.put_object(obj("person://alice"), br#"{"name":"Alice"}"#.to_vec());
    tx.put_object(
        obj("person://bob"),
        br#"{"name":"Bob","score":10}"#.to_vec(),
    );
    tx.put_object(
        obj("person://carol"),
        br#"{"name":"Carol","score":5}"#.to_vec(),
    );
    tx.put_object(
        obj("person://dave"),
        br#"{"name":"Dave","score":20}"#.to_vec(),
    );

    // Tags
    tx.put_object(
        obj("tag://rust"),
        br#"{"label":"rust","weight":0.9}"#.to_vec(),
    );
    tx.put_object(
        obj("tag://databases"),
        br#"{"label":"databases","weight":0.4}"#.to_vec(),
    );

    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    // alice KNOWS bob, carol, dave
    tx2.put_edge(
        eid("e-a-bob"),
        obj("person://alice"),
        obj("person://bob"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e-a-carol"),
        obj("person://alice"),
        obj("person://carol"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e-a-dave"),
        obj("person://alice"),
        obj("person://dave"),
        "KNOWS",
        None,
    );
    // bob HAS_TAG rust, databases
    tx2.put_edge(
        eid("e-bob-rust"),
        obj("person://bob"),
        obj("tag://rust"),
        "HAS_TAG",
        None,
    );
    tx2.put_edge(
        eid("e-bob-db"),
        obj("person://bob"),
        obj("tag://databases"),
        "HAS_TAG",
        None,
    );
    // dave HAS_TAG rust
    tx2.put_edge(
        eid("e-dave-rust"),
        obj("person://dave"),
        obj("tag://rust"),
        "HAS_TAG",
        None,
    );
    // carol has no HAS_TAG edges
    store.commit(tx2, "test", "edges").unwrap();
}

pub fn build_grouped_graph(store: &BinderStore) {
    // hub → 5 nodes across 3 categories:
    //   animal: cat (score=2), dog (score=4)
    //   plant:  rose (score=6), fern (score=8)
    //   other:  rock (score=1)
    let mut tx = Transaction::new();
    tx.put_object(obj("h://root"), b"{}".to_vec());
    tx.put_object(obj("n://cat"), br#"{"type":"animal","score":2}"#.to_vec());
    tx.put_object(obj("n://dog"), br#"{"type":"animal","score":4}"#.to_vec());
    tx.put_object(obj("n://rose"), br#"{"type":"plant","score":6}"#.to_vec());
    tx.put_object(obj("n://fern"), br#"{"type":"plant","score":8}"#.to_vec());
    tx.put_object(obj("n://rock"), br#"{"type":"other","score":1}"#.to_vec());
    tx.put_edge(eid("e1"), obj("h://root"), obj("n://cat"), "LINKS", None);
    tx.put_edge(eid("e2"), obj("h://root"), obj("n://dog"), "LINKS", None);
    tx.put_edge(eid("e3"), obj("h://root"), obj("n://rose"), "LINKS", None);
    tx.put_edge(eid("e4"), obj("h://root"), obj("n://fern"), "LINKS", None);
    tx.put_edge(eid("e5"), obj("h://root"), obj("n://rock"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();
}

pub fn build_multikey_graph(store: &BinderStore) {
    // hub → 6 nodes, two grouping fields: type + status
    //   (animal, active): cat (score=2), dog (score=4)
    //   (animal, retired): wolf (score=3)
    //   (plant, active): rose (score=6)
    //   (plant, retired): fern (score=8)
    //   (null, active): rock (score=1) — missing type field
    let mut tx = Transaction::new();
    tx.put_object(obj("h://mk_root"), b"{}".to_vec());
    tx.put_object(
        obj("n://mk/cat"),
        br#"{"type":"animal","status":"active","score":2}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/dog"),
        br#"{"type":"animal","status":"active","score":4}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/wolf"),
        br#"{"type":"animal","status":"retired","score":3}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/rose"),
        br#"{"type":"plant","status":"active","score":6}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/fern"),
        br#"{"type":"plant","status":"retired","score":8}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/rock"),
        br#"{"status":"active","score":1}"#.to_vec(),
    );
    tx.put_edge(
        eid("ek1"),
        obj("h://mk_root"),
        obj("n://mk/cat"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek2"),
        obj("h://mk_root"),
        obj("n://mk/dog"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek3"),
        obj("h://mk_root"),
        obj("n://mk/wolf"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek4"),
        obj("h://mk_root"),
        obj("n://mk/rose"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek5"),
        obj("h://mk_root"),
        obj("n://mk/fern"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek6"),
        obj("h://mk_root"),
        obj("n://mk/rock"),
        "MK",
        None,
    );
    store.commit(tx, "test", "").unwrap();
}

pub fn build_weighted_graph(store: &BinderStore) {
    // root → 4 nodes via edges that carry a JSON payload with a "weight" field.
    //   edge to n://strong1: {"weight": 0.9, "kind": "strong"}
    //   edge to n://strong2: {"weight": 0.8, "kind": "strong"}
    //   edge to n://weak1:   {"weight": 0.3, "kind": "weak"}
    //   edge to n://untyped: no payload (None)
    let mut tx = Transaction::new();
    tx.put_object(obj("h://wroot"), b"{}".to_vec());
    tx.put_object(obj("n://strong1"), br#"{"label":"s1"}"#.to_vec());
    tx.put_object(obj("n://strong2"), br#"{"label":"s2"}"#.to_vec());
    tx.put_object(obj("n://weak1"), br#"{"label":"w1"}"#.to_vec());
    tx.put_object(obj("n://untyped"), br#"{"label":"u1"}"#.to_vec());
    tx.put_edge(
        eid("we1"),
        obj("h://wroot"),
        obj("n://strong1"),
        "REL",
        Some(br#"{"weight":0.9,"kind":"strong"}"#.to_vec()),
    );
    tx.put_edge(
        eid("we2"),
        obj("h://wroot"),
        obj("n://strong2"),
        "REL",
        Some(br#"{"weight":0.8,"kind":"strong"}"#.to_vec()),
    );
    tx.put_edge(
        eid("we3"),
        obj("h://wroot"),
        obj("n://weak1"),
        "REL",
        Some(br#"{"weight":0.3,"kind":"weak"}"#.to_vec()),
    );
    tx.put_edge(
        eid("we4"),
        obj("h://wroot"),
        obj("n://untyped"),
        "REL",
        None,
    );
    store.commit(tx, "test", "").unwrap();
}

pub fn build_string_pred_graph() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // Nodes with string payload fields
    tx.put_object(
        obj("sp://alice"),
        br#"{"name":"Alice Smith","role":"admin","tag":"alpha-user"}"#.to_vec(),
    );
    tx.put_object(
        obj("sp://bob"),
        br#"{"name":"Bobby Jones","role":"editor","tag":"beta-user"}"#.to_vec(),
    );
    tx.put_object(
        obj("sp://carol"),
        br#"{"name":"Carol White","role":"admin","tag":"alpha-mod"}"#.to_vec(),
    );
    tx.put_object(
        obj("sp://dave"),
        br#"{"name":"Dave Brown","role":"viewer","tag":"gamma"}"#.to_vec(),
    );
    tx.put_object(obj("sp://root"), br#"{"name":"Root"}"#.to_vec());
    // Edges from root to everyone
    tx.put_edge(
        eid("sp_e1"),
        obj("sp://root"),
        obj("sp://alice"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("sp_e2"),
        obj("sp://root"),
        obj("sp://bob"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("sp_e3"),
        obj("sp://root"),
        obj("sp://carol"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("sp_e4"),
        obj("sp://root"),
        obj("sp://dave"),
        "KNOWS",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    (store, dir)
}

pub fn build_null_check_graph() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // "optional_field" present on some nodes, absent on others
    tx.put_object(obj("nc://a"), br#"{"name":"alpha","score":10}"#.to_vec());
    tx.put_object(
        obj("nc://b"),
        br#"{"name":"beta"}"#.to_vec(), // no "score"
    );
    tx.put_object(obj("nc://c"), br#"{"name":"gamma","score":0}"#.to_vec());
    tx.put_object(
        obj("nc://d"),
        br#"{"name":"delta"}"#.to_vec(), // no "score"
    );
    tx.put_object(obj("nc://root"), b"{}".to_vec());
    tx.put_edge(eid("nc_e1"), obj("nc://root"), obj("nc://a"), "LINK", None);
    tx.put_edge(eid("nc_e2"), obj("nc://root"), obj("nc://b"), "LINK", None);
    tx.put_edge(eid("nc_e3"), obj("nc://root"), obj("nc://c"), "LINK", None);
    tx.put_edge(eid("nc_e4"), obj("nc://root"), obj("nc://d"), "LINK", None);
    store.commit(tx, "test", "").unwrap();
    (store, dir)
}

pub fn build_in_pred_graph() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        obj("in://alice"),
        br#"{"role":"admin","dept":"eng"}"#.to_vec(),
    );
    tx.put_object(
        obj("in://bob"),
        br#"{"role":"editor","dept":"eng"}"#.to_vec(),
    );
    tx.put_object(
        obj("in://carol"),
        br#"{"role":"viewer","dept":"hr"}"#.to_vec(),
    );
    tx.put_object(
        obj("in://dave"),
        br#"{"role":"admin","dept":"hr"}"#.to_vec(),
    );
    tx.put_object(obj("in://root"), b"{}".to_vec());
    tx.put_edge(
        eid("in_e1"),
        obj("in://root"),
        obj("in://alice"),
        "MEMBER",
        None,
    );
    tx.put_edge(
        eid("in_e2"),
        obj("in://root"),
        obj("in://bob"),
        "MEMBER",
        None,
    );
    tx.put_edge(
        eid("in_e3"),
        obj("in://root"),
        obj("in://carol"),
        "MEMBER",
        None,
    );
    tx.put_edge(
        eid("in_e4"),
        obj("in://root"),
        obj("in://dave"),
        "MEMBER",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    (store, dir)
}

pub fn build_return_id_graph() -> (BinderStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rid://root"), b"{}".to_vec());
    tx.put_object(
        obj("rid://alice"),
        br#"{"name":"alice","score":10}"#.to_vec(),
    );
    tx.put_object(obj("rid://bob"), br#"{"name":"bob","score":20}"#.to_vec());
    tx.put_object(
        obj("rid://carol"),
        br#"{"name":"carol","score":15}"#.to_vec(),
    );
    tx.put_edge(
        eid("rid://e1"),
        obj("rid://root"),
        obj("rid://alice"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("rid://e2"),
        obj("rid://root"),
        obj("rid://bob"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("rid://e3"),
        obj("rid://root"),
        obj("rid://carol"),
        "KNOWS",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    (store, dir)
}

pub fn build_between_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(obj("hub://root"), b"{}".to_vec());
    tx.put_object(obj("item://low"), br#"{"price":5.0}"#.to_vec());
    tx.put_object(obj("item://mid"), br#"{"price":50.0}"#.to_vec());
    tx.put_object(obj("item://high"), br#"{"price":150.0}"#.to_vec());
    tx.put_object(obj("item://no-price"), b"{}".to_vec());
    tx.put_edge(eid("e1"), obj("hub://root"), obj("item://low"), "HAS", None);
    tx.put_edge(eid("e2"), obj("hub://root"), obj("item://mid"), "HAS", None);
    tx.put_edge(
        eid("e3"),
        obj("hub://root"),
        obj("item://high"),
        "HAS",
        None,
    );
    tx.put_edge(
        eid("e4"),
        obj("hub://root"),
        obj("item://no-price"),
        "HAS",
        None,
    );
    store.commit(tx, "test", "seed").unwrap();
}

pub fn build_like_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(obj("hub://root"), b"{}".to_vec());
    tx.put_object(
        obj("node://alice"),
        br#"{"name":"alice@corp.com","code":"US01"}"#.to_vec(),
    );
    tx.put_object(
        obj("node://bob"),
        br#"{"name":"bob@corp.com","code":"US02"}"#.to_vec(),
    );
    tx.put_object(
        obj("node://carol"),
        br#"{"name":"carol@other.com","code":"EU01"}"#.to_vec(),
    );
    tx.put_object(
        obj("node://dave"),
        br#"{"name":"dave","code":"EU02"}"#.to_vec(), // no @ — won't match email pattern
    );
    tx.put_edge(
        eid("e1"),
        obj("hub://root"),
        obj("node://alice"),
        "HAS",
        None,
    );
    tx.put_edge(eid("e2"), obj("hub://root"), obj("node://bob"), "HAS", None);
    tx.put_edge(
        eid("e3"),
        obj("hub://root"),
        obj("node://carol"),
        "HAS",
        None,
    );
    tx.put_edge(
        eid("e4"),
        obj("hub://root"),
        obj("node://dave"),
        "HAS",
        None,
    );
    store.commit(tx, "test", "seed").unwrap();
}

pub fn build_scan_store(dir: &std::path::Path) -> BinderStore {
    let store = BinderStore::open(dir).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        obj("people://alice"),
        br#"{"name":"Alice","age":30,"role":"admin"}"#.to_vec(),
    );
    tx.put_object(
        obj("people://bob"),
        br#"{"name":"Bob","age":25,"role":"user"}"#.to_vec(),
    );
    tx.put_object(
        obj("people://carol"),
        br#"{"name":"Carol","age":35,"role":"admin"}"#.to_vec(),
    );
    tx.put_object(
        obj("people://dave"),
        br#"{"name":"Dave","age":22,"role":"user"}"#.to_vec(),
    );
    tx.put_object(obj("other://x"), br#"{"name":"X"}"#.to_vec());
    store.commit(tx, "test", "scan_store").unwrap();
    store
}

pub fn build_multistart_store(dir: &std::path::Path) -> BinderStore {
    let store = BinderStore::open(dir).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn://alice"), br#"{"name":"Alice"}"#.to_vec());
    tx.put_object(obj("sn://bob"), br#"{"name":"Bob"}"#.to_vec());
    tx.put_object(obj("sn://carol"), br#"{"name":"Carol"}"#.to_vec());
    tx.put_object(obj("sn://dave"), br#"{"name":"Dave"}"#.to_vec());
    tx.put_object(obj("sn://eve"), br#"{"name":"Eve"}"#.to_vec());
    tx.put_object(obj("sn://shared"), br#"{"name":"Shared"}"#.to_vec());
    store.commit(tx, "test", "nodes").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ms:e1"),
        obj("sn://alice"),
        obj("sn://bob"),
        "FOLLOWS",
        None,
    );
    tx2.put_edge(
        eid("ms:e2"),
        obj("sn://alice"),
        obj("sn://carol"),
        "FOLLOWS",
        None,
    );
    tx2.put_edge(
        eid("ms:e3"),
        obj("sn://dave"),
        obj("sn://eve"),
        "FOLLOWS",
        None,
    );
    tx2.put_edge(
        eid("ms:e4"),
        obj("sn://alice"),
        obj("sn://shared"),
        "FOLLOWS",
        None,
    );
    tx2.put_edge(
        eid("ms:e5"),
        obj("sn://dave"),
        obj("sn://shared"),
        "FOLLOWS",
        None,
    );
    store.commit(tx2, "test", "edges").unwrap();
    store
}

pub fn build_arr_graph(dir: &std::path::Path) -> BinderStore {
    let store = BinderStore::open(dir).unwrap();
    let mut tx = Transaction::new();
    // Hub
    tx.put_object(obj("obs://arr:hub"), b"{}".to_vec());
    // Nodes with array tags
    tx.put_object(
        obj("obs://arr:one"),
        br#"{"tags":["rust","perf","db"],"score":3}"#.to_vec(),
    );
    tx.put_object(
        obj("obs://arr:two"),
        br#"{"tags":["graph"],"score":1}"#.to_vec(),
    );
    tx.put_object(
        obj("obs://arr:three"),
        br#"{"tags":[],"score":0}"#.to_vec(),
    );
    // Edges from hub
    tx.put_edge(
        eid("obs://arr:hub->one"),
        obj("obs://arr:hub"),
        obj("obs://arr:one"),
        "LINK",
        None,
    );
    tx.put_edge(
        eid("obs://arr:hub->two"),
        obj("obs://arr:hub"),
        obj("obs://arr:two"),
        "LINK",
        None,
    );
    tx.put_edge(
        eid("obs://arr:hub->three"),
        obj("obs://arr:hub"),
        obj("obs://arr:three"),
        "LINK",
        None,
    );
    store.commit(tx, "test", "arr_graph").unwrap();
    store
}

pub fn build_tagged_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    // hub
    tx.put_object(obj("hub://root"), br#"{"name":"root"}"#.to_vec());
    // spokes with tags arrays
    tx.put_object(
        obj("node://rust"),
        br#"{"name":"rust","tags":["systems","performance","safe"]}"#.to_vec(),
    );
    tx.put_object(
        obj("node://graph"),
        br#"{"name":"graph","tags":["data","graph"]}"#.to_vec(),
    );
    tx.put_object(
        obj("node://perf"),
        br#"{"name":"perf","tags":["performance","latency"]}"#.to_vec(),
    );
    tx.put_object(
        obj("node://empty"),
        br#"{"name":"empty","tags":[]}"#.to_vec(),
    );
    // edges hub → spokes
    for target in &["node://rust", "node://graph", "node://perf", "node://empty"] {
        tx.put_edge(
            EdgeId::new(format!("e-{target}")),
            obj("hub://root"),
            obj(target),
            "LINKS",
            None,
        );
    }
    store.commit(tx, "test", "tagged").unwrap();
}
