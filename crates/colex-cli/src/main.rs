//! Command line tools for the COLex format.
//!
//! `info` is here first because it is what makes the format inspectable while
//! the rest of the reader is being written: it reports what the header claims,
//! rather than what an index built by this workspace is assumed to contain.

use std::process::ExitCode;

mod info;
mod search;

const USAGE: &str = "\
colex - a cloud-optimized concordance format

usage:
    colex info <file>            print the header and section table
    colex search <file> <term>...  intersect terms and report the cost
    colex --version       print the version
    colex --help          print this message
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match args.as_slice() {
        [] | ["--help"] | ["-h"] | ["help"] => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        ["--version"] | ["-V"] => {
            println!("colex {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        ["info", path] => info::run(path.as_ref()),
        ["search", path, terms @ ..] => search::run(path.as_ref(), terms),
        [cmd, ..] => {
            eprintln!("colex: unknown command '{cmd}'\n");
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("colex: {err}");
            ExitCode::FAILURE
        }
    }
}
