import { type Normalize } from "./tokenizers.js";
/**
 * Where a file's bytes come from: anything that returns a range of them.
 *
 * A URL string makes the reader fetch ranges itself. A `Blob` (a `File` from a
 * page, `openAsBlob(path)` in Node) is read a slice at a time, so a file on
 * disk is searched without being loaded and without a network. An object lets
 * the caller decide — an R2 binding and an edge cache in a worker — while the
 * reader counts what was asked for either way.
 */
export interface Source {
    read(offset: number, length: number): Promise<Uint8Array> | Uint8Array;
}
/** What a query cost, so a page can show it rather than claim it. */
export interface Stats {
    requests: number;
    bytes: number;
}
/** One range request, as reported to `onRequest`. */
export interface RequestInfo {
    offset: number;
    length: number;
}
/** A value the metadata section can hold (SPEC.md §6). */
export type CborValue = number | string | boolean | null | Uint8Array | CborValue[] | {
    [key: string]: CborValue;
};
/** One dictionary entry (SPEC.md §5.3.4). */
export interface TermEntry {
    term: Uint8Array;
    termId: number;
    docFreq: number;
    postingsOffset: number;
    /** Bytes of the postings section the term's run occupies: zero when the
     * entry states its postings itself, implied or inline. */
    postingsLength: number;
    /** The postings, when the entry carries them inline. */
    inline?: Postings;
    positionsOffset?: number;
    positionsLength?: number;
}
/** One term's postings: ascending document IDs and their frequencies. */
export interface Postings {
    docs: number[];
    freqs: number[];
}
/** One artifact an index was derived from, as its metadata records it (SPEC.md §6). */
export interface Provenance {
    /** `<algorithm>:<lowercase hex>`. */
    contentHash: string;
    /** Where it can be obtained. */
    uri: string | null;
    /** When the builder obtained it, RFC 3339. */
    retrieved: string | null;
    /** When its publisher last changed it, RFC 3339 — how current the data is. */
    modified: string | null;
}
/** One named column of per-document numbers (SPEC.md §5.8). */
export interface Column {
    name: string;
    scaleExponent: number;
    values: number[];
}
/** How `search` combines and ranks the terms of a query. */
export interface SearchOptions {
    /** How many results to return with their payload. */
    limit?: number;
    /** The union of the terms rather than their intersection: what an n-gram
     * query wants, since a misspelling shares every gram but two. */
    any?: boolean;
    /** Fetch postings for only this many of the terms, the rarest, which the
     * dictionary says without reading any postings. */
    probe?: number;
    /** BM25's `b`, from 0 to 1. Zero for a corpus whose long documents are its
     * good ones. */
    lengthNorm?: number;
    /** An attribute column added to each score as `ln(1 + value)`, a static
     * prior. */
    boost?: string;
    /** Keep only the documents this accepts, by document ID, before they are
     * ranked and before any payload is read, so that discarding a document costs
     * no payload. `matched` then counts what it kept. */
    filter?: (doc: number) => boolean;
}
/** What `search` found, and what deciding it took. */
export interface SearchResult {
    /** The terms the query became. */
    terms: string[];
    /** The terms whose postings were read, with how many documents each names. */
    probed: {
        term: string;
        docFreq: number;
    }[];
    /** Terms present but not read, because `probe` left them out. */
    skipped: number;
    /** Every document matched, best first. */
    matched: number;
    results: {
        doc: number;
        score: number | null;
        payload: string;
    }[];
}
/** The completions section (SPEC.md §5.3.5), keyed by prefix bytes. */
export interface Completions {
    minTerms: number;
    maxSpan: number;
    docsPerEntry: number;
    entries: Map<string, number[]>;
}
interface Section {
    codec: number;
    offset: number;
    length: number;
}
/** The metadata section's CBOR, decoded; exported for the tests. */
export declare const decodeCbor: (bytes: Uint8Array) => CborValue;
/** One open Mokuji file: its resident set held, everything else read on demand. */
export declare class MokujiIndex {
    /** Everything a query costs, so the page can show it rather than claim it. */
    stats: Stats;
    /** Whether neighbouring blocks are read in one request (SPEC.md §4 allows
     * it: the count can only fall). On by default; the conformance suite turns
     * it off to check the count the cases state, block by block. */
    coalesce: boolean;
    /** Called with every range request, so a page can show the requests
     * themselves rather than a count of them. */
    onRequest: ((request: RequestInfo) => void) | null;
    /** The file's length, once a response has said. */
    fileLength: number | undefined;
    flags: number;
    numDocs: number;
    sumDoclen: number;
    sections: Map<number, Section>;
    /** The metadata section, decoded (SPEC.md §6). */
    meta: CborValue;
    tokenizerId: string;
    /** The index's license, as an SPDX expression, if it states one. */
    license: string | null;
    /** The credit its results carry, if it states one. */
    attribution: string | null;
    /** What it was derived from, and when (SPEC.md §6). */
    sources: Provenance[];
    /** What a query goes through before it is looked up. */
    normalize: Normalize;
    docLengths: number[] | null;
    columns: Map<string, Column>;
    completions: Completions | null;
    termIndex: Uint8Array;
    termBlockOffsets: Uint8Array;
    payloadOffsets: Uint8Array | undefined;
    blockCount: number;
    boundaries: Uint8Array[];
    /** A URL, or what the bytes are read from; a `Blob` is read as a `Source`. */
    readonly source: string | Source;
    /**
     * @param source a URL, a `Blob`, or a `Source` returning bytes — which is
     * what lets the same reader be tested against a file on disk.
     */
    constructor(source: string | Source | Blob);
    resetStats(): void;
    /**
     * `length` bytes at `offset`. A range that runs past the end of the file is
     * refused (SPEC.md §8) rather than answered with what there is, unless
     * `upTo` says fewer is fine — as it is for the opening read, which asks for
     * more than a small file holds.
     */
    read(offset: number, length: number, upTo?: boolean): Promise<Uint8Array>;
    private fetchRange;
    unit(bytes: Uint8Array, codec: number): Promise<Uint8Array>;
    private section;
    static open(source: string | Source | Blob): Promise<MokujiIndex>;
    /** Where block `i` of section `id` is, from that section's offsets, which
     * are checked to lie within it before they are used (SPEC.md §8). */
    private offsetPair;
    /** Blocks of one section by ordinal, a request for each run of neighbours.
     *
     * Blocks are laid end to end, so ordinals that follow one another are one
     * contiguous range of the file, and reading them as one costs a single round
     * trip where reading them apart costs one each. The runs are independent of
     * each other and fetched at once. Returns each block as stored, before its
     * codec is undone.
     */
    private blockRuns;
    /** Several dictionary blocks, decoded, a request for each run of neighbours. */
    termBlocks(ordinals: Iterable<number>): Promise<Map<number, TermEntry[]>>;
    termBlock(ordinal: number): Promise<TermEntry[]>;
    private decodeTermBlock;
    /** One named column of per-document numbers, or undefined.
     *
     * Resident, so this costs no read: the point of the section is that a scorer
     * can weigh many documents before deciding which payloads are worth fetching.
     */
    column(name: string): Column | undefined;
    /** Where a term occurs in each document its postings name.
     *
     * One read, and only for a caller that asks. The run is decoded from the
     * front and the frequencies say how many values belong to each document, so
     * the postings have to be in hand first.
     */
    positions(entry: TermEntry, postings: Postings): Promise<number[][]>;
    /** The dictionary block a term would be in, from the resident index alone. */
    blockFor(term: Uint8Array): number;
    /** One term, exactly, or null.
     *
     * A prefix scan would answer this, and would cost more: `xn76ur` is a prefix
     * of every cell inside it, so completing it fetches postings for all of them
     * when the caller wanted one. Exactly one block is read, always.
     */
    exact(termText: string): Promise<TermEntry | null>;
    /** Several terms at once, each exactly, in one read per dictionary block.
     *
     * Cells one geohash precision apart are neighbours in the dictionary, so the
     * lookups a point needs usually land in the same block. Asking for them
     * together is the difference between reading it once and reading it twice.
     */
    exactMany(termTexts: string[]): Promise<(TermEntry | null)[]>;
    /** Terms beginning with `prefix`, at most `limit` of them. */
    prefix(prefixText: string, limit?: number): Promise<TermEntry[]>;
    /** Terms beginning with each of several prefixes, every one of them.
     *
     * Which blocks each prefix spans is answerable from the resident index, so
     * all of them are known before any is read, and they are fetched together:
     * a request for each run of neighbouring blocks, however many prefixes fall
     * in it. This is what a query over an area needs — the cells covering it
     * are many short prefixes, mostly in blocks next to one another — and what
     * `prefix`, which walks one prefix a block at a time, would make a request
     * per block per prefix.
     */
    prefixMany(prefixTexts: string[]): Promise<TermEntry[][]>;
    /** Terms from `low` to `high`, both inclusive, at most `limit` of them.
     *
     * The same walk as `prefix`, with a different test for where it stops: the
     * dictionary is sorted, so a range of keys is a run of neighbouring entries,
     * and whether the run continues past a block is answerable from the resident
     * index alone.
     */
    range(lowText: string, highText: string, limit?: number): Promise<TermEntry[]>;
    implied(entry: TermEntry): Postings;
    /** Postings for a contiguous range of entries, in one request — or none. */
    postingsSpan(entries: TermEntry[]): Promise<Postings[]>;
    /** Payload for documents, as text: opaque bytes decoded, or integers written
     * out against the scale the file declares. */
    payloadSpan(docs: number[]): Promise<string[]>;
    /** Documents ranked by BM25 over the terms the query becomes.
     *
     * The same ranking as `mokuji search`: Robertson–Spärck Jones idf, `k1` of
     * 1.2, `b` from `lengthNorm`, a union scored over the terms a document
     * actually has, and ties broken by document ID. Without document lengths the
     * order is by document ID and the scores are null.
     *
     * One request per run of dictionary blocks the terms fall in, then one per
     * term whose postings are stored as a run, then the payload of the first
     * `limit`.
     */
    search(query: string, options?: SearchOptions): Promise<SearchResult>;
    /** The first `limit` documents for what has been typed.
     *
     * From the completions section when the prefix has an entry that reaches
     * the limit — or lists fewer than it may, so lists them all — which costs no
     * request before the payload. Otherwise every term the prefix matches, whose
     * runs are united and returned in document order.
     */
    complete(typed: string, limit?: number): Promise<Completion>;
    /** `complete` for a prefix already normalized, without the payload. */
    completeDocs(normalized: string, limit: number): Promise<{
        docs: number[];
        fromTable: boolean;
    }>;
    /** Complete a prefix, returning every matching document with its payload.
     *
     * The query goes through the index's own tokenizer first. Without that, an
     * index built from `一丁目` is unreachable by anyone typing `一丁目`.
     */
    lookup(prefixText: string, limit?: number): Promise<{
        normalized: string;
        results: {
            term: string;
            doc: number;
            payload: string;
        }[];
    }>;
}
/** What `MokujiIndex.complete` found, and how. */
export interface Completion {
    normalized: string;
    /** Whether the completions section answered, at no read before the payload. */
    fromTable: boolean;
    docs: number[];
    payloads: string[];
    requestsBeforePayload: number;
}
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
/** One document of a collection: its segment, and its ID there. */
export interface Hit {
    segment: number;
    doc: number;
}
/** What a collection's completion found, and how. */
export interface CollectionCompletion {
    normalized: string;
    hits: Hit[];
    consulted: {
        segment: number;
        fromTable: boolean;
    }[];
    payloads: string[];
    requestsBeforePayload: number;
}
/**
 * A collection: segments in a manifest's order, less their deletes
 * (MANIFEST.md). Every segment is an ordinary Mokuji file, opened on its own.
 */
export declare class MokujiCollection {
    readonly manifest: Manifest;
    readonly segments: MokujiIndex[];
    readonly deletes: Set<number>[];
    /** How many segments the manifest lists, before a region left some out. */
    manifestSegments: number;
    constructor(manifest: Manifest, segments: MokujiIndex[]);
    /**
     * Opens the manifest at `url` and every segment it names, at once: they are
     * independent, so opening them costs one round trip however many there are.
     * `sourceFor(url)` may supply a byte source instead of fetching.
     */
    static open(url: string | Manifest, sourceFor?: (url: string) => string | Source | Blob, { countries }?: {
        countries?: string[] | null;
    }): Promise<MokujiCollection>;
    resetStats(): void;
    /** Requests and bytes, summed; and the most requests any one segment made. */
    get stats(): Stats & {
        roundTrips: number;
    };
    normalize(typed: string): string;
    /** One segment's first `need` live documents: the entry, then all it
     * lists, then as many again as the segment has deletes (MANIFEST.md §3.2). */
    liveIn(at: number, normalized: string, need: number): Promise<{
        live: number[];
        fromTable: boolean;
    }>;
    /** The first `limit` live documents, in collection order. */
    complete(typed: string, limit?: number): Promise<CollectionCompletion>;
    /** Every segment's own first `limit`, asked at once, for a caller that
     * ranks across segments by a score of its own (MANIFEST.md §3.3). */
    candidates(typed: string, limit?: number): Promise<CollectionCompletion>;
    /** Payload for hits, one span per segment, in the hits' order. */
    payloads(hits: Hit[]): Promise<string[]>;
    /** Each segment's credit, once each, for a page that shows results. */
    get attributions(): string[];
}
export {};
