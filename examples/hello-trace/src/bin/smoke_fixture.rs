//! Fixture helper for `scripts/smoke.sh`, which needs an `inputs.pb` on disk
//! before it runs the worker and needs to read the `outputs.pb` back — from
//! shell, with no Python and no test harness.
//!
//! Deliberately separate from the example itself: the hardcoded demo values live
//! here so `main.rs` stays a showcase.
//!
//!     smoke_fixture write-inputs <path>
//!     smoke_fixture read-outputs <path>

use flyte::idl::Message as _;
use flyte::FlyteType as _;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("write-inputs"), Some(path)) => {
            // Must match my_task's signature in main.rs; `describe-interface`
            // prints what that is.
            let inputs = flyte::types::build_inputs(vec![
                ("x", 21i64.to_literal().unwrap()),
                ("label", "demo".to_string().to_literal().unwrap()),
            ]);
            std::fs::write(path, inputs.encode_to_vec()).expect("write inputs.pb");
            println!("wrote {path}");
            std::process::ExitCode::SUCCESS
        }
        (Some("read-outputs"), Some(path)) => {
            let data = std::fs::read(path).expect("read outputs.pb");
            let outputs = flyte::idl::Outputs::decode(data.as_slice()).expect("decode outputs.pb");
            let lit = flyte::types::output_literal(&outputs, "o0").expect("output o0");
            println!("o0 = {:?}", String::from_literal(lit).expect("decode o0"));
            std::process::ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: smoke_fixture <write-inputs|read-outputs> <path>");
            std::process::ExitCode::FAILURE
        }
    }
}
