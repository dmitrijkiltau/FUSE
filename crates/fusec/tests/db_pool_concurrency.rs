use std::collections::HashMap;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fusec::db::Db;
use fusec::interp::Value;

fn temp_db_url() -> String {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("fuse_db_pool_concurrency_{stamp}.sqlite"));
    format!("sqlite://{}", path.display())
}

fn scalar_i64(rows: &[HashMap<String, Value>], key: &str) -> i64 {
    let value = rows.first().and_then(|row| row.get(key));
    match value {
        Some(Value::Int(v)) => *v,
        _ => panic!("expected Int scalar for key {key}, got {value:?}"),
    }
}

fn exec_insert_with_retry(db: &Db, id: i64) -> Result<(), String> {
    for _ in 0..40 {
        match db.exec_params("insert into items (id) values (?)", &[Value::Int(id)]) {
            Ok(()) => return Ok(()),
            Err(err) if err.contains("database is locked") => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(err) => return Err(err),
        }
    }
    Err("timed out waiting for sqlite write lock".to_string())
}

#[test]
fn db_pool_parallel_insert_smoke() {
    let db_url = temp_db_url();
    let setup = Db::open_with_pool(&db_url, 1).expect("open setup db");
    setup
        .exec("create table items (id integer primary key)")
        .expect("create table");

    let threads = 4usize;
    let per_thread = 40usize;
    let mut handles = Vec::with_capacity(threads);

    for thread_idx in 0..threads {
        let db_url = db_url.clone();
        handles.push(thread::spawn(move || {
            let db = Db::open_with_pool(&db_url, 2).expect("open worker db");
            for i in 0..per_thread {
                let id = (thread_idx * 10_000 + i) as i64;
                exec_insert_with_retry(&db, id).expect("insert with retry");
            }
        }));
    }

    for handle in handles {
        handle.join().expect("worker thread panicked");
    }

    let verify = Db::open_with_pool(&db_url, 1).expect("open verify db");
    let rows = verify
        .query("select count(*) as c from items")
        .expect("count rows");
    assert_eq!(scalar_i64(&rows, "c"), (threads * per_thread) as i64);
}
