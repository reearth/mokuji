//! Command line tools for the COLex format.

use std::path::Path;
use std::process::ExitCode;

use colex_write::Options;

use corpus::Tokenizer;

mod build;
mod corpus;
mod info;
mod prefix;
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
        --tokenizer <id>               space-split (default) or identity
        --source <path>                record this file's hash as a source too;
                                       repeatable, for the register a corpus
                                       was converted from
        --generation <n>               generation number (default 1)

    colex info <file>                  print the header and section table
    colex search <file> <term>...      intersect terms and report the cost
    colex prefix <file> <prefix> [-n <limit>]
                                       complete a prefix; the query a
                                       hierarchical key such as an address is
                                       for
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
        ["prefix", path, needle, flags @ ..] => prefix_command(path, needle, flags),
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
    let mut tokenizer = Tokenizer::default();
    let mut generation = 1u64;
    let mut origins: Vec<&str> = Vec::new();
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
            "--source" => {
                let (path, tail) = rest
                    .split_first()
                    .ok_or_else(|| Error::Usage("--source needs a path".into()))?;
                rest = tail;
                origins.push(path);
            }
            "--tokenizer" => {
                let (id, tail) = rest
                    .split_first()
                    .ok_or_else(|| Error::Usage("--tokenizer needs an id".into()))?;
                rest = tail;
                tokenizer = Tokenizer::parse(id).ok_or_else(|| {
                    Error::Usage(format!(
                        "'{id}' is not a registered tokenizer; see TOKENIZERS.md"
                    ))
                })?;
            }
            other => return Err(Error::Usage(format!("unknown option '{other}'"))),
        }
    }
    let origins: Vec<&Path> = origins.iter().map(|p| Path::new(*p)).collect();
    build::run(
        input.as_ref(),
        output.as_ref(),
        &origins,
        options,
        tokenizer,
        generation,
    )
}

fn prefix_command(path: &str, needle: &str, flags: &[&str]) -> Result<(), Error> {
    let limit = match flags {
        [] => 20,
        ["-n", value] | ["--limit", value] => value
            .parse()
            .map_err(|_| Error::Usage(format!("limit needs a number, not '{value}'")))?,
        _ => return Err(Error::Usage("prefix takes at most -n <limit>".into())),
    };
    prefix::run(path.as_ref(), needle.as_bytes(), limit)
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
