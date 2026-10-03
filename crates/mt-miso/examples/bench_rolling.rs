//! How long the rolling five-minute feed takes to turn into a store, at full
//! size: ~2,600 CP nodes over a whole day (~33 MB of JSON, like MISO's feed
//! late in the day). Synthetic data in the real format, so it runs offline.
//!
//!     cargo run --release -p mt-miso --example bench_rolling
//!
//! Prints the time for each stage: JSON parse into rows, building the
//! columnar store, and the save/restore round trip used across restarts.

use std::fmt::Write as _;
use std::time::Instant;

use mt_core::RtIntraday;
use mt_miso::parse::parse_rt_five_min;

const NODES: usize = 2_600;
const INTERVALS: usize = 204; // 00:00 to 16:55

fn synthetic_feed() -> String {
    let mut s = String::with_capacity(40 << 20);
    s.push_str(r#"{"data":["#);
    let mut first = true;
    for i in 0..INTERVALS {
        let (h, m) = (i / 12, (i % 12) * 5);
        for n in 0..NODES {
            if !first {
                s.push(',');
            }
            first = false;
            let lmp = 20.0 + ((i * 7 + n * 13) % 400) as f64 / 10.0;
            let _ = write!(
                s,
                r#"["2026-10-02T{h:02}:{m:02}:00","NODE.{n:04}","{lmp:.2}","{:.2}","{:.2}"]"#,
                lmp / 20.0 - 1.0,
                lmp / 50.0 - 0.4
            );
        }
    }
    s.push_str(r#"],"headers":["INTERVAL","CPNODE","LMP","MCC","MLC"]}"#);
    s
}

fn main() {
    let body = synthetic_feed();
    let mb = body.len() as f64 / 1_048_576.0;
    println!("feed: {mb:.1} MB, {} rows", NODES * INTERVALS);

    let t = Instant::now();
    let rows = parse_rt_five_min(&body).expect("parses");
    let parse = t.elapsed();
    assert_eq!(rows.len(), NODES * INTERVALS);

    let t = Instant::now();
    let store = RtIntraday::from_rows(rows);
    let build = t.elapsed();

    let t = Instant::now();
    let bytes = store.to_bytes();
    let save = t.elapsed();
    let t = Instant::now();
    let back = RtIntraday::from_bytes(&bytes).expect("round-trips");
    let restore = t.elapsed();
    assert_eq!(back.intervals().len(), INTERVALS);

    println!(
        "parse {parse:>9.1?}  ({:.0} MB/s)",
        mb / parse.as_secs_f64()
    );
    println!("build {build:>9.1?}");
    println!(
        "save  {save:>9.1?}  ({:.1} MB store)",
        bytes.len() as f64 / 1_048_576.0
    );
    println!("load  {restore:>9.1?}");
}
