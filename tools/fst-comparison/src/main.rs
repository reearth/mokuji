//! Does automaton minimization beat front-coding plus deflate on real
//! addresses? The claim under test is that a finite state transducer shares the
//! repeated 番/号 tails between neighbourhoods, which a blocked front-coded
//! dictionary cannot.

use std::io::{BufRead, BufReader};

fn main() {
    let path = std::env::args().nth(1).expect("usage: fstbench <dump.tsv>");
    let file = BufReader::new(std::fs::File::open(&path).expect("open"));

    let mut terms: Vec<String> = Vec::new();
    for line in file.lines() {
        let line = line.expect("read");
        terms.push(line.split('\t').next().expect("term").to_string());
    }
    let bytes: usize = terms.iter().map(String::len).sum();
    println!("{} terms, {} bytes of raw text", terms.len(), bytes);
    println!("  raw text            {:.2} B/term", bytes as f64 / terms.len() as f64);

    let set = fst::Set::from_iter(terms.iter()).expect("terms are sorted");
    let set_len = set.as_fst().as_bytes().len();
    println!(
        "  fst::Set            {:>10} bytes  {:.2} B/term",
        set_len,
        set_len as f64 / terms.len() as f64
    );

    let map = fst::Map::from_iter(terms.iter().enumerate().map(|(i, t)| (t, i as u64)))
        .expect("terms are sorted");
    let map_len = map.as_fst().as_bytes().len();
    println!(
        "  fst::Map, rank      {:>10} bytes  {:.2} B/term",
        map_len,
        map_len as f64 / terms.len() as f64
    );
}
