// The manifest of a collection (MANIFEST.md): read and checked as the Rust
// reader's `Manifest::parse` checks it, restricted to a place as
// `Manifest::within` does, and written down as `Manifest::to_json` writes it,
// byte for byte, so that a manifest a page or a worker writes is the one the
// command would have.
import { MokujiError } from "./errors.js";
/** The version of MANIFEST.md this reader implements. */
export const MANIFEST_VERSION = "0.1";
const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const isCount = (v) => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
function manifestError(detail) {
    return new MokujiError(`manifest: ${detail}`);
}
function segmentError(path, detail) {
    return new MokujiError(`segment ${path}: ${detail}`);
}
/**
 * A manifest already decoded from JSON, checked: its shape, a version this
 * reader implements, at least one segment, and for each a path, a content
 * hash of the form `<algorithm>:<lowercase hex>`, deletes strictly
 * increasing, and a region of distinct ISO 3166-1 alpha-2 codes. Keys this
 * version does not define are left out of what is returned, as a later
 * version may add some and the rest still reads.
 */
export function checkManifest(value) {
    if (!isObject(value))
        throw manifestError("not a JSON object");
    const { mokuji_manifest: version, generation, segments } = value;
    if (typeof version !== "string")
        throw manifestError("mokuji_manifest is not a string");
    if (!isCount(generation))
        throw manifestError("generation is not a non-negative integer");
    if (!Array.isArray(segments))
        throw manifestError("segments is not an array");
    if (version.split(".")[0] !== MANIFEST_VERSION.split(".")[0]) {
        throw manifestError(`version ${version} is not one this reader implements`);
    }
    if (segments.length === 0)
        throw manifestError("a collection has no segments");
    const checked = segments.map((raw) => {
        if (!isObject(raw))
            throw manifestError("a segment is not an object");
        const { path, content_hash: hash, length, deletes, region } = raw;
        if (typeof path !== "string")
            throw manifestError("a segment's path is not a string");
        if (typeof hash !== "string")
            throw manifestError("a segment's content_hash is not a string");
        if (!isCount(length))
            throw manifestError("a segment's length is not a non-negative integer");
        if (path === "")
            throw manifestError("a segment has no path");
        const cut = hash.indexOf(":");
        if (cut <= 0 || cut === hash.length - 1 || !/^[0-9a-f]+$/.test(hash.slice(cut + 1))) {
            throw segmentError(path, "content_hash is not <algorithm>:<hex>");
        }
        const segment = { path, content_hash: hash, length };
        if (deletes !== undefined) {
            if (!Array.isArray(deletes) || !deletes.every(isCount)) {
                throw manifestError("a segment's deletes are not non-negative integers");
            }
            if (deletes.some((d, i) => i > 0 && d <= deletes[i - 1])) {
                throw segmentError(path, "deletes are not strictly increasing");
            }
            if (deletes.length)
                segment.deletes = [...deletes];
        }
        if (region !== undefined && region !== null) {
            if (!isObject(region) || !Array.isArray(region.countries) || !region.countries.every((c) => typeof c === "string")) {
                throw manifestError("a segment's region is not a list of countries");
            }
            const countries = region.countries;
            if (countries.length === 0)
                throw segmentError(path, "a region names no country");
            if (!countries.every((c) => /^[A-Z]{2}$/.test(c))) {
                throw segmentError(path, "a region's countries are ISO 3166-1 alpha-2 codes");
            }
            if (new Set(countries).size !== countries.length)
                throw segmentError(path, "a region names a country twice");
            segment.region = { countries: [...countries] };
        }
        return segment;
    });
    return { mokuji_manifest: version, generation, segments: checked };
}
/** Parses and checks a manifest's JSON text (see `checkManifest`). */
export function parseManifest(text) {
    let value;
    try {
        value = JSON.parse(text);
    }
    catch (error) {
        throw manifestError(error instanceof Error ? error.message : String(error));
    }
    return checkManifest(value);
}
/**
 * The collection within these countries: every segment whose region includes
 * one of them, and every segment with no region, in the same order and with
 * the same deletes. Restricting keeps collection order, so a restricted
 * collection answers as the whole one would with the other regions'
 * documents deleted (MANIFEST.md §7).
 */
export function within(manifest, countries) {
    return {
        ...manifest,
        segments: manifest.segments.filter((s) => !s.region || s.region.countries.some((c) => countries.includes(c))),
    };
}
/** The manifest as JSON, in one key order and one layout, ending in a
 * newline: the bytes `mokuji` writes for the same collection. */
export function manifestToJson(manifest) {
    const ordered = {
        mokuji_manifest: manifest.mokuji_manifest,
        generation: manifest.generation,
        segments: manifest.segments.map((s) => ({
            path: s.path,
            content_hash: s.content_hash,
            length: s.length,
            // An empty list and an absent one mean the same, and only one is written.
            ...(s.deletes?.length ? { deletes: s.deletes } : {}),
            ...(s.region ? { region: { countries: s.region.countries } } : {}),
        })),
    };
    return `${JSON.stringify(ordered, null, 2)}\n`;
}
