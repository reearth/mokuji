//! Command line tools for the Mokuji format.

use std::path::Path;
use std::process::ExitCode;

use mokuji_write::{Options, PayloadEncoding};

use mokuji_cli::corpus::Tokenizer;
use mokuji_cli::info::{self, Error};
use mokuji_cli::{build, dump, prefix, search, verify};

const USAGE: &str = "\
mokuji - a cloud-optimized concordance format

usage:
    mokuji build <input.tsv> <output.mokuji> [options]
        --terms-per-block <n>          terms per dictionary block (default 128)
        --docs-per-payload-block <n>   documents per payload block (default 64)
        --no-freqs                     omit term frequencies
        --payload <how>                text (default), or ints:<decimals> to
                                       store the payload as delta-coded fixed
                                       point numbers
        --tokenizer <id>               space-split (default) or identity
        --source <path>                record this file's hash as a source too;
                                       repeatable, for the register a corpus
                                       was converted from
        --generation <n>               generation number (default 1)

    mokuji dump <file>                  write the index back out as a corpus
    mokuji info <file>                  print the header and section table
    mokuji search <file> <term>... [-n <limit>]
                                       intersect terms, rank them if the file
                                       carries document lengths, and report the
                                       cost
    mokuji prefix <file> <prefix> [-n <limit>] [--resident]
                                       complete a prefix; the query a
                                       hierarchical key such as an address is
                                       for. --resident holds the whole
                                       dictionary, so a lookup costs no read
    mokuji verify <file> [--source <p>] check the whole file, and its provenance

    mokuji --version
    mokuji --help

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
            println!("mokuji {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        ["dump", path] => dump::run(path.as_ref()),
        ["info", path] => info::run(path.as_ref()),
        ["search", path, terms @ ..] => search_command(path, terms),
        ["prefix", path, needle, flags @ ..] => prefix_command(path, needle, flags),
        ["build", input, output, flags @ ..] => build_command(input, output, flags),
        ["verify", path, flags @ ..] => verify_command(path, flags),
        [cmd, ..] => {
            eprintln!("mokuji: unknown command '{cmd}'\n");
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mokuji: {err}");
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
            "--payload" => {
                let (how, tail) = rest
                    .split_first()
                    .ok_or_else(|| Error::Usage("--payload needs a value".into()))?;
                rest = tail;
                options.payload = match how.split_once(':') {
                    Some(("ints", decimals)) => {
                        let decimals: u8 = decimals.parse().map_err(|_| {
                            Error::Usage(format!("'{decimals}' is not a number of decimals"))
                        })?;
                        if decimals > 18 {
                            return Err(Error::Usage("at most 18 decimals".into()));
                        }
                        PayloadEncoding::Ints {
                            // Counted from the corpus once it is read.
                            values_per_doc: 0,
                            scale_exponent: -(decimals as i8),
                        }
                    }
                    _ if *how == "text" => PayloadEncoding::Opaque,
                    _ => {
                        return Err(Error::Usage(format!(
                            "'{how}' is not a payload encoding; use text or ints:<decimals>"
                        )))
                    }
                };
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

fn search_command(path: &str, args: &[&str]) -> Result<(), Error> {
    let mut limit = 10;
    let mut terms: Vec<&str> = Vec::new();
    let mut rest = args;
    while let Some((arg, tail)) = rest.split_first() {
        rest = tail;
        match *arg {
            "-n" | "--limit" => limit = take_number(arg, &mut rest)? as usize,
            other => terms.push(other),
        }
    }
    search::run(path.as_ref(), &terms, limit)
}

fn prefix_command(path: &str, needle: &str, flags: &[&str]) -> Result<(), Error> {
    let mut limit = 20;
    let mut resident = false;
    let mut rest = flags;
    while let Some((flag, tail)) = rest.split_first() {
        rest = tail;
        match *flag {
            "--resident" => resident = true,
            "-n" | "--limit" => limit = take_number(flag, &mut rest)? as usize,
            other => return Err(Error::Usage(format!("unknown option '{other}'"))),
        }
    }
    prefix::run(path.as_ref(), needle, limit, resident)
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
