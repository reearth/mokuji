//! `mokuji manifest` - keep a collection's manifest, and query a collection.
//!
//! A collection is a base and the segments added since, with the documents
//! deleted from each, listed in a manifest (`MANIFEST.md`). The files never
//! change; the manifest is the one thing rewritten, and every change to it
//! bumps its generation.

use std::path::{Path, PathBuf};

use mokuji_manifest::{Collection, Manifest, Segment, VERSION};
use mokuji_meta::{ContentHash, Meta};
use mokuji_read::{block_on::block_on, CountingReader, RangeReader, SliceReader};

use crate::corpus::Tokenizer;
use crate::info::Error;

/// Whether `path` names a manifest rather than a Mokuji file.
///
/// Decided by content, not by name: a Mokuji file begins with its magic, and a
/// manifest is JSON, which does not.
pub fn is_manifest(path: &Path) -> bool {
    use std::io::Read;
    let mut first = [0u8; 1];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut first))
        .map(|_| first[0] == b'{')
        .unwrap_or(false)
}

/// Reads a manifest.
pub fn read(path: &Path) -> Result<Manifest, Error> {
    Ok(Manifest::parse(&std::fs::read_to_string(path)?)?)
}

/// Where a segment the manifest names is, on disk.
pub fn locate(manifest: &Path, segment: &Segment) -> PathBuf {
    manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&segment.path)
}

/// Opens every segment, each through its own counting reader.
pub fn open(path: &Path, manifest: &Manifest) -> Result<Vec<CountingReader<SliceReader>>, Error> {
    manifest
        .segments
        .iter()
        .map(|segment| {
            Ok(CountingReader::new(SliceReader::new(std::fs::read(
                locate(path, segment),
            )?)))
        })
        .collect()
}

fn describe(manifest: &Path, file: &Path) -> Result<Segment, Error> {
    let length = std::fs::metadata(file)?.len();
    Ok(Segment {
        path: relative(manifest, file),
        content_hash: ContentHash::sha2_256(&crate::build::hash_file(file)?).to_string(),
        length,
        deletes: Vec::new(),
    })
}

/// `file` as the manifest should name it: relative to the manifest's directory
/// when it is beneath it, so that the two can be published together.
fn relative(manifest: &Path, file: &Path) -> String {
    let base = manifest
        .parent()
        .map(|p| {
            if p.as_os_str().is_empty() {
                Path::new(".")
            } else {
                p
            }
        })
        .and_then(|p| p.canonicalize().ok());
    let full = file.canonicalize().ok();
    match (base, full) {
        (Some(base), Some(full)) => match full.strip_prefix(&base) {
            Ok(rest) => rest.display().to_string(),
            Err(_) => full.display().to_string(),
        },
        _ => file.display().to_string(),
    }
}

fn write(path: &Path, manifest: &Manifest) -> Result<(), Error> {
    // Checked as a reader would check it before it is written, so that a
    // manifest this tool writes is one it would accept.
    Manifest::parse(&manifest.to_json())?;
    std::fs::write(path, manifest.to_json())?;
    Ok(())
}

/// `mokuji manifest new`: a collection of these files, in this order.
pub fn new(path: &Path, files: &[&Path]) -> Result<(), Error> {
    if files.is_empty() {
        return Err(Error::Usage(
            "a collection needs at least one segment".into(),
        ));
    }
    let manifest = Manifest {
        mokuji_manifest: VERSION.into(),
        generation: 1,
        segments: files
            .iter()
            .map(|f| describe(path, f))
            .collect::<Result<_, _>>()?,
    };
    check(path, &manifest)?;
    write(path, &manifest)?;
    println!(
        "generation {}: {} segment(s)",
        manifest.generation,
        manifest.segments.len()
    );
    Ok(())
}

/// `mokuji manifest add`: one more segment, after the rest.
pub fn add(path: &Path, file: &Path) -> Result<(), Error> {
    let mut manifest = read(path)?;
    manifest.segments.push(describe(path, file)?);
    manifest.generation += 1;
    check(path, &manifest)?;
    write(path, &manifest)?;
    println!(
        "generation {}: {} segment(s)",
        manifest.generation,
        manifest.segments.len()
    );
    Ok(())
}

/// `mokuji manifest delete`: these documents of this segment are gone.
///
/// Reports how many completion entries now list a deleted document. Each is a
/// prefix that the section no longer answers alone, and costs what it would
/// without one until the collection is compacted — which is what to watch.
pub fn delete(path: &Path, segment_path: &str, docs: &[u64]) -> Result<(), Error> {
    let mut manifest = read(path)?;
    let at = manifest
        .segments
        .iter()
        .position(|s| s.path == segment_path)
        .ok_or_else(|| Error::Usage(format!("no segment '{segment_path}' in the manifest")))?;
    let index = block_on(mokuji_read::Index::open(SliceReader::new(std::fs::read(
        locate(path, &manifest.segments[at]),
    )?)))?;
    if let Some(&bad) = docs.iter().find(|&&d| d >= index.header().num_docs) {
        return Err(Error::Usage(format!(
            "document {bad} is not in '{segment_path}', which has {}",
            index.header().num_docs
        )));
    }

    let deletes = &mut manifest.segments[at].deletes;
    deletes.extend_from_slice(docs);
    deletes.sort_unstable();
    deletes.dedup();
    manifest.generation += 1;
    write(path, &manifest)?;

    let deleted = &manifest.segments[at].deletes;
    let touched = index.completions().map_or(0, |c| {
        c.entries()
            .filter(|(_, listed)| listed.iter().any(|d| deleted.binary_search(d).is_ok()))
            .count()
    });
    println!(
        "generation {}: {} document(s) deleted from {segment_path}",
        manifest.generation,
        deleted.len()
    );
    if touched > 0 {
        println!(
            "{touched} completion entr{} now list a deleted document, and cost a walk \
             of the dictionary until compacted",
            if touched == 1 { "y" } else { "ies" }
        );
    }
    Ok(())
}

/// Checks a manifest against its files: that each is there, is the length and
/// hash the manifest says, and opens alongside the others.
pub fn check(path: &Path, manifest: &Manifest) -> Result<(), Error> {
    for segment in &manifest.segments {
        let file = locate(path, segment);
        let hash = ContentHash::sha2_256(&crate::build::hash_file(&file)?).to_string();
        if hash != segment.content_hash {
            return Err(Error::Invalid(format!(
                "{} hashes to {hash}, not the {} the manifest records",
                segment.path, segment.content_hash
            )));
        }
    }
    let readers = open(path, manifest)?;
    block_on(Collection::open(manifest, readers))?;
    Ok(())
}

/// `mokuji verify` on a manifest.
pub fn verify(path: &Path) -> Result<(), Error> {
    let manifest = read(path)?;
    check(path, &manifest)?;
    println!("generation      {}", manifest.generation);
    for segment in &manifest.segments {
        println!(
            "segment         {}  {} bytes, {} deleted",
            segment.path,
            segment.length,
            segment.deletes.len()
        );
    }
    println!();
    println!("ok");
    Ok(())
}

fn tokenizer_of<R: RangeReader>(collection: &Collection<R>) -> Result<Option<Tokenizer>, Error> {
    let meta = Meta::parse(collection.segment(0).meta())?;
    Ok(Tokenizer::parse(&meta.tokenizer.id))
}

fn report(readers: &[CountingReader<SliceReader>]) {
    let per: Vec<u32> = readers.iter().map(|r| r.reads()).collect();
    let bytes: u64 = readers.iter().map(|r| r.stats().bytes).sum();
    eprintln!(
        "query:    {} read(s) across {} segment(s), at most {} in any one; {bytes} bytes",
        per.iter().sum::<u32>(),
        per.len(),
        per.iter().max().copied().unwrap_or(0),
    );
}

/// `mokuji search` on a manifest: every term, in every segment, less deletes.
pub fn search(path: &Path, terms: &[&str], limit: usize) -> Result<(), Error> {
    let manifest = read(path)?;
    let readers = open(path, &manifest)?;
    let collection = block_on(Collection::open(&manifest, readers.iter().collect()))?;
    readers.iter().for_each(CountingReader::reset);

    let tokenizer = tokenizer_of(&collection)?;
    let normalized: Vec<String> = terms
        .iter()
        .map(|t| tokenizer.map_or_else(|| t.to_string(), |k| k.normalize_query(t)))
        .collect();
    let query: Vec<&[u8]> = normalized.iter().map(|t| t.as_bytes()).collect();
    let hits = block_on(collection.search(&query))?;
    let shown: Vec<_> = hits.iter().copied().take(limit).collect();
    let payloads = block_on(collection.payloads(&shown))?;
    for (hit, payload) in shown.iter().zip(&payloads) {
        println!("{}:{}\t{}", hit.segment, hit.doc, payload);
    }
    eprintln!();
    eprintln!("{} match(es), {} shown", hits.len(), shown.len());
    report(&readers);
    Ok(())
}

/// `mokuji complete` on a manifest.
pub fn complete(path: &Path, typed: &str, limit: usize) -> Result<(), Error> {
    let manifest = read(path)?;
    let readers = open(path, &manifest)?;
    let collection = block_on(Collection::open(&manifest, readers.iter().collect()))?;
    readers.iter().for_each(CountingReader::reset);

    let normalized =
        tokenizer_of(&collection)?.map_or_else(|| typed.to_string(), |k| k.normalize_query(typed));
    let found = block_on(collection.complete(normalized.as_bytes(), limit))?;
    let payloads = block_on(collection.payloads(&found.hits))?;
    for (hit, payload) in found.hits.iter().zip(&payloads) {
        println!("{}:{}\t{}", hit.segment, hit.doc, payload);
    }
    eprintln!();
    for (segment, from_table) in &found.consulted {
        eprintln!(
            "segment {segment}: {}",
            if *from_table {
                "the completions section"
            } else {
                "the dictionary"
            }
        );
    }
    report(&readers);
    Ok(())
}
