//! Reports which languages' build tests and interop harnesses can run on this
//! machine, and how to install what's missing (dgtlrift/cddlc#169).
//!
//! ```text
//! cddlc-toolchains                      every language: ready, or what to install
//! cddlc-toolchains --check <lang>       exit 0 if <lang> can run, else 1 (report on stderr)
//! cddlc-toolchains --registry-log <file> exit 0 (and print the line) if the log shows a
//!                                        package registry was unavailable, else 1
//! ```
//! `scripts/interop.sh` uses the last two.

use std::process::ExitCode;

use backend_buildtest::toolchain::{self, Lang};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => {
            for lang in Lang::ALL {
                match toolchain::probe(lang) {
                    Ok(()) => println!("{:<7} ready", lang.name()),
                    Err(m) => println!("{:<7} {}\n{m}", lang.name(), m.short()),
                }
            }
            ExitCode::SUCCESS
        }
        ["--check", name] => match Lang::from_name(name) {
            None => {
                eprintln!(
                    "unknown language '{name}': one of {}",
                    Lang::ALL.map(|l| l.name()).join(", ")
                );
                ExitCode::from(2)
            }
            Some(lang) => match toolchain::probe(lang) {
                Ok(()) => ExitCode::SUCCESS,
                Err(m) => {
                    eprintln!("{m}");
                    ExitCode::from(1)
                }
            },
        },
        ["--registry-log", file] => {
            let log = std::fs::read_to_string(file).unwrap_or_default();
            match toolchain::registry_unavailable(&log) {
                Some(line) => {
                    println!("{line}");
                    ExitCode::SUCCESS
                }
                None => ExitCode::from(1),
            }
        }
        _ => {
            eprintln!("usage: cddlc-toolchains [--check <lang> | --registry-log <file>]");
            ExitCode::from(2)
        }
    }
}
