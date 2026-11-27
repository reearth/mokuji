import type { MokujiIndex } from "./mokuji.js";
/** How a hit of `lookupPrefix` relates to what was asked. */
export type PrefixMatch = "exact" | "prefix" | "within";
export interface PrefixHit {
    term: string;
    doc: number;
    payload: string;
    /** `exact` for the term asked for, `prefix` for one that begins with it,
     * `within` for the longest term a reading begins with: what was asked lies
     * within it. */
    match: PrefixMatch;
}
/**
 * The terms that begin with `normalized`, as many as `limit` documents, or,
 * when none does, the longest term that one of `readings` begins with.
 *
 * The fallback is for keys that a query runs past: an address typed with
 * its block and lot after the town, when the file keys towns, finds the
 * town. Every prefix of every reading, from two characters to one fewer than
 * the reading, is looked up at once; prefixes of one string fall in one or
 * two dictionary blocks, read together. `normalized` and `readings` are
 * already normalized, as the file's tokenizer does (`index.normalize`).
 */
export declare function lookupPrefix(index: MokujiIndex, normalized: string, limit?: number, readings?: string[]): Promise<PrefixHit[]>;
/** The geohash cell of `precision` characters holding (lat, lon). */
export declare function geohash(lat: number, lon: number, precision: number): string;
/** The geohash precisions the file is keyed at (`geohash-precisions`, such
 * as `5,6,7`), or null when it does not say. */
export declare function precisionsOf(index: MokujiIndex): number[] | null;
/** Integers per degree the rings are stored in (`geometry`, such as
 * `delta-int-1e5`), or null when the file does not say. */
export declare function scaleOf(index: MokujiIndex): number | null;
/** The names of the fields a label is cut into at U+001F
 * (`payload-fields`), or null when a label is one name. */
export declare function fieldsOf(index: MokujiIndex): string[] | null;
/** A polygon as `[lon, lat]` rings, closed. */
export type Rings = [number, number][][];
/** Rings as `geometry=delta-int-1e…` writes them: `;` between rings, each the
 * first vertex then the difference from the vertex before, in integers of
 * 1/`scale` degree, without the closing vertex, which is put back. Throws
 * for values that are not integers. */
export declare function decodeRings(text: string, scale: number): Rings;
/** Whether (x, y) is inside, even-odd over every ring, so a hole is one. */
export declare function contains(rings: Rings, x: number, y: number): boolean;
export interface Polygon {
    doc: number;
    /** The payload before its rings. */
    label: string;
    /** The label cut at U+001F and named by `payload-fields`, or null when the
     * file names no fields. */
    fields: Record<string, string> | null;
    rings: Rings;
}
export interface Located {
    /** The point's cell, at the finest precision the file is keyed at. */
    cell: string;
    /** How many polygons the cells named, before the point was tested. */
    candidates: number;
    /** The ones holding the point. */
    inside: Polygon[];
}
/**
 * The polygons holding (lat, lon), from a file of geohash coverings.
 *
 * Every precision is looked up, not only the finest that matches, since a
 * point can be in one polygon's coarse interior cell and another's fine
 * boundary cell at once. The cells are prefixes of one string, so they fall
 * in neighbouring dictionary blocks, read together; the candidates' rings
 * come with their payload, and an even-odd test settles them. Throws for a
 * file that does not say its precisions and geometry, unless they are given.
 */
export declare function locate(index: MokujiIndex, lat: number, lon: number, options?: {
    precisions?: number[];
    scale?: number;
}): Promise<Located>;
