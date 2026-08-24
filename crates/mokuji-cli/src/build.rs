//! `mokuji build` - turn a corpus into an index, recording where it came from.

use std::path::Path;

use mokuji_meta::{
    cbor_map, ContentHash, Generator, Meta, Source, Tokenizer as MetaTokenizer, Value,
};
use mokuji_write::{Builder, Options, PayloadEncoding};
use sha2::{Digest, Sha256};

use crate::corpus::{self, Tokenizer};
use crate::info::Error;

/// Everything a build records about itself, as opposed to what it encodes.
#[derive(Default)]
pub struct Recipe<'a> {
    /// Further files to record as sources. An index is often derived from
    /// something the builder never sees — here, a corpus converted from a
    /// published register — and recording only the file it read would make the
    /// provenance true but useless: it would name an intermediate nobody keeps.
    pub origins: &'a [&'a Path],
    /// Attribute columns to read from the corpus, as (name, decimals).
    pub columns: &'a [(String, u8)],
    /// Application-defined metadata entries.
    pub attrs: &'a [(String, String)],
    /// Generation number.
    pub generation: u64,
}

/// Builds `input` into `output`.
pub fn run(
    input: &Path,
    output: &Path,
    recipe: &Recipe<'_>,
    mut options: Options,
    tokenizer: Tokenizer,
) -> Result<(), Error> {
    let Recipe {
        origins,
        columns: declared,
        attrs,
        generation,
    } = *recipe;
    let raw = std::fs::read(input)?;
    let text = std::str::from_utf8(&raw).map_err(|_| Error::Corpus("input is not UTF-8".into()))?;
    let corpus =
        corpus::parse(text, tokenizer, declared.len(), options.positions).map_err(Error::Corpus)?;

    // Hash the source bytes as they were read, not the parsed corpus. The claim
    // being recorded is "this index was derived from that file", and it is only
    // checkable against the file itself.
    let digest = Sha256::digest(&raw);

    // Worth storing when a document can be longer than one term, since a
    // scorer needs the length of the document in front of it. A guess, and a
    // wrong one for an index whose documents are all the same length — hence
    // the override.
    options.doc_lengths = options.doc_lengths && !tokenizer.is_key();

    // How many values a document carries is a property of the corpus, not
    // something to make the caller count. Taken from the first document and
    // then required of the rest by the builder.
    if let PayloadEncoding::Ints {
        values_per_doc: values_per_doc @ 0,
        scale_exponent,
    } = &mut options.payload
    {
        let first = corpus
            .payloads
            .first()
            .ok_or(Error::Corpus("the corpus is empty".into()))?;
        let text = std::str::from_utf8(first)
            .map_err(|_| Error::Corpus("a payload is not UTF-8".into()))?;
        let count = crate::payload::parse_values(text, scale_exponent.unsigned_abs())?.len();
        *values_per_doc = u8::try_from(count)
            .map_err(|_| Error::Corpus("a document carries too many values".into()))?;
    }

    // A dictionary index earns implied postings only when a term's document ID
    // is the term's own, which means the documents have to be in term order.
    // Requiring the caller to have sorted the corpus that way would be a
    // requirement they cannot check, since the tokenizer decides what the terms
    // are — so the ordering is done here instead.
    let order = dictionary_order(&corpus);
    let mut builder = Builder::new(options)?;
    let identity: Vec<u64> = (0..corpus.payloads.len() as u64).collect();
    let sequence = order.as_ref().unwrap_or(&identity);
    for &doc in sequence {
        let payload = &corpus.payloads[doc as usize];
        match options.payload {
            PayloadEncoding::Opaque => {
                builder.add_document(payload.clone())?;
            }
            PayloadEncoding::Ints { scale_exponent, .. } => {
                let text = std::str::from_utf8(payload)
                    .map_err(|_| Error::Corpus("a payload is not UTF-8".into()))?;
                let values = crate::payload::parse_values(text, scale_exponent.unsigned_abs())?;
                builder.add_document_values(&values)?;
            }
        }
    }

    // Attribute columns follow the documents, reordering and all.
    for (i, (name, decimals)) in declared.iter().enumerate() {
        let raw = &corpus.attributes[i];
        let mut values = Vec::with_capacity(sequence.len());
        for &doc in sequence {
            let field = &raw[doc as usize];
            let parsed = crate::payload::parse_values(field, *decimals)?;
            match parsed.as_slice() {
                [one] => values.push(*one),
                _ => {
                    return Err(Error::Corpus(format!(
                        "attribute column '{name}' holds '{field}', which is not one number"
                    )))
                }
            }
        }
        builder.add_column(mokuji_write::Column {
            name: name.clone(),
            scale_exponent: -(*decimals as i8),
            values,
        })?;
    }

    let renumbered = match &order {
        Some(order) => {
            let mut map = vec![0u64; corpus.payloads.len()];
            for (new, &old) in order.iter().enumerate() {
                map[old as usize] = new as u64;
            }
            map
        }
        None => (0..corpus.payloads.len() as u64).collect(),
    };
    match &corpus.positions {
        Some(positions) => {
            for (term, docs) in positions {
                for (doc, places) in docs {
                    builder.add_posting_at(term, renumbered[*doc as usize], places)?;
                }
            }
        }
        None => {
            for (term, docs) in &corpus.postings {
                for (&doc, &freq) in docs {
                    builder.add_posting(term, renumbered[doc as usize], freq)?;
                }
            }
        }
    }

    let mut sources = vec![Source {
        content_hash: ContentHash::sha2_256(&digest),
        uri: Some(input.display().to_string()),
    }];
    for origin in origins {
        sources.push(Source {
            content_hash: ContentHash::sha2_256(&hash_file(origin)?),
            uri: Some(origin.display().to_string()),
        });
    }

    // Recorded only when there are any: an empty list is noise in a record
    // whose whole job is to be exact.
    let mut params = vec![
        (
            "terms_per_block",
            Value::Uint(options.terms_per_block as u64),
        ),
        (
            "docs_per_payload_block",
            Value::Uint(u64::from(options.docs_per_payload_block)),
        ),
        ("has_freqs", Value::Uint(u64::from(options.has_freqs))),
        ("payload", Value::Text(describe(options.payload))),
        ("doc_lengths", Value::Uint(u64::from(options.doc_lengths))),
        ("positions", Value::Uint(u64::from(options.positions))),
    ];
    if !declared.is_empty() {
        params.push((
            "attributes",
            Value::Array(
                declared
                    .iter()
                    .map(|(name, decimals)| Value::Text(format!("{name}:{decimals}")))
                    .collect(),
            ),
        ));
    }

    let meta = Meta {
        generation,
        sources,
        generator: Generator {
            name: "mokuji".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            // Everything the writer branches on. SPEC.md §3.4 requires these
            // to be sufficient, with the source hash, to rebuild the file.
            params: cbor_map(params),
        },
        tokenizer: MetaTokenizer {
            id: tokenizer.id().into(),
            params: cbor_map::<&str>([]),
        },
        // Application-defined, and recorded only when there is something to
        // record. The format has no opinion about what goes here; a reader
        // that needs a parameter the format does not define — which geohash
        // precisions an index uses, say — has nowhere else to find it.
        attrs: (!attrs.is_empty()).then(|| {
            cbor_map(
                attrs
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::Text(value.clone()))),
            )
        }),
    };

    let bytes = builder.finish(&meta.encode())?;
    std::fs::write(output, &bytes)?;

    println!("{} bytes", bytes.len());
    println!(
        "{} documents, {} terms",
        corpus.payloads.len(),
        corpus.postings.len()
    );
    if corpus.duplicate_keys > 0 {
        println!(
            "{} lines dropped: their key was already taken after {} normalization",
            corpus.duplicate_keys,
            tokenizer.id()
        );
    }
    for source in &meta.sources {
        println!("derived from {}", source.content_hash);
        if let Some(uri) = &source.uri {
            println!("             {uri}");
        }
    }
    Ok(())
}

/// How the payload encoding is recorded in the metadata.
///
/// A reader does not need this — the payload offsets section declares the codec
/// — but a rebuild does. And where the scale is a decision about precision, as
/// it is for coordinates, recording it turns a shortened copy into one that
/// says it was shortened.
fn describe(payload: PayloadEncoding) -> String {
    match payload {
        PayloadEncoding::Opaque => "opaque".to_string(),
        PayloadEncoding::Ints { scale_exponent, .. } => {
            format!("ints:{}", scale_exponent.unsigned_abs())
        }
    }
}

/// The documents in term order, if this corpus is a dictionary.
///
/// A dictionary here means what it sounds like: every term names exactly one
/// record and every record has exactly one term. Address registers are like
/// this and document collections are not, so the test is on the corpus rather
/// than on a flag.
fn dictionary_order(corpus: &corpus::Corpus) -> Option<Vec<u64>> {
    if corpus.postings.len() != corpus.payloads.len() {
        return None;
    }
    let mut order = Vec::with_capacity(corpus.postings.len());
    let mut taken = vec![false; corpus.payloads.len()];
    for docs in corpus.postings.values() {
        let (&doc, &freq) = match docs.iter().next() {
            Some(only) if docs.len() == 1 => only,
            _ => return None,
        };
        if freq != 1 || taken[doc as usize] {
            return None;
        }
        taken[doc as usize] = true;
        order.push(doc);
    }
    Some(order)
}

/// Hashes a file without holding it in memory, since a register is large.
fn hash_file(path: &Path) -> Result<Vec<u8>, Error> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finalize().to_vec());
        }
        hasher.update(&buffer[..read]);
    }
}
