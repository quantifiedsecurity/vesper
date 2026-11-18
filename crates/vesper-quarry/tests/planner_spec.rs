//! Conformance harness for the Quarry planner specification
//! (`spec/planner-ddl.md`), meant to outlive the Rust executor: the future
//! Vesper-source port of the planner must reproduce `golden.txt` exactly.
//!
//! What this test checks today, in Rust:
//! * every case in `tests/fixtures/planner/cases.txt` parses, its `plan:`
//!   line is well formed and names real aliases, indexes and columns;
//! * the row executor and the vectorized executor agree on every case;
//! * the rendered results (with the expected plans) equal `golden.txt`.
//!
//! Rust has no index-selecting planner, so the `plan:` lines are the
//! contract for the port, not something the Rust code chooses. Set
//! `VESPER_BLESS=1` to rewrite `golden.txt` from the Rust executor. Set
//! `QUARRY_PORT_CMD` to a shell command: it is run with the path of
//! `cases.txt` as its argument and its stdout must equal `golden.txt`
//! byte for byte (plans and rows), which is the differential check the
//! port has to pass.
use std::collections::BTreeMap;
use std::path::PathBuf;
use vesper_quarry::device::SimDevice;
use vesper_quarry::store::{Store, StoreErrorKind, StoreLimits};
use vesper_quarry::vector::VectorEngine;
use vesper_quarry::{ErrorKind, Limits, ParamDef, ScalarType, Value};

const CITIES: [&str; 8] = [
    "ankara", "bergen", "cairo", "delhi", "oslo", "perth", "quito", "rome",
];

/// The data set of the specification (section "Conformance data set").
fn build_store() -> Store<SimDevice> {
    let (mut s, _) = Store::open(SimDevice::new(), StoreLimits::default()).unwrap();
    s.execute_ddl(
        "CREATE TABLE p (id I64 NOT NULL, age I64 NOT NULL, name TEXT NOT NULL, city TEXT); \
         CREATE TABLE o (id I64 NOT NULL, owner I64 NOT NULL, amount I64 NOT NULL); \
         CREATE TABLE c (code TEXT NOT NULL, label TEXT NOT NULL)",
    )
    .unwrap();
    let mut t = s.begin().unwrap();
    for id in 1..=400u64 {
        let city = if id % 13 == 0 {
            Value::Null
        } else {
            Value::Text(CITIES[(id % 8) as usize].into())
        };
        let row = [
            Value::I64(id as i64),
            Value::I64(((id * 7) % 80) as i64),
            Value::Text(format!("p{id}")),
            city,
        ];
        s.insert(&mut t, "p", &row).unwrap();
    }
    for id in 1..=600u64 {
        let row = [
            Value::I64(id as i64),
            Value::I64(((id * 11) % 400 + 1) as i64),
            Value::I64(((id * 37) % 1000) as i64),
        ];
        s.insert(&mut t, "o", &row).unwrap();
    }
    for city in CITIES {
        let row = [Value::Text(city.into()), Value::Text(format!("L-{city}"))];
        s.insert(&mut t, "c", &row).unwrap();
    }
    s.commit(t).unwrap();
    for (name, table, column) in [
        ("p_id", "p", "id"),
        ("p_age", "p", "age"),
        ("p_city", "p", "city"),
        ("o_owner", "o", "owner"),
        ("o_amount", "o", "amount"),
    ] {
        s.create_index(name, table, column).unwrap();
    }
    s
}

struct Case {
    name: String,
    plan: String,
    params: Vec<(ParamDef, Value)>,
    sql: String,
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/planner")
}

fn scalar(name: &str) -> ScalarType {
    match name {
        "U64" => ScalarType::U64,
        "I64" => ScalarType::I64,
        "TEXT" => ScalarType::Text,
        "BOOL" => ScalarType::Bool,
        other => panic!("unknown parameter type {other}"),
    }
}

fn parse_cases(text: &str) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix("== ") {
            cases.push(Case {
                name: name.to_string(),
                plan: String::new(),
                params: Vec::new(),
                sql: String::new(),
            });
            continue;
        }
        let case = cases.last_mut().expect("a line before the first case");
        if let Some(plan) = line.strip_prefix("plan: ") {
            case.plan = plan.to_string();
        } else if let Some(params) = line.strip_prefix("params: ") {
            for p in params.split(',').map(str::trim) {
                let (decl, value) = p.split_once('=').expect("name:TYPE=value");
                let (name, ty) = decl.split_once(':').expect("name:TYPE");
                let ty = scalar(ty);
                let value = if value == "null" {
                    Value::Null
                } else if let Some(t) = value.strip_prefix('\'') {
                    Value::Text(t.strip_suffix('\'').expect("closing quote").into())
                } else if ty == ScalarType::U64 {
                    Value::U64(value.parse().unwrap())
                } else {
                    Value::I64(value.parse().unwrap())
                };
                let def = ParamDef {
                    name: name.to_string(),
                    ty,
                    nullable: true,
                };
                case.params.push((def, value));
            }
        } else {
            assert!(case.sql.is_empty(), "{}: SQL is one line", case.name);
            case.sql = line.to_string();
        }
    }
    for c in &cases {
        assert!(
            !c.plan.is_empty() && !c.sql.is_empty(),
            "{}: incomplete",
            c.name
        );
    }
    cases
}

/// Checks a `plan:` line against the query and the store's indexes.
fn check_plan(case: &Case, indexes: &BTreeMap<String, (String, String)>) {
    let sql = format!(" {} ", case.sql.to_ascii_lowercase());
    for entry in case.plan.split_whitespace() {
        let (alias, path) = entry
            .split_once('=')
            .unwrap_or_else(|| panic!("{}: bad plan entry {entry}", case.name));
        let binds = [" from ", " join "].iter().any(|kw| {
            sql.split(kw).skip(1).any(|rest| {
                let mut words = rest.split_whitespace();
                let table = words.next().unwrap_or("");
                let next = words.next().unwrap_or("");
                table == alias || next == alias
            })
        });
        assert!(binds, "{}: alias {alias} is not bound", case.name);
        if path == "scan" {
            continue;
        }
        let inner = path
            .strip_prefix("index(")
            .or_else(|| path.strip_prefix("index_join("))
            .and_then(|r| r.strip_suffix(')'))
            .unwrap_or_else(|| panic!("{}: bad access path {path}", case.name));
        let (index, arg) = inner.split_once(',').expect("INDEX,ARG");
        assert!(
            indexes.contains_key(index),
            "{}: unknown index {index}",
            case.name
        );
        if path.starts_with("index(") {
            assert!(
                ["eq", "range", "lower", "upper"].contains(&arg),
                "{}: bad bound kind {arg}",
                case.name
            );
            let column = &indexes[index].1;
            assert!(
                sql.contains(&format!(" {column} ")) || sql.contains(&format!(".{column} ")),
                "{}: predicate on {column} missing",
                case.name
            );
        } else {
            let (outer, column) = arg.split_once('.').expect("OUTER.COLUMN");
            assert!(
                case.plan.contains(&format!("{outer}=")),
                "{}: outer alias {outer} not in plan",
                case.name
            );
            assert!(
                sql.contains(&format!("{outer}.{column}")),
                "{}: join column missing",
                case.name
            );
        }
    }
}

fn render_value(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::U64(x) => format!("{x}u"),
        Value::I64(x) => x.to_string(),
        Value::F64(x) => format!("{x:?}"),
        Value::Bool(x) => x.to_string(),
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Bytes(b) => {
            let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
            format!("x'{hex}'")
        }
    }
}

fn render(cases: &[Case], s: &Store<SimDevice>) -> String {
    let engine = VectorEngine::portable();
    let mut out = String::new();
    for case in cases {
        let defs: Vec<ParamDef> = case.params.iter().map(|(d, _)| d.clone()).collect();
        let binds: BTreeMap<String, Value> = case
            .params
            .iter()
            .map(|(d, v)| (d.name.clone(), v.clone()))
            .collect();
        let t = s.begin().unwrap();
        // The vectorized executor (hash joins) is the reference; the row
        // executor (nested loops) must agree wherever it fits its budget.
        let rows = s
            .query_vectorized(&t, &case.sql, &defs, &binds, Limits::default(), &engine)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        match s.query(&t, &case.sql, &defs, &binds, Limits::default()) {
            Ok(by_row) => {
                assert_eq!(by_row.columns, rows.columns, "{}", case.name);
                assert_eq!(by_row.rows, rows.rows, "{}: executors differ", case.name);
            }
            Err(e) => assert_eq!(
                e.kind,
                StoreErrorKind::Sql(ErrorKind::Limit),
                "{}: {e}",
                case.name
            ),
        }
        s.commit(t).unwrap();
        out.push_str(&format!("== {}\nplan: {}\n", case.name, case.plan));
        let cols: Vec<String> = rows
            .columns
            .iter()
            .map(|c| format!("{}:{:?}", c.name, c.ty))
            .collect();
        out.push_str(&format!("columns: {}\n", cols.join(", ")));
        for row in &rows.rows {
            let vals: Vec<String> = row.iter().map(render_value).collect();
            out.push_str(&format!("row: {}\n", vals.join(" | ")));
        }
        out.push_str(&format!("rows: {}\n", rows.rows.len()));
    }
    out
}

#[test]
fn planner_cases_match_golden_results() {
    let dir = fixtures();
    let cases = parse_cases(&std::fs::read_to_string(dir.join("cases.txt")).unwrap());
    assert!(cases.len() >= 20);
    let s = build_store();
    let indexes = s.indexes();
    for case in &cases {
        check_plan(case, &indexes);
    }
    let rendered = render(&cases, &s);
    let golden_path = dir.join("golden.txt");
    if std::env::var_os("VESPER_BLESS").is_some() {
        std::fs::write(&golden_path, &rendered).unwrap();
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap();
    assert_eq!(
        rendered, golden,
        "golden.txt is out of date (VESPER_BLESS=1)"
    );

    if let Some(cmd) = std::env::var_os("QUARRY_PORT_CMD") {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{} \"$0\"", cmd.to_string_lossy()))
            .arg(dir.join("cases.txt"))
            .output()
            .expect("run QUARRY_PORT_CMD");
        assert!(out.status.success(), "port command failed");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            golden,
            "the port's plans or rows differ from golden.txt"
        );
    }
}

#[test]
fn golden_results_equal_filtered_index_lookups() {
    // The rows a single-table indexed case returns must be exactly what the
    // index lookups of its access path return after the residual filter,
    // so "equal results with and without indexes" holds for the golden
    // file. Checked here for the point and range cases through the store's
    // lookup API.
    let s = build_store();
    let t = s.begin().unwrap();
    let ids = |rows: Vec<(u64, Vec<Value>)>| -> Vec<Value> {
        rows.into_iter().map(|(_, r)| r[0].clone()).collect()
    };
    let by_query = |sql: &str| -> Vec<Value> {
        s.query(&t, sql, &[], &BTreeMap::new(), Limits::default())
            .unwrap()
            .rows
            .into_iter()
            .map(|r| r[0].clone())
            .collect()
    };
    assert_eq!(
        ids(s.lookup(&t, "p_age", &Value::I64(30)).unwrap()),
        by_query("SELECT id FROM p WHERE age = 30")
    );
    assert_eq!(
        ids(s
            .lookup_range(&t, "p_id", &Value::I64(395), &Value::I64(i64::MAX))
            .unwrap()),
        by_query("SELECT id FROM p WHERE id >= 395")
    );
    assert_eq!(
        ids(s
            .lookup_range(&t, "o_amount", &Value::I64(990), &Value::I64(i64::MAX))
            .unwrap()),
        by_query("SELECT id FROM o WHERE amount >= 990")
    );
    let mut probe: Vec<Value> = ids(s.lookup(&t, "o_owner", &Value::I64(17)).unwrap());
    probe.sort_by_key(|v| match v {
        Value::I64(x) => *x,
        _ => 0,
    });
    assert_eq!(
        probe,
        by_query("SELECT o.id FROM p JOIN o ON o.owner = p.id WHERE p.id = 17")
    );
    s.commit(t).unwrap();
}

/// A literal of `front_end.txt`: `5`, `5u`, `1.5`, `true`, `null`, `'text'`
/// or `x'0aff'`.
fn literal(s: &str) -> Value {
    let s = s.trim();
    if s == "null" {
        Value::Null
    } else if s == "true" || s == "false" {
        Value::Bool(s == "true")
    } else if let Some(hex) = s.strip_prefix("x'") {
        let hex = hex.strip_suffix('\'').expect("closing quote");
        Value::Bytes(
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect(),
        )
    } else if let Some(t) = s.strip_prefix('\'') {
        Value::Text(
            t.strip_suffix('\'')
                .expect("closing quote")
                .replace("''", "'"),
        )
    } else if let Some(u) = s.strip_suffix('u') {
        Value::U64(u.parse().unwrap())
    } else if s.contains(['.', 'e']) {
        Value::F64(s.parse().unwrap())
    } else {
        Value::I64(s.parse().unwrap())
    }
}

/// `field=N, ..` applied to the default limits.
fn limits_line(s: &str) -> Limits {
    let mut l = Limits::default();
    for part in s.split(',').map(str::trim) {
        let (k, v) = part.split_once('=').expect("field=N");
        let v: usize = v.parse().unwrap();
        match k {
            "sql_bytes" => l.sql_bytes = v,
            "tokens" => l.tokens = v,
            "nodes" => l.nodes = v,
            "nesting" => l.nesting = v,
            "tables" => l.tables = v,
            "columns_per_table" => l.columns_per_table = v,
            "relations" => l.relations = v,
            "input_rows" => l.input_rows = v,
            "input_bytes" => l.input_bytes = v,
            "work" => l.work = v,
            "intermediate_rows" => l.intermediate_rows = v,
            "intermediate_bytes" => l.intermediate_bytes = v,
            "output_rows" => l.output_rows = v,
            "output_bytes" => l.output_bytes = v,
            other => panic!("unknown limit {other}"),
        }
    }
    l
}

fn outcome<T>(r: &vesper_quarry::Result<T>, ok: impl FnOnce(&T) -> String) -> String {
    match r {
        Ok(v) => ok(v),
        Err(e) => format!("error {:?}: {}\n", e.kind, e.message),
    }
}

/// Runs `front_end.txt` and renders every step's outcome.
fn front_end_transcript(text: &str) -> String {
    use vesper_quarry::{DataSet, HermeticCatalog};
    let mut catalog = HermeticCatalog::default();
    let mut data: Option<DataSet> = None;
    let mut limits = Limits::default();
    let mut params: Vec<(ParamDef, Value, bool, bool)> = Vec::new();
    let mut out = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (step, arg) = line.split_once(": ").expect("step: argument");
        match step {
            "limits" => {
                limits = limits_line(arg);
                continue;
            }
            "params" => {
                for p in arg.split(", ") {
                    let (decl, value) = p.split_once('=').expect("name:TYPE=value");
                    let (name, ty) = decl.split_once(':').expect("name:TYPE");
                    let (declared, bound) = (!name.starts_with('+'), !name.ends_with('?'));
                    let name = name.trim_start_matches('+').trim_end_matches('?');
                    let ty = match ty {
                        "F64" => ScalarType::F64,
                        other => scalar(other),
                    };
                    let def = ParamDef {
                        name: name.to_string(),
                        ty,
                        nullable: true,
                    };
                    params.push((def, literal(value), declared, bound));
                }
                continue;
            }
            _ => {}
        }
        out.push_str(&format!("> {line}\n"));
        match step {
            "ddl" => {
                let r = catalog.apply_ddl(arg, limits);
                out.push_str(&outcome(&r, |_| {
                    format!(
                        "ok revision {} tables {:?}\n",
                        catalog.revision(),
                        catalog.tables()
                    )
                }));
            }
            "rows" => {
                let (table, rows) = arg.split_once(' ').expect("TABLE ROWS");
                let rows: Vec<Vec<Value>> = rows
                    .split(';')
                    .map(|r| r.split(", ").map(literal).collect())
                    .collect();
                let d = data.get_or_insert_with(|| DataSet::new(&catalog));
                let r = d.set_rows(table, &rows, limits);
                out.push_str(&outcome(&r, |_| format!("ok {:?}\n", d.tables())));
            }
            "query" => {
                let defs: Vec<ParamDef> =
                    params.iter().filter(|p| p.2).map(|p| p.0.clone()).collect();
                let binds: BTreeMap<String, Value> = params
                    .iter()
                    .filter(|p| p.3)
                    .map(|p| (p.0.name.to_ascii_lowercase(), p.1.clone()))
                    .collect();
                let prepared = catalog.prepare(arg, &defs, limits);
                out.push_str(&outcome(&prepared, |q| {
                    let o = q.optimized();
                    format!(
                        "plan {:?}\noptimized {:?}\ncolumns {:?}\noutput {:?}\nparameters {:?}\n",
                        q.plan(),
                        o.plan(),
                        q.columns(),
                        q.output(),
                        q.parameters()
                    )
                }));
                if let Ok(q) = prepared {
                    let empty = DataSet::new(&catalog);
                    let d = data.as_ref().unwrap_or(&empty);
                    for (name, q) in [("plain", q.clone()), ("optimized", q.optimized())] {
                        let r = q.execute(&catalog, d, &binds);
                        out.push_str(&format!("{name} "));
                        out.push_str(&outcome(&r, |r| {
                            let rows: Vec<String> = r
                                .rows
                                .iter()
                                .map(|row| {
                                    row.iter().map(render_value).collect::<Vec<_>>().join(" | ")
                                })
                                .collect();
                            format!("work {} rows [{}]\n", r.work, rows.join("; "))
                        }));
                    }
                }
                params.clear();
            }
            other => panic!("unknown step {other}"),
        }
        limits = Limits::default();
    }
    out
}

#[test]
fn front_end_transcript_matches_golden() {
    // Plans, types, results, work counters and every error message are part
    // of the API, so the Rust build and the Vesper port must render this
    // transcript identically (VESPER_BLESS=1 rewrites it from the Rust build).
    let dir = fixtures();
    let text = std::fs::read_to_string(dir.join("front_end.txt")).unwrap();
    let rendered = front_end_transcript(&text);
    let golden_path = dir.join("front_end.golden");
    if std::env::var_os("VESPER_BLESS").is_some() && cfg!(not(feature = "vesper-port")) {
        std::fs::write(&golden_path, &rendered).unwrap();
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap();
    let mismatch = rendered
        .lines()
        .zip(golden.lines())
        .position(|(a, b)| a != b);
    assert!(
        rendered == golden,
        "front_end.golden differs at line {:?}:\n{}",
        mismatch.map(|i| i + 1),
        mismatch
            .map(|i| format!(
                "got:  {}\nwant: {}",
                rendered.lines().nth(i).unwrap_or(""),
                golden.lines().nth(i).unwrap_or("")
            ))
            .unwrap_or_default()
    );
}

#[test]
fn planner_chooses_the_golden_access_paths() {
    // The cost model of spec/planner-ddl.md section 2 must choose each
    // case's `plan:` line. Only the Vesper port implements it.
    let dir = fixtures();
    let cases = parse_cases(&std::fs::read_to_string(dir.join("cases.txt")).unwrap());
    let s = build_store();
    let indexes = s.indexes();
    let t = s.begin().unwrap();
    let data = s.dataset(&t, &["c", "o", "p"], Limits::default()).unwrap();
    let mut wrong = Vec::new();
    for case in &cases {
        let defs: Vec<ParamDef> = case.params.iter().map(|(d, _)| d.clone()).collect();
        let binds: BTreeMap<String, Value> = case
            .params
            .iter()
            .map(|(d, v)| (d.name.clone(), v.clone()))
            .collect();
        let q = s
            .catalog()
            .prepare(&case.sql, &defs, Limits::default())
            .unwrap();
        match q.optimized().access_plan(&data, &indexes, &binds) {
            Ok(plan) => {
                if plan.to_string() != case.plan {
                    wrong.push(format!(
                        "{}: chose {plan} ({:?}), want {}",
                        case.name,
                        plan.bindings
                            .iter()
                            .map(|b| (b.rows, b.cost))
                            .collect::<Vec<_>>(),
                        case.plan
                    ));
                }
            }
            Err(e) => {
                // Only the Rust build lacks the access planner.
                let port = cfg!(feature = "vesper-port");
                assert!(!port, "{}: {e}", case.name);
                assert_eq!(e.kind, ErrorKind::Unsupported);
            }
        }
    }
    s.commit(t).unwrap();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A bound parameter's value must not shift the per-value counts of the
/// statistics: with `t.a` always 1, `a = :p` (p = 1) selects every row, so
/// a scan beats the index.
#[test]
fn bound_parameters_keep_statistics_aligned() {
    let mut c = vesper_quarry::HermeticCatalog::default();
    c.apply_ddl("CREATE TABLE t (a I64 NOT NULL)", Limits::default())
        .unwrap();
    let mut d = vesper_quarry::DataSet::new(&c);
    let rows: Vec<Vec<Value>> = (0..100).map(|_| vec![Value::I64(1)]).collect();
    d.set_rows("t", &rows, Limits::default()).unwrap();
    let param = ParamDef {
        name: "p".into(),
        ty: ScalarType::I64,
        nullable: false,
    };
    let q = c
        .prepare("SELECT a FROM t WHERE a = :p", &[param], Limits::default())
        .unwrap()
        .optimized();
    let indexes = BTreeMap::from([("t_a".to_string(), ("t".to_string(), "a".to_string()))]);
    let params = BTreeMap::from([("p".to_string(), Value::I64(1))]);
    match q.access_plan(&d, &indexes, &params) {
        Ok(plan) => {
            assert_eq!(plan.to_string(), "t=scan");
            assert_eq!(plan.bindings[0].rows, 100);
        }
        Err(e) => {
            let port = cfg!(feature = "vesper-port");
            assert!(!port, "{e}");
            assert_eq!(e.kind, ErrorKind::Unsupported);
        }
    }
}

/// Access planning, like execution, refuses data from another schema
/// snapshot.
#[test]
fn access_plan_rejects_a_stale_data_snapshot() {
    let mut c = vesper_quarry::HermeticCatalog::default();
    c.apply_ddl("CREATE TABLE t (a I64, b I64)", Limits::default())
        .unwrap();
    let q = c
        .prepare("SELECT a FROM t WHERE a = 1", &[], Limits::default())
        .unwrap();
    let mut c2 = vesper_quarry::HermeticCatalog::default();
    c2.apply_ddl("CREATE TABLE t (b I64, a I64)", Limits::default())
        .unwrap();
    let d = vesper_quarry::DataSet::new(&c2);
    let e = q
        .access_plan(&d, &BTreeMap::new(), &BTreeMap::new())
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Stale);
}
