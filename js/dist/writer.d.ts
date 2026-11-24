import type { Column } from "./mokuji.js";
/** What this writer calls itself in `generator.name`. */
export declare const GENERATOR_NAME = "mokuji-js";
/**
 * What this package records as `generator.version`. A release sets it with
 * the other packages' versions (RELEASING.md), and publishing checks that they
 * agree.
 */
export declare const VERSION = "0.2.0";
/** The first section identifier an application may use (SPEC.md §5.2). */
export declare const SECTION_APPLICATION_MIN = 32768;
/** An integer as a writer takes it: a safe integer, or a bigint for the rest of 64 bits. */
export type Int = number | bigint;
/**
 * Which prefixes get an entry in the completions section (SPEC.md §5.3.5), and
 * how many documents each lists. The documents listed are the smallest IDs
 * among those holding a term that begins with the prefix, so a caller who
 * wants the most important first numbers documents by importance.
 */
export interface CompletionOptions {
    /** A prefix that at least this many terms begin with gets an entry; zero promises nothing. */
    minTerms?: number;
    /** A prefix whose terms' runs total at least this many bytes gets an entry; zero promises nothing. */
    maxSpan?: number;
    /** How many documents an entry lists at most. */
    docsPerEntry: number;
}
/**
 * A build's parameters, defaulting to the Rust writer's. They belong in the
 * metadata's `generator.params` (SPEC.md §3.4); `writeParams` says them.
 */
export interface WriteOptions {
    /** Terms per dictionary block. 1024. */
    termsPerBlock?: number;
    /** Documents per payload block. 64. */
    docsPerPayloadBlock?: number;
    /** Whether to store term frequencies. True. */
    hasFreqs?: boolean;
    /**
     * A fixed row of integers per document, delta-coded (SPEC.md §5.5.3), in
     * place of opaque bytes; a stored v stands for v * 10^scaleExponent.
     */
    ints?: {
        valuesPerDoc: number;
        scaleExponent: number;
    } | null;
    /** Whether to store where each term occurs in each document. False. */
    positions?: boolean;
    /** Whether to store every document's length in tokens, for BM25. False. */
    docLengths?: boolean;
    /**
     * Whether to compress the sections that can be: each unit on its own, and a
     * section whose units do not shrink stored raw. True.
     */
    compress?: boolean;
    /** Whether to answer expensive prefixes ahead of time. */
    completions?: CompletionOptions | null;
    /** The most bytes a term's postings may take inline in its entry; zero writes every term a run. 8. */
    inlinePostings?: number;
}
type Resolved = Required<Omit<WriteOptions, "ints" | "completions">> & {
    ints: {
        valuesPerDoc: number;
        scaleExponent: number;
    } | null;
    completions: Required<CompletionOptions> | null;
};
/**
 * The options as `mokuji build` records them in `generator.params`, and
 * whether the build compressed. A caller with parameters of its own, such as
 * how the documents were ordered, adds them.
 */
export declare function writeParams(options?: WriteOptions): Record<string, CborInput>;
/**
 * A value metadata can hold: a non-negative integer, text, an array, or an
 * object with text keys. Booleans are written as 0 and 1, as `mokuji build`
 * records them.
 */
export type CborInput = number | bigint | boolean | string | CborInput[] | {
    [key: string]: CborInput;
};
/**
 * One value as canonical CBOR (RFC 8949 §4.2): definite lengths, shortest-form
 * integers, and map keys sorted by their encoded bytes.
 */
export declare function encodeCbor(value: CborInput): Uint8Array;
/** One source an index was derived from, as a writer records it. */
export interface SourceRecord {
    /** `<algorithm>:<lowercase hex>`, required. */
    contentHash: string;
    uri?: string | null;
    retrieved?: string | null;
    modified?: string | null;
}
/**
 * The metadata section (SPEC.md §6), for a writer: what an index was derived
 * from and how, which together are required to be sufficient to rebuild it
 * byte for byte (§3.4).
 */
export interface Metadata {
    /** A file is immutable; a new index is a new generation. 1. */
    generation?: number;
    sources: SourceRecord[];
    /** `GENERATOR_NAME`, without it. */
    generatorName?: string;
    /** `VERSION`, without it. */
    generatorVersion?: string;
    /** The full build parameters, such as `writeParams` gives. */
    generatorParams?: Record<string, CborInput>;
    /** An identifier from TOKENIZERS.md. */
    tokenizerId: string;
    tokenizerParams?: Record<string, CborInput>;
    /** Application-defined additions. */
    attrs?: Record<string, CborInput> | null;
    /** The index's own license, as an SPDX expression. */
    license?: string | null;
    /** What a consumer shows alongside its results. */
    attribution?: string | null;
}
/** The metadata section, as canonical CBOR. */
export declare function encodeMetadata(meta: Metadata): Uint8Array;
/**
 * The terms a document's text becomes under a tokenizer from TOKENIZERS.md,
 * as a writer builds them. A key tokenizer, `identity` or `ja-address`, makes
 * the whole text one term.
 */
export declare function split(tokenizerId: string, text: string): string[];
/**
 * Builds a Mokuji file from documents and a sorted stream of postings.
 *
 * Documents are added first, in document ID order; postings are then pushed
 * in term order (by UTF-8 bytes, which is not JavaScript's string order) and
 * within a term in document order. Both orderings are checked rather than
 * assumed. Sorting stays with the caller.
 */
export declare class MokujiBuilder {
    readonly options: Resolved;
    private payloads;
    private rows;
    private columns;
    private sumDocLen;
    private lengths;
    private terms;
    private open;
    private sections;
    constructor(options?: WriteOptions);
    /** How many documents have been added. */
    get documents(): number;
    /** Appends a document whose payload is opaque bytes, or text written as UTF-8, returning its ID. */
    addDocument(payload: string | Uint8Array): number;
    /** Appends a document whose payload is a fixed row of integers, returning its ID. */
    addDocumentValues(values: readonly Int[]): number;
    /** Adds a column of per-document numbers (SPEC.md §5.8), checked at `finish` against the document count. */
    addColumn(column: Column): void;
    /**
     * Records that `term` occurs `freq` times in `doc`. Terms ascend by their
     * UTF-8 bytes; within a term, documents ascend.
     */
    addPosting(term: string | Uint8Array, doc: number, freq?: number): void;
    /** Records that `term` occurs in `doc` at exactly these ascending positions. */
    addPostingAt(term: string | Uint8Array, doc: number, positions: readonly number[]): void;
    /**
     * Attaches an application-defined section (SPEC.md §5.2), stored
     * uncompressed and placed last, so that it moves nothing a reader looks for.
     */
    addSection(id: number, bytes: Uint8Array): void;
    private closeTerm;
    /**
     * Moves short postings into their dictionary entries (SPEC.md §5.3.4), once
     * every term is known: a term's first document is written against the
     * entries before it in its block.
     */
    private inlinePostings;
    /** Compresses each unit, or leaves them all raw if that is no smaller; decided per section. */
    private pack;
    /**
     * The file, with `meta` as the metadata section: canonical CBOR, such as
     * `encodeMetadata` makes. It carries the provenance of a source the writer
     * never sees, so composing it is the caller's job.
     */
    finish(meta: Uint8Array): Promise<Uint8Array>;
    private termBlocks;
}
export {};
