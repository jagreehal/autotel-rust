//! `cargo ex 04`  runs the exercise in the part whose directory starts with 04.
//! `cargo sol 04` runs our solution to it.
//!
//! The loop this workshop wants is edit, save, look at the trace. A part runs
//! to completion and exits; run it again after each change. With
//! `cargo install cargo-watch`, `cargo watch -x "ex 04"` reruns it on save.

use std::{env, fs, process::Command};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let kind = args.first().map_or("exercise", String::as_str);
    let alias = if kind == "solution" { "sol" } else { "ex" };

    let mut parts: Vec<String> = fs::read_dir(env!("CARGO_MANIFEST_DIR"))
        .expect("workshop directory")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            name.len() > 3 && name[..2].bytes().all(|b| b.is_ascii_digit()) && &name[2..3] == "-"
        })
        .collect();
    parts.sort();

    let Some(requested) = args.get(1) else {
        eprintln!("usage: cargo {alias} <part>\n");
        for part in &parts {
            eprintln!("  {part}");
        }
        std::process::exit(1);
    };

    let prefix = format!("{requested:0>2}");
    let Some(part) = parts.iter().find(|part| part.starts_with(&prefix)) else {
        eprintln!("No part matches \"{requested}\". Available:\n");
        for part in &parts {
            eprintln!("  {part}");
        }
        std::process::exit(1);
    };

    let bin = format!("{alias}{}", &part[..2]);
    println!("▶ workshop/{part}/{kind}.rs\n");

    let status = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["run", "-q", "-p", "workshop", "--bin", &bin])
        .status()
        .expect("cargo runs");
    std::process::exit(status.code().unwrap_or(1));
}
