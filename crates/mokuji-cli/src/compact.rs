//! `mokuji compact` - fold a collection into one file.
//!
//! The file answers exactly as the collection did. Documents keep the order the
//! collection gives them — segments in manifest order, each in its own order,
//! deletes left out — so that a ranking by document order, which is how the
//! completions this workspace writes rank, survives the fold unchanged.
//!
//! It works on the indexes, not on a corpus: terms, postings and payload are
//! read back and written again under new document IDs. Nothing is tokenized
//! twice, so nothing can be tokenized differently, and a collection can be
//! compacted without the sources it was built from.

use std::collections::BTreeMap;
use std::path::Path;

use mokuji_format::{SectionCodec, SectionId};
use mokuji_manifest::Collection;
use mokuji_meta::{cbor_map, ContentHash, Generator, Meta, Source, Value};
use mokuji_read::{block_on::block_on, term, Index, Payload, RangeReader, SliceReader};
use mokuji_write::{Builder, Column, Completions, Options, PayloadEncoding};

use crate::collection;
use crate::info::Error;

/// Compacts the collection `manifest` names into `output`.
pub fn run(manifest_path: &Path, output: &Path) -> Result<(), Error> {
    let manifest = collection::read(manifest_path)?;
    collection::check(manifest_path, &manifest)?;
    let readers: Vec<SliceReader> = manifest
        .segments
        .iter()
        .map(|s| {
            Ok(SliceReader::new(std::fs::read(collection::locate(
                manifest_path,
                s,
            ))?))
        })
        .collect::<Result<_, Error>>()?;
    let collection = block_on(Collection::open(&manifest, readers))?;

    let base = collection.segment(0);
    let base_meta = Meta::parse(base.meta())?;
    let options = options_of(base, &base_meta)?;
    for i in 1..collection.len() {
        agree(
            &manifest.segments[i].path,
            base,
            &base_meta,
            collection.segment(i),
        )?;
    }

    // New IDs in collection order.
    let mut renumbered: Vec<Vec<Option<u64>>> = Vec::with_capacity(collection.len());
    let mut next = 0u64;
    for (i, segment) in manifest.segments.iter().enumerate() {
        let docs = collection.segment(i).header().num_docs;
        let mut map = vec![None; docs as usize];
        for (doc, slot) in map.iter_mut().enumerate() {
            if segment.deletes.binary_search(&(doc as u64)).is_err() {
                *slot = Some(next);
                next += 1;
            }
        }
        renumbered.push(map);
    }
    if next == 0 {
        return Err(Error::Invalid("every document has been deleted".into()));
    }

    let mut builder = Builder::new(options)?;
    let mut columns: Vec<Column> = base
        .columns()
        .iter()
        .map(|c| Column {
            name: c.name.clone(),
            scale_exponent: c.scale_exponent,
            values: Vec::new(),
        })
        .collect();
    for (i, map) in renumbered.iter().enumerate() {
        let index = collection.segment(i);
        let live: Vec<u64> = (0..map.len() as u64)
            .filter(|&d| map[d as usize].is_some())
            .collect();
        for chunk in live.chunks(1 << 16) {
            for payload in block_on(index.payload_span(chunk))? {
                match payload {
                    Payload::Opaque(bytes) => builder.add_document(bytes)?,
                    Payload::Ints { values, .. } => builder.add_document_values(&values)?,
                };
            }
        }
        for (column, from) in columns.iter_mut().zip(index.columns()) {
            column
                .values
                .extend(live.iter().map(|&d| from.values[d as usize]));
        }
    }
    for column in columns {
        builder.add_column(column)?;
    }

    // Every term of every segment, merged in term order. Segments are visited
    // in collection order and IDs were assigned in it, so each term's
    // documents arrive already ascending.
    let mut postings: BTreeMap<Vec<u8>, Vec<(u64, u64)>> = BTreeMap::new();
    for (i, map) in renumbered.iter().enumerate() {
        block_on(gather(collection.segment(i), map, &mut postings))?;
    }
    for (term, docs) in &postings {
        for &(doc, freq) in docs {
            builder.add_posting(term, doc, freq)?;
        }
    }

    // The inputs are the manifest and the segments it names, so those are
    // what the provenance records. The build parameters are the base's.
    let mut sources = vec![Source {
        content_hash: ContentHash::sha2_256(&crate::build::hash_file(manifest_path)?),
        uri: Some(manifest_path.display().to_string()),
        ..Source::default()
    }];
    for segment in &manifest.segments {
        sources.push(Source {
            content_hash: ContentHash::parse(&segment.content_hash)?,
            uri: Some(segment.path.clone()),
            ..Source::default()
        });
    }
    let mut params: Vec<(String, Value)> = match &base_meta.generator.params {
        Value::Map(entries) => entries
            .iter()
            .filter_map(|(k, v)| k.as_text().map(|k| (k.to_string(), v.clone())))
            .collect(),
        _ => Vec::new(),
    };
    params.push((
        "compacted".into(),
        Value::Text(format!(
            "{} segment(s), manifest generation {}",
            manifest.segments.len(),
            manifest.generation
        )),
    ));
    let meta = Meta {
        generation: manifest.generation + 1,
        sources,
        generator: Generator {
            name: "mokuji".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            params: cbor_map(params),
        },
        tokenizer: base_meta.tokenizer.clone(),
        attrs: base_meta.attrs.clone(),
        license: base_meta.license.clone(),
        attribution: base_meta.attribution.clone(),
    };
    let bytes = builder.finish(&meta.encode())?;
    std::fs::write(output, &bytes)?;

    let deleted: usize = manifest.segments.iter().map(|s| s.deletes.len()).sum();
    println!("{} bytes", bytes.len());
    println!(
        "{next} documents from {} segment(s), {deleted} deleted, {} terms",
        manifest.segments.len(),
        postings.len()
    );
    Ok(())
}

async fn gather<R: RangeReader>(
    index: &Index<R>,
    map: &[Option<u64>],
    out: &mut BTreeMap<Vec<u8>, Vec<(u64, u64)>>,
) -> Result<(), Error> {
    for ordinal in 0..index.term_index()?.block_count() {
        let block = index.read_term_block(ordinal).await?;
        let entries = term::decode_block(&block, index.block_shape())?;
        let located: Vec<_> = entries.iter().map(|(_, e)| e.clone()).collect();
        for ((term, _), run) in entries.iter().zip(index.postings_span(&located).await?) {
            let live: Vec<(u64, u64)> = run
                .docs
                .iter()
                .zip(&run.freqs)
                .filter_map(|(&d, &f)| map[d as usize].map(|n| (n, f)))
                .collect();
            if !live.is_empty() {
                out.entry(term.clone()).or_default().extend(live);
            }
        }
    }
    Ok(())
}

/// The build options the base was written with, read from the file itself
/// where the file says, and from its recorded parameters where only they do.
fn options_of<R: RangeReader>(base: &Index<R>, meta: &Meta) -> Result<Options, Error> {
    if base.has_positions() {
        return Err(Error::Invalid(
            "compacting a collection with positions is not implemented".into(),
        ));
    }
    let param = |key: &str| meta.generator.params.get(key).and_then(Value::as_uint);
    let payload = match block_on(base.payload_span(&[0]))?.into_iter().next() {
        Some(Payload::Ints {
            values,
            scale_exponent,
        }) => PayloadEncoding::Ints {
            values_per_doc: values.len() as u8,
            scale_exponent,
        },
        _ => PayloadEncoding::Opaque,
    };
    let codec = |id: SectionId| base.sections().iter().find(|s| s.id == id).map(|s| s.codec);
    Ok(Options {
        terms_per_block: param("terms_per_block").unwrap_or(1024) as usize,
        docs_per_payload_block: param("docs_per_payload_block").unwrap_or(64) as u32,
        has_freqs: base
            .header()
            .flags
            .contains(mokuji_format::Flags::HAS_FREQS),
        payload,
        positions: false,
        doc_lengths: base.doc_lengths().is_some(),
        compress: codec(SectionId::TERM_BLOCKS) != Some(SectionCodec::Raw),
        completions: base.completions().map(|c| Completions {
            min_terms: c.min_terms,
            max_span: c.max_span,
            docs_per_entry: c.docs_per_entry as u32,
        }),
        // A base written before inline postings records no bound, and wrote
        // every term a run.
        inline_postings: param("inline_postings").unwrap_or(0) as usize,
    })
}

/// A later segment has to be written down the same way as the base, or one
/// file cannot hold both.
fn agree<R: RangeReader>(
    path: &str,
    base: &Index<R>,
    base_meta: &Meta,
    other: &Index<R>,
) -> Result<(), Error> {
    let fail = |what: &str| {
        Err(Error::Invalid(format!(
            "{path} differs from the base in {what}"
        )))
    };
    let meta = Meta::parse(other.meta())?;
    if base.header().flags != other.header().flags {
        return fail("its flags");
    }
    if base.doc_lengths().is_some() != other.doc_lengths().is_some() {
        return fail("whether it carries document lengths");
    }
    let names = |i: &Index<R>| {
        i.columns()
            .iter()
            .map(|c| (c.name.clone(), c.scale_exponent))
            .collect::<Vec<_>>()
    };
    if names(base) != names(other) {
        return fail("its attribute columns");
    }
    let kind = |p: Option<Payload>| match p {
        Some(Payload::Ints {
            values,
            scale_exponent,
        }) => Some((values.len(), scale_exponent)),
        _ => None,
    };
    if kind(block_on(base.payload_span(&[0]))?.pop())
        != kind(block_on(other.payload_span(&[0]))?.pop())
    {
        return fail("how its payload is written down");
    }
    // Sources that license differently belong in different files (SPEC.md §6),
    // so a fold that would mix them is refused rather than guessed at.
    if base_meta.license != meta.license || base_meta.attribution != meta.attribution {
        return fail("its license or attribution");
    }
    Ok(())
}
