//! Example single-node traced task.
//!
//! Local dev loop:      `cargo run -p hello-trace -- local`
//! In a task container: the binary is the container command; the backend passes
//! the standard `a0 --inputs ... --outputs-path ...` args and env.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, flyte::FlyteStruct)]
struct Stats {
    mean: f64,
    count: i64,
    label: String,
}

#[flyte::trace]
async fn double(x: i64) -> Result<i64, flyte::Error> {
    Ok(x * 2)
}

#[flyte::trace]
async fn compute_stats(total: i64, label: String) -> Result<Stats, flyte::Error> {
    Ok(Stats {
        mean: total as f64 / 2.0,
        count: 2,
        label,
    })
}

#[flyte::trace]
async fn describe(stats: Stats) -> Result<String, flyte::Error> {
    Ok(format!(
        "{}: mean={} over {} values",
        stats.label, stats.mean, stats.count
    ))
}

#[flyte::task]
async fn my_task(x: i64, label: String) -> Result<String, flyte::Error> {
    let doubled = double(x).await?;
    let stats = compute_stats(doubled, label).await?;
    describe(stats).await
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("local") => {
            let out = flyte::run_local(my_task(21, "demo".to_string()));
            println!("local result: {out:?}");
            std::process::ExitCode::SUCCESS
        }
        // Smoke-test helpers: write an inputs.pb for this task / decode an outputs.pb.
        Some("write-inputs") => {
            use flyte::idl::Message as _;
            use flyte::FlyteType as _;
            let path = args.get(2).expect("usage: write-inputs <path>");
            let inputs = flyte::types::build_inputs(vec![
                ("x", 21i64.to_literal().unwrap()),
                ("label", "demo".to_string().to_literal().unwrap()),
            ]);
            std::fs::write(path, inputs.encode_to_vec()).expect("write inputs.pb");
            println!("wrote {path}");
            std::process::ExitCode::SUCCESS
        }
        Some("read-outputs") => {
            use flyte::idl::Message as _;
            use flyte::FlyteType as _;
            let path = args.get(2).expect("usage: read-outputs <path>");
            let data = std::fs::read(path).expect("read outputs.pb");
            let outputs = flyte::idl::Outputs::decode(data.as_slice()).expect("decode outputs.pb");
            let lit = flyte::types::output_literal(&outputs, "o0").expect("output o0");
            println!("o0 = {:?}", String::from_literal(lit).expect("decode o0"));
            std::process::ExitCode::SUCCESS
        }
        _ => flyte::worker_main(my_task_entry()),
    }
}
