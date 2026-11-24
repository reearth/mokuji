/** The `ja-address` tokenizer, applied to a query. */
export declare function jaAddress(text: string): string;
/** One name as `name-list` normalizes it (TOKENIZERS.md): NFKC, the default
 * lowercase mapping, and every run of White_Space one space. `\s` is not
 * White_Space — it takes U+FEFF and misses U+0085 — so the property is named. */
export declare function nameList(text: string): string;
/** The terms `bigram` makes of a text (TOKENIZERS.md): compatibility
 * composition, a run at everything that is not a letter or a digit, each run
 * split again where ASCII meets CJK, each piece lowercased, and a piece that is
 * all ASCII kept whole while any other becomes its character bigrams, or its
 * one character. */
export declare function bigram(text: string): string[];
/** What a query goes through, by the identifier the index records. */
export type Normalize = (query: string) => string;
/** The terms a query is looked up as, by the identifier the index records.
 *
 * A search, unlike a prefix, puts the query through the tokenizer whole: a
 * tokenizer that splits turns what was typed into several terms. */
export declare const SPLITTERS: Readonly<Record<string, (query: string) => string[]>>;
export declare const TOKENIZERS: Readonly<Record<string, Normalize>>;
