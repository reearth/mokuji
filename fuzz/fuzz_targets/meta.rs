//! Metadata is canonical CBOR from an untrusted file.
//!
//! Given its own target because it is the one section written in a general
//! encoding, and therefore the one with the most shapes to get wrong.

#![no_main]

use colex_meta::Meta;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(meta) = Meta::parse(data) {
        // A parse that succeeded must re-encode; if it does not, the parser
        // accepted something the encoder cannot express.
        let _ = meta.encode();
    }
});
