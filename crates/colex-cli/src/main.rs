//! Command line tools for the COLex format.

use std::process::ExitCode;

use colex_write::Options;

mod build;
mod corpus;
mod info;
mod search;
mod verify;

use info::Error;

const USAGE: &str = "\
colex - a cloud-optimized concordance format

usage:
    colex build <input.tsv> <output.colex> [options]
        --terms-per-block <n>          terms per dictionary block (default 128)
        --docs-per-payload-block <n>   documents per payload block (default 64)
        --no-freqs                     omit term frequencies
        --generation <n>               generation number (default 1)

    colex info <file>                  print the header and section table
    colex search <file> <term>...      intersect terms and report the cost
    colex verify <file> [--source <p>] check the whole file, and its provenance

    colex --version
    colex --help

The input to `build` is one document per line: text, a tab, then the payload
to return for it.
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
        ["build", input, output, flags @ ..] => build_command(input, output, flags),
        ["verify", path, flags @ ..] => verify_command(path, flags),
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

fn build_command(input: &str, output: &str, flags: &[&str]) -> Result<(), Error> {
    let mut options = Options::default();
    let mut generation = 1u64;
    let mut rest = flags;

    while let Some((flag, tail)) = rest.split_first() {
        rest = tail;
        match *flag {
            "--no-freqs" => options.has_freqs = false,
            "--terms-per-block" => options.terms_per_block = take_number(flag, &mut rest)? as usize,
            "--docs-per-payload-block" => {
                options.docs_per_payload_block = u32::try_from(take_number(flag, &mut rest)?)
                    .map_err(|_| Error::Usage(format!("{flag} is too large")))?;
            }
            "--generation" => generation = take_number(flag, &mut rest)?,
            other => return Err(Error::Usage(format!("unknown option '{other}'"))),
        }
    }
    build::run(input.as_ref(), output.as_ref(), options, generation)
}

fn verify_command(path: &str, flags: &[&str]) -> Result<(), Error> {
    let source = match flags {
        [] => None,
        ["--source", source] => Some(*source),
        _ => return Err(Error::Usage("verify takes at most --source <path>".into())),
    };
    verify::run(path.as_ref(), source.map(AsRef::as_ref))
}

fn take_number(flag: &str, rest: &mut &[&str]) -> Result<u64, Error> {
    let (value, tail) = rest
        .split_first()
        .ok_or_else(|| Error::Usage(format!("{flag} needs a value")))?;
    *rest = tail;
    value
        .parse()
        .map_err(|_| Error::Usage(format!("{flag} needs a number, not '{value}'")))
}
