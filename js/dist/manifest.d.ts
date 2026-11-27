/** The version of MANIFEST.md this reader implements. */
export declare const MANIFEST_VERSION = "0.1";
/** One segment of a collection, as the manifest lists it (MANIFEST.md §2). */
export interface ManifestSegment {
    path: string;
    content_hash: string;
    length: number;
    deletes?: number[];
    region?: {
        countries: string[];
    };
}
/** A collection's manifest (MANIFEST.md §2). */
export interface Manifest {
    mokuji_manifest: string;
    generation: number;
    segments: ManifestSegment[];
}
type Json = unknown;
/**
 * A manifest already decoded from JSON, checked: its shape, a version this
 * reader implements, at least one segment, and for each a path, a content
 * hash of the form `<algorithm>:<lowercase hex>`, deletes strictly
 * increasing, and a region of distinct ISO 3166-1 alpha-2 codes. Keys this
 * version does not define are left out of what is returned, as a later
 * version may add some and the rest still reads.
 */
export declare function checkManifest(value: Json): Manifest;
/** Parses and checks a manifest's JSON text (see `checkManifest`). */
export declare function parseManifest(text: string): Manifest;
/**
 * The collection within these countries: every segment whose region includes
 * one of them, and every segment with no region, in the same order and with
 * the same deletes. Restricting keeps collection order, so a restricted
 * collection answers as the whole one would with the other regions'
 * documents deleted (MANIFEST.md §7).
 */
export declare function within(manifest: Manifest, countries: string[]): Manifest;
/** The manifest as JSON, in one key order and one layout, ending in a
 * newline: the bytes `mokuji` writes for the same collection. */
export declare function manifestToJson(manifest: Manifest): string;
export {};
