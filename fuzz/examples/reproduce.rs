//! Replay saved fuzz inputs on stable Rust, without cargo-fuzz:
//!
//! ```text
//! cargo run --manifest-path fuzz/Cargo.toml --example reproduce -- <target> <file or directory>
//! ```
//!
//! A crash shows up as a panic naming the broken invariant; set
//! `RUST_BACKTRACE=1` for the stack. Exit status: 0 if every input ran
//! cleanly, 101 on a panic, 2 for usage errors.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [target, path] = args.as_slice() else {
        eprintln!(
            "usage: reproduce <target> <file or directory>\ntargets: {}",
            aegis_fuzz::TARGETS.join(", ")
        );
        return ExitCode::from(2);
    };
    match aegis_fuzz::replay(target, Path::new(path)) {
        Ok(n) => {
            eprintln!("{n} input(s) ran cleanly under {target}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("reproduce: {e}");
            ExitCode::from(2)
        }
    }
}
