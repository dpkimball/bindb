#[path = "sql_agg_helpers.rs"]
mod helpers;
use helpers::*;

// --- Window functions ---

#[test]
fn window_row_number_global() {
    // Three rows with different scores; ROW_NUMBER() OVER (ORDER BY score ASC) = 1,2,3
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wrng:a"), br#"{"score":30}"#.to_vec());
    tx.put_object(oid("obs://wrng:b"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wrng:c"), br#"{"score":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let mut rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), ROW_NUMBER() OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS rn FROM 'obs://wrng'",
        )
        .unwrap();
    // Sort output by score so assertions are deterministic.
    rows.sort_by_key(|r| r.get("score").and_then(|v| v.as_i64()).unwrap_or(0));
    assert_eq!(rows[0].get("rn").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(rows[1].get("rn").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(rows[2].get("rn").and_then(|v| v.as_u64()), Some(3));
    store.close().unwrap();
}

#[test]
fn window_row_number_partition_by() {
    // Two departments; ROW_NUMBER() per dept ordered by score.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://wrnp:a"),
        br#"{"dept":"eng","score":10}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://wrnp:b"),
        br#"{"dept":"eng","score":20}"#.to_vec(),
    );
    tx.put_object(oid("obs://wrnp:c"), br#"{"dept":"mkt","score":5}"#.to_vec());
    tx.put_object(
        oid("obs://wrnp:d"),
        br#"{"dept":"mkt","score":15}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('dept'), PAYLOAD_FIELD('score'), \
             ROW_NUMBER() OVER (PARTITION BY PAYLOAD_FIELD('dept') ORDER BY PAYLOAD_FIELD('score') ASC) AS rn \
             FROM 'obs://wrnp'",
        )
        .unwrap();
    // Each dept should have rn=1 and rn=2; no rn > 2.
    let max_rn = rows
        .iter()
        .filter_map(|r| r.get("rn").and_then(|v| v.as_u64()))
        .max()
        .unwrap();
    assert_eq!(max_rn, 2);
    // rn=1 for score=10 in eng and score=5 in mkt.
    let rn1_scores: std::collections::HashSet<i64> = rows
        .iter()
        .filter(|r| r.get("rn").and_then(|v| v.as_u64()) == Some(1))
        .filter_map(|r| r.get("score").and_then(|v| v.as_i64()))
        .collect();
    assert!(rn1_scores.contains(&10));
    assert!(rn1_scores.contains(&5));
    store.close().unwrap();
}

#[test]
fn window_rank_ties() {
    // Two rows with same score get same rank; next rank skips.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wrt:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wrt:b"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wrt:c"), br#"{"score":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), RANK() OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS rnk \
             FROM 'obs://wrt'",
        )
        .unwrap();
    let ranks: std::collections::HashSet<u64> = rows
        .iter()
        .filter_map(|r| r.get("rnk").and_then(|v| v.as_u64()))
        .collect();
    // Ranks should be {1, 3} — two rows at rank 1 (tie), then rank 3 skips 2.
    assert!(ranks.contains(&1));
    assert!(ranks.contains(&3));
    assert!(!ranks.contains(&2));
    store.close().unwrap();
}

#[test]
fn window_dense_rank_ties() {
    // Two rows with same score get same dense_rank; next dense_rank does NOT skip.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wdrt:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wdrt:b"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wdrt:c"), br#"{"score":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), DENSE_RANK() OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS dr \
             FROM 'obs://wdrt'",
        )
        .unwrap();
    let dense_ranks: std::collections::HashSet<u64> = rows
        .iter()
        .filter_map(|r| r.get("dr").and_then(|v| v.as_u64()))
        .collect();
    // Dense ranks should be {1, 2} — no gap.
    assert!(dense_ranks.contains(&1));
    assert!(dense_ranks.contains(&2));
    assert!(!dense_ranks.contains(&3));
    store.close().unwrap();
}

#[test]
fn window_rank_desc_order() {
    // RANK() ORDER BY score DESC — highest score gets rank 1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wrdo:a"), br#"{"score":100}"#.to_vec());
    tx.put_object(oid("obs://wrdo:b"), br#"{"score":50}"#.to_vec());
    tx.put_object(oid("obs://wrdo:c"), br#"{"score":75}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), RANK() OVER (ORDER BY PAYLOAD_FIELD('score') DESC) AS rnk \
             FROM 'obs://wrdo'",
        )
        .unwrap();
    let rank_for = |score: i64| {
        rows.iter()
            .find(|r| r.get("score").and_then(|v| v.as_i64()) == Some(score))
            .and_then(|r| r.get("rnk").and_then(|v| v.as_u64()))
            .unwrap()
    };
    assert_eq!(rank_for(100), 1);
    assert_eq!(rank_for(75), 2);
    assert_eq!(rank_for(50), 3);
    store.close().unwrap();
}

#[test]
fn window_with_limit() {
    // Window functions are computed before LIMIT; LIMIT then trims the output.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for i in 1u64..=5 {
        tx.put_object(
            oid(&format!("obs://wlim:{i}")),
            format!(r#"{{"n":{i}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('n'), ROW_NUMBER() OVER (ORDER BY PAYLOAD_FIELD('n') ASC) AS rn \
             FROM 'obs://wlim' ORDER BY PAYLOAD_FIELD('n') ASC LIMIT 3",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // Row numbers in first 3 rows should be 1, 2, 3.
    let rns: Vec<u64> = rows
        .iter()
        .filter_map(|r| r.get("rn").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(rns, vec![1, 2, 3]);
    store.close().unwrap();
}

// --- LAG / LEAD window functions ---

#[test]
fn window_lag_basic() {
    // LAG(score, 1) returns the previous row's score within the global partition ordered by score ASC.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wlag:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wlag:b"), br#"{"score":20}"#.to_vec());
    tx.put_object(oid("obs://wlag:c"), br#"{"score":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), \
             LAG(PAYLOAD_FIELD('score'), 1) OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS prev \
             FROM 'obs://wlag'",
        )
        .unwrap();
    // Sort rows by score for deterministic assertions.
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("score").and_then(|v| v.as_i64()).unwrap_or(0));

    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(10));
    assert_eq!(rows[0].get("prev"), Some(&serde_json::Value::Null)); // no previous row
    assert_eq!(rows[1].get("score").and_then(|v| v.as_i64()), Some(20));
    assert_eq!(rows[1].get("prev").and_then(|v| v.as_i64()), Some(10));
    assert_eq!(rows[2].get("score").and_then(|v| v.as_i64()), Some(30));
    assert_eq!(rows[2].get("prev").and_then(|v| v.as_i64()), Some(20));
    store.close().unwrap();
}

#[test]
fn window_lead_basic() {
    // LEAD(score, 1) returns the next row's score.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wlead:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wlead:b"), br#"{"score":20}"#.to_vec());
    tx.put_object(oid("obs://wlead:c"), br#"{"score":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), \
             LEAD(PAYLOAD_FIELD('score'), 1) OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS nxt \
             FROM 'obs://wlead'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("score").and_then(|v| v.as_i64()).unwrap_or(0));

    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(10));
    assert_eq!(rows[0].get("nxt").and_then(|v| v.as_i64()), Some(20));
    assert_eq!(rows[1].get("score").and_then(|v| v.as_i64()), Some(20));
    assert_eq!(rows[1].get("nxt").and_then(|v| v.as_i64()), Some(30));
    assert_eq!(rows[2].get("score").and_then(|v| v.as_i64()), Some(30));
    assert_eq!(rows[2].get("nxt"), Some(&serde_json::Value::Null)); // no next row
    store.close().unwrap();
}

#[test]
fn window_lag_offset_2() {
    // LAG with offset=2 skips one row.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for i in 1u64..=4 {
        tx.put_object(
            oid(&format!("obs://wlag2:{i}")),
            format!(r#"{{"n":{i}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('n'), \
             LAG(PAYLOAD_FIELD('n'), 2) OVER (ORDER BY PAYLOAD_FIELD('n') ASC) AS prev2 \
             FROM 'obs://wlag2'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("n").and_then(|v| v.as_i64()).unwrap_or(0));

    assert_eq!(rows[0].get("prev2"), Some(&serde_json::Value::Null));
    assert_eq!(rows[1].get("prev2"), Some(&serde_json::Value::Null));
    assert_eq!(rows[2].get("prev2").and_then(|v| v.as_i64()), Some(1)); // 3 - 2 = 1
    assert_eq!(rows[3].get("prev2").and_then(|v| v.as_i64()), Some(2)); // 4 - 2 = 2
    store.close().unwrap();
}

#[test]
fn window_lag_partition_by() {
    // LAG per partition: two groups (cat A and B); LAG resets at partition boundary.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wlagp:a1"), br#"{"cat":"A","v":1}"#.to_vec());
    tx.put_object(oid("obs://wlagp:a2"), br#"{"cat":"A","v":2}"#.to_vec());
    tx.put_object(oid("obs://wlagp:b1"), br#"{"cat":"B","v":10}"#.to_vec());
    tx.put_object(oid("obs://wlagp:b2"), br#"{"cat":"B","v":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('cat'), PAYLOAD_FIELD('v'), \
             LAG(PAYLOAD_FIELD('v'), 1) OVER (PARTITION BY PAYLOAD_FIELD('cat') ORDER BY PAYLOAD_FIELD('v') ASC) AS prev \
             FROM 'obs://wlagp'",
        )
        .unwrap();

    let get = |cat: &str, v: i64| {
        rows.iter()
            .find(|r| {
                r.get("cat").and_then(|x| x.as_str()) == Some(cat)
                    && r.get("v").and_then(|x| x.as_i64()) == Some(v)
            })
            .map(|r| r.get("prev").cloned().unwrap_or(serde_json::Value::Null))
            .unwrap()
    };
    // First in each partition: no lag.
    assert_eq!(get("A", 1), serde_json::Value::Null);
    assert_eq!(get("B", 10), serde_json::Value::Null);
    // Second in each partition: lag = first value.
    assert_eq!(get("A", 2).as_i64(), Some(1));
    assert_eq!(get("B", 20).as_i64(), Some(10));
    store.close().unwrap();
}

#[test]
fn window_lag_lead_combined() {
    // Both LAG and LEAD in the same SELECT.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wll:a"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("obs://wll:b"), br#"{"v":2}"#.to_vec());
    tx.put_object(oid("obs://wll:c"), br#"{"v":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             LAG(PAYLOAD_FIELD('v'), 1) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS prev, \
             LEAD(PAYLOAD_FIELD('v'), 1) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS nxt \
             FROM 'obs://wll'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));

    // Middle row (v=2): prev=1, next=3.
    assert_eq!(rows[1].get("prev").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(rows[1].get("nxt").and_then(|v| v.as_i64()), Some(3));
    store.close().unwrap();
}

#[test]
fn window_lead_different_field() {
    // LEAD can reference a different field than the one in SELECT.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://wldf:a"),
        br#"{"rank":1,"name":"alice"}"#.to_vec(),
    );
    tx.put_object(oid("obs://wldf:b"), br#"{"rank":2,"name":"bob"}"#.to_vec());
    tx.put_object(
        oid("obs://wldf:c"),
        br#"{"rank":3,"name":"carol"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('rank'), \
             LEAD(PAYLOAD_FIELD('name'), 1) OVER (ORDER BY PAYLOAD_FIELD('rank') ASC) AS next_name \
             FROM 'obs://wldf'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("rank").and_then(|v| v.as_i64()).unwrap_or(0));

    assert_eq!(
        rows[0].get("next_name").and_then(|v| v.as_str()),
        Some("bob")
    );
    assert_eq!(
        rows[1].get("next_name").and_then(|v| v.as_str()),
        Some("carol")
    );
    assert_eq!(rows[2].get("next_name"), Some(&serde_json::Value::Null));
    store.close().unwrap();
}

// ── Aggregate window functions ──────────────────────────────────────────────

#[test]
fn window_sum_full_partition() {
    // SUM with no ORDER BY → every row gets the partition total.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wsf:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wsf:b"), br#"{"score":20}"#.to_vec());
    tx.put_object(oid("obs://wsf:c"), br#"{"score":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), \
             SUM(PAYLOAD_FIELD('score')) OVER () AS total \
             FROM 'obs://wsf'",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("total").and_then(|v| v.as_f64()), Some(60.0));
    }
    store.close().unwrap();
}

#[test]
fn window_sum_running() {
    // SUM with ORDER BY → running cumulative sum.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wsr:a"), br#"{"n":1}"#.to_vec());
    tx.put_object(oid("obs://wsr:b"), br#"{"n":2}"#.to_vec());
    tx.put_object(oid("obs://wsr:c"), br#"{"n":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('n'), \
             SUM(PAYLOAD_FIELD('n')) OVER (ORDER BY PAYLOAD_FIELD('n') ASC) AS running \
             FROM 'obs://wsr'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("n").and_then(|v| v.as_i64()).unwrap_or(0));
    assert_eq!(rows[0].get("running").and_then(|v| v.as_f64()), Some(1.0));
    assert_eq!(rows[1].get("running").and_then(|v| v.as_f64()), Some(3.0));
    assert_eq!(rows[2].get("running").and_then(|v| v.as_f64()), Some(6.0));
    store.close().unwrap();
}

#[test]
fn window_avg_full_partition() {
    // AVG with no ORDER BY → every row gets the partition average.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wav:a"), br#"{"x":10}"#.to_vec());
    tx.put_object(oid("obs://wav:b"), br#"{"x":20}"#.to_vec());
    tx.put_object(oid("obs://wav:c"), br#"{"x":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('x'), \
             AVG(PAYLOAD_FIELD('x')) OVER () AS avg_x \
             FROM 'obs://wav'",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("avg_x").and_then(|v| v.as_f64()), Some(20.0));
    }
    store.close().unwrap();
}

#[test]
fn window_count_star() {
    // COUNT(*) OVER () → every row gets total row count.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wcs:a"), br#"{"k":1}"#.to_vec());
    tx.put_object(oid("obs://wcs:b"), br#"{"k":2}"#.to_vec());
    tx.put_object(oid("obs://wcs:c"), br#"{"k":3}"#.to_vec());
    tx.put_object(oid("obs://wcs:d"), br#"{"k":4}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('k'), \
             COUNT(*) OVER () AS cnt \
             FROM 'obs://wcs'",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("cnt").and_then(|v| v.as_f64()), Some(4.0));
    }
    store.close().unwrap();
}

#[test]
fn window_min_max() {
    // MIN and MAX over full partition.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wmm:a"), br#"{"v":5}"#.to_vec());
    tx.put_object(oid("obs://wmm:b"), br#"{"v":2}"#.to_vec());
    tx.put_object(oid("obs://wmm:c"), br#"{"v":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             MIN(PAYLOAD_FIELD('v')) OVER () AS lo, \
             MAX(PAYLOAD_FIELD('v')) OVER () AS hi \
             FROM 'obs://wmm'",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("lo").and_then(|v| v.as_f64()), Some(2.0));
        assert_eq!(r.get("hi").and_then(|v| v.as_f64()), Some(8.0));
    }
    store.close().unwrap();
}

#[test]
fn window_sum_partition_by() {
    // SUM OVER (PARTITION BY group) → each group gets its own total.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wsp:a"), br#"{"grp":"x","v":1}"#.to_vec());
    tx.put_object(oid("obs://wsp:b"), br#"{"grp":"x","v":2}"#.to_vec());
    tx.put_object(oid("obs://wsp:c"), br#"{"grp":"y","v":10}"#.to_vec());
    tx.put_object(oid("obs://wsp:d"), br#"{"grp":"y","v":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('grp'), PAYLOAD_FIELD('v'), \
             SUM(PAYLOAD_FIELD('v')) OVER (PARTITION BY PAYLOAD_FIELD('grp')) AS grp_total \
             FROM 'obs://wsp'",
        )
        .unwrap();

    for r in &rows {
        let grp = r.get("grp").and_then(|v| v.as_str()).unwrap_or("");
        let total = r.get("grp_total").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if grp == "x" {
            assert_eq!(total, 3.0);
        } else {
            assert_eq!(total, 30.0);
        }
    }
    store.close().unwrap();
}

// ── Named window functions ──────────────────────────────────────────────────

#[test]
fn named_window_row_number() {
    // WINDOW w AS (...) reused by ROW_NUMBER() OVER w.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwrn:a"), br#"{"grp":"x","v":3}"#.to_vec());
    tx.put_object(oid("obs://nwrn:b"), br#"{"grp":"x","v":1}"#.to_vec());
    tx.put_object(oid("obs://nwrn:c"), br#"{"grp":"x","v":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), ROW_NUMBER() OVER w AS rn \
             FROM 'obs://nwrn' \
             WINDOW w AS (ORDER BY PAYLOAD_FIELD('v') ASC)",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    assert_eq!(rows[0].get("rn").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(rows[1].get("rn").and_then(|v| v.as_i64()), Some(2));
    assert_eq!(rows[2].get("rn").and_then(|v| v.as_i64()), Some(3));
    store.close().unwrap();
}

#[test]
fn named_window_shared_across_two_functions() {
    // Same named window referenced by both ROW_NUMBER and SUM.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nws2:a"), br#"{"v":10}"#.to_vec());
    tx.put_object(oid("obs://nws2:b"), br#"{"v":20}"#.to_vec());
    tx.put_object(oid("obs://nws2:c"), br#"{"v":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             ROW_NUMBER() OVER w AS rn, \
             SUM(PAYLOAD_FIELD('v')) OVER w AS running \
             FROM 'obs://nws2' \
             WINDOW w AS (ORDER BY PAYLOAD_FIELD('v') ASC)",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    // rn: 1, 2, 3
    assert_eq!(rows[0].get("rn").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(rows[2].get("rn").and_then(|v| v.as_i64()), Some(3));
    // running sum: 10, 30, 60
    assert_eq!(rows[0].get("running").and_then(|v| v.as_f64()), Some(10.0));
    assert_eq!(rows[1].get("running").and_then(|v| v.as_f64()), Some(30.0));
    assert_eq!(rows[2].get("running").and_then(|v| v.as_f64()), Some(60.0));
    store.close().unwrap();
}

#[test]
fn named_window_partition_by() {
    // Named window with PARTITION BY — SUM resets per group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwpb:a"), br#"{"g":"a","v":1}"#.to_vec());
    tx.put_object(oid("obs://nwpb:b"), br#"{"g":"a","v":2}"#.to_vec());
    tx.put_object(oid("obs://nwpb:c"), br#"{"g":"b","v":10}"#.to_vec());
    tx.put_object(oid("obs://nwpb:d"), br#"{"g":"b","v":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('g'), PAYLOAD_FIELD('v'), \
             SUM(PAYLOAD_FIELD('v')) OVER w AS grp_total \
             FROM 'obs://nwpb' \
             WINDOW w AS (PARTITION BY PAYLOAD_FIELD('g'))",
        )
        .unwrap();
    for r in &rows {
        let g = r.get("g").and_then(|v| v.as_str()).unwrap_or("");
        let t = r.get("grp_total").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if g == "a" {
            assert_eq!(t, 3.0);
        } else {
            assert_eq!(t, 30.0);
        }
    }
    store.close().unwrap();
}

#[test]
fn named_window_multiple_definitions() {
    // Two different named windows in one WINDOW clause.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwmd:a"), br#"{"g":"x","v":5}"#.to_vec());
    tx.put_object(oid("obs://nwmd:b"), br#"{"g":"x","v":3}"#.to_vec());
    tx.put_object(oid("obs://nwmd:c"), br#"{"g":"y","v":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('g'), PAYLOAD_FIELD('v'), \
             ROW_NUMBER() OVER w1 AS rn, \
             SUM(PAYLOAD_FIELD('v')) OVER w2 AS grp_sum \
             FROM 'obs://nwmd' \
             WINDOW w1 AS (ORDER BY PAYLOAD_FIELD('v') ASC), \
                    w2 AS (PARTITION BY PAYLOAD_FIELD('g'))",
        )
        .unwrap();
    // rn is global across all rows by v order
    let mut by_v = rows.clone();
    by_v.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    assert_eq!(by_v[0].get("rn").and_then(|v| v.as_i64()), Some(1)); // v=3
    assert_eq!(by_v[1].get("rn").and_then(|v| v.as_i64()), Some(2)); // v=5
    assert_eq!(by_v[2].get("rn").and_then(|v| v.as_i64()), Some(3)); // v=8
                                                                     // grp_sum: group x = 8, group y = 8
    for r in &rows {
        let g = r.get("g").and_then(|v| v.as_str()).unwrap_or("");
        let s = r.get("grp_sum").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if g == "x" {
            assert_eq!(s, 8.0);
        } else {
            assert_eq!(s, 8.0); // y group has only v=8
        }
    }
    store.close().unwrap();
}

#[test]
fn named_window_unknown_name_errors() {
    // Referencing an undefined window name returns an error.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwun:a"), br#"{"v":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let result =
        store.sql_project("SELECT ROW_NUMBER() OVER missing_window AS rn FROM 'obs://nwun'");
    assert!(result.is_err());
    store.close().unwrap();
}

#[test]
fn named_window_count_star() {
    // COUNT(*) OVER named window gives total row count.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwcs:a"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("obs://nwcs:b"), br#"{"v":2}"#.to_vec());
    tx.put_object(oid("obs://nwcs:c"), br#"{"v":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), COUNT(*) OVER w AS cnt \
             FROM 'obs://nwcs' \
             WINDOW w AS ()",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("cnt").and_then(|v| v.as_f64()), Some(3.0));
    }
    store.close().unwrap();
}

#[test]
fn sql_case_when_basic() {
    // SELECT CASE PAYLOAD_FIELD('role') WHEN 'admin' THEN 'A' WHEN 'user' THEN 'U' ELSE 'X' END AS tier
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scwb:a"), br#"{"role":"admin"}"#.to_vec());
    tx.put_object(oid("obs://scwb:b"), br#"{"role":"user"}"#.to_vec());
    tx.put_object(oid("obs://scwb:c"), br#"{"role":"guest"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT CASE PAYLOAD_FIELD('role') \
             WHEN 'admin' THEN 'A' WHEN 'user' THEN 'U' ELSE 'X' END AS tier \
             FROM 'obs://scwb'",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    let mut tiers: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            r.get("tier")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    tiers.sort();
    assert_eq!(tiers, vec!["A", "U", "X"]);
    store.close().unwrap();
}

#[test]
fn sql_case_when_no_else() {
    // Without ELSE, unmatched rows receive empty string.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scwne:a"), br#"{"status":"active"}"#.to_vec());
    tx.put_object(oid("obs://scwne:b"), br#"{"status":"inactive"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT CASE PAYLOAD_FIELD('status') WHEN 'active' THEN 'yes' END AS flag \
             FROM 'obs://scwne'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let yes = rows
        .iter()
        .filter(|r| r.get("flag").and_then(|v| v.as_str()) == Some("yes"))
        .count();
    assert_eq!(yes, 1);
    store.close().unwrap();
}

#[test]
fn sql_case_when_combined_with_field() {
    // CASE alongside a regular PAYLOAD_FIELD projection.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://scwcf:a"),
        br#"{"name":"alice","role":"admin"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://scwcf:b"),
        br#"{"name":"bob","role":"user"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name'), \
             CASE PAYLOAD_FIELD('role') WHEN 'admin' THEN 'elevated' ELSE 'standard' END AS access \
             FROM 'obs://scwcf'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let access = r.get("access").and_then(|v| v.as_str()).unwrap_or("");
        if name == "alice" {
            assert_eq!(access, "elevated");
        } else if name == "bob" {
            assert_eq!(access, "standard");
        }
    }
    store.close().unwrap();
}

#[test]
fn window_nth_value_basic() {
    // NTH_VALUE returns the nth row's field value from the partition-ordered sequence.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 10), ("b", 30), ("c", 20), ("d", 40)] {
        tx.put_object(
            oid(&format!("obs://wnvb:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             NTH_VALUE(PAYLOAD_FIELD('v'), 2) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS nth2 \
             FROM 'obs://wnvb'",
        )
        .unwrap();
    // 2nd value in ascending order is v=20; all rows share it.
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.get("nth2").and_then(|v| v.as_i64()), Some(20));
    }
    store.close().unwrap();
}

#[test]
fn window_nth_value_out_of_bounds() {
    // NTH_VALUE returns null when n exceeds the partition size.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wnvob:a"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("obs://wnvob:b"), br#"{"v":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             NTH_VALUE(PAYLOAD_FIELD('v'), 5) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS nth5 \
             FROM 'obs://wnvob'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(r.get("nth5").is_none_or(|v| v.is_null()));
    }
    store.close().unwrap();
}

#[test]
fn window_nth_value_partition_by() {
    // NTH_VALUE with PARTITION BY resets per group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wnvp:a"), br#"{"g":"x","v":1}"#.to_vec());
    tx.put_object(oid("obs://wnvp:b"), br#"{"g":"x","v":5}"#.to_vec());
    tx.put_object(oid("obs://wnvp:c"), br#"{"g":"y","v":10}"#.to_vec());
    tx.put_object(oid("obs://wnvp:d"), br#"{"g":"y","v":50}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('g'), PAYLOAD_FIELD('v'), \
             NTH_VALUE(PAYLOAD_FIELD('v'), 2) \
             OVER (PARTITION BY PAYLOAD_FIELD('g') ORDER BY PAYLOAD_FIELD('v') ASC) AS nth2 \
             FROM 'obs://wnvp'",
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    for r in &rows {
        let g = r.get("g").and_then(|v| v.as_str()).unwrap_or("");
        let nth2 = r.get("nth2").and_then(|v| v.as_i64()).unwrap_or(-1);
        // 2nd in group x (v=1,5) is 5; 2nd in group y (v=10,50) is 50
        if g == "x" {
            assert_eq!(nth2, 5, "group x 2nd");
        } else {
            assert_eq!(nth2, 50, "group y 2nd");
        }
    }
    store.close().unwrap();
}

#[test]
fn window_ntile_basic() {
    // NTILE(3) over 6 ordered rows assigns equal-size bucket numbers 1,1,2,2,3,3.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [
        ("a", 10),
        ("b", 20),
        ("c", 30),
        ("d", 40),
        ("e", 50),
        ("f", 60),
    ] {
        tx.put_object(
            oid(&format!("obs://ntb:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             NTILE(3) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS bucket \
             FROM 'obs://ntb'",
        )
        .unwrap();
    let mut sorted = rows.clone();
    sorted.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    // buckets: [1,1,2,2,3,3] for v=[10,20,30,40,50,60]
    let expected = [1i64, 1, 2, 2, 3, 3];
    for (r, &exp) in sorted.iter().zip(expected.iter()) {
        assert_eq!(r.get("bucket").and_then(|v| v.as_i64()), Some(exp));
    }
    store.close().unwrap();
}

#[test]
fn window_first_value() {
    // FIRST_VALUE returns the value from the first row in the partition ordering.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 3), ("b", 1), ("c", 4), ("d", 2)] {
        tx.put_object(
            oid(&format!("obs://wfv:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             FIRST_VALUE(PAYLOAD_FIELD('v')) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS fv \
             FROM 'obs://wfv'",
        )
        .unwrap();
    // All rows share FIRST_VALUE = 1 (smallest v after ORDER BY ASC).
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.get("fv").and_then(|v| v.as_i64()), Some(1));
    }
    store.close().unwrap();
}

#[test]
fn window_last_value() {
    // LAST_VALUE returns the value from the last row in the partition ordering.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 3), ("b", 1), ("c", 4), ("d", 2)] {
        tx.put_object(
            oid(&format!("obs://wlv:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             LAST_VALUE(PAYLOAD_FIELD('v')) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS lv \
             FROM 'obs://wlv'",
        )
        .unwrap();
    // All rows share LAST_VALUE = 4 (largest v after ORDER BY ASC).
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.get("lv").and_then(|v| v.as_i64()), Some(4));
    }
    store.close().unwrap();
}

#[test]
fn window_percent_rank() {
    // PERCENT_RANK over 4 distinct-value rows: 0.0, 1/3, 2/3, 1.0.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 10), ("b", 20), ("c", 30), ("d", 40)] {
        tx.put_object(
            oid(&format!("obs://wpr:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             PERCENT_RANK() OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS pr \
             FROM 'obs://wpr'",
        )
        .unwrap();
    let mut sorted = rows.clone();
    sorted.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    let eps = 1e-9;
    let expected = [0.0f64, 1.0 / 3.0, 2.0 / 3.0, 1.0];
    for (r, &exp) in sorted.iter().zip(expected.iter()) {
        let pr = r.get("pr").and_then(|v| v.as_f64()).unwrap_or(-1.0);
        assert!((pr - exp).abs() < eps, "expected {exp}, got {pr}");
    }
    store.close().unwrap();
}

#[test]
fn window_cume_dist_with_ties() {
    // CUME_DIST: ties share the same value; rows with v=20 both get 3/4.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 10), ("b", 20), ("c", 20), ("d", 40)] {
        tx.put_object(
            oid(&format!("obs://wcd:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             CUME_DIST() OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS cd \
             FROM 'obs://wcd'",
        )
        .unwrap();
    let mut sorted = rows.clone();
    sorted.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    let eps = 1e-9;
    // v=10 → 1/4, v=20 (×2) → 3/4, v=40 → 4/4
    let cd0 = sorted[0].get("cd").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    let cd1 = sorted[1].get("cd").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    let cd2 = sorted[2].get("cd").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    let cd3 = sorted[3].get("cd").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    assert!((cd0 - 0.25).abs() < eps, "got {cd0}");
    assert!((cd1 - 0.75).abs() < eps, "got {cd1}");
    assert!((cd2 - 0.75).abs() < eps, "got {cd2}");
    assert!((cd3 - 1.0).abs() < eps, "got {cd3}");
    store.close().unwrap();
}

#[test]
fn window_first_last_with_partition_by() {
    // FIRST_VALUE and LAST_VALUE with PARTITION BY reset per group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wflp:a"), br#"{"g":"x","v":1}"#.to_vec());
    tx.put_object(oid("obs://wflp:b"), br#"{"g":"x","v":5}"#.to_vec());
    tx.put_object(oid("obs://wflp:c"), br#"{"g":"y","v":10}"#.to_vec());
    tx.put_object(oid("obs://wflp:d"), br#"{"g":"y","v":50}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('g'), PAYLOAD_FIELD('v'), \
             FIRST_VALUE(PAYLOAD_FIELD('v')) OVER (PARTITION BY PAYLOAD_FIELD('g') ORDER BY PAYLOAD_FIELD('v') ASC) AS fv, \
             LAST_VALUE(PAYLOAD_FIELD('v')) OVER (PARTITION BY PAYLOAD_FIELD('g') ORDER BY PAYLOAD_FIELD('v') ASC) AS lv \
             FROM 'obs://wflp'",
        )
        .unwrap();
    for r in &rows {
        let g = r.get("g").and_then(|v| v.as_str()).unwrap_or("");
        let fv = r.get("fv").and_then(|v| v.as_i64()).unwrap_or(-1);
        let lv = r.get("lv").and_then(|v| v.as_i64()).unwrap_or(-1);
        if g == "x" {
            assert_eq!(fv, 1, "group x first");
            assert_eq!(lv, 5, "group x last");
        } else {
            assert_eq!(fv, 10, "group y first");
            assert_eq!(lv, 50, "group y last");
        }
    }
    store.close().unwrap();
}
