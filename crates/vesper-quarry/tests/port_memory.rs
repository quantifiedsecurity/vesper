//! The Vesper port frees everything it allocates: after warm-up, repeated
//! DDL replay, prepare, optimize, set_rows, execute and access planning
//! (successful and failing) leave no live port allocation behind. In the
//! Rust build the count is always 0. Its own test binary, so no other test
//! allocates concurrently.
use std::collections::BTreeMap;
use vesper_quarry::{port_live_allocations, DataSet, HermeticCatalog, Limits, Value};

fn cycle() {
    let mut c = HermeticCatalog::default();
    c.apply_ddl(
        "CREATE TABLE p (id I64 NOT NULL, name TEXT); CREATE TABLE o (id I64, owner I64)",
        Limits::default(),
    )
    .unwrap();
    assert!(c
        .apply_ddl("CREATE TABLE p (x I64)", Limits::default())
        .is_err());
    let mut d = DataSet::new(&c);
    let rows: Vec<Vec<Value>> = (0..20)
        .map(|i| vec![Value::I64(i), Value::Text(format!("n{i}"))])
        .collect();
    d.set_rows("p", &rows, Limits::default()).unwrap();
    let orders: Vec<Vec<Value>> = (0..30)
        .map(|i| vec![Value::I64(i), Value::I64(i % 20)])
        .collect();
    d.set_rows("o", &orders, Limits::default()).unwrap();
    assert!(d.set_rows("p", &orders, Limits::default()).is_err());
    let q = c
        .prepare(
            "SELECT p.name, o.id FROM p LEFT JOIN o ON o.owner = p.id WHERE p.id < 5 AND o.id > 2",
            &[],
            Limits::default(),
        )
        .unwrap()
        .optimized();
    let r = q.execute(&c, &d, &BTreeMap::new()).unwrap();
    assert!(!r.rows.is_empty());
    let indexes = BTreeMap::from([("p_id".to_string(), ("p".to_string(), "id".to_string()))]);
    let _ = q.access_plan(&d, &indexes, &BTreeMap::new());
    assert!(c
        .prepare("SELECT nope FROM p", &[], Limits::default())
        .is_err());
}

#[test]
fn the_port_frees_every_allocation() {
    cycle();
    let before = port_live_allocations();
    for _ in 0..200 {
        cycle();
    }
    assert_eq!(
        port_live_allocations(),
        before,
        "the Vesper port leaked over 200 cycles"
    );
}
