/** The `ja-address` tokenizer, applied to a query. */
export declare function jaAddress(text: string): string;
/** One name as `name-list` normalizes it (TOKENIZERS.md): NFKC, the default
 * lowercase mapping, and every run of White_Space one space. `\s` is not
 * White_Space — it takes U+FEFF and misses U+0085 — so the property is named. */
export declare function nameList(text: string): string;
/** What a query goes through, by the identifier the index records. */
export type Normalize = (query: string) => string;
export declare const TOKENIZERS: Readonly<Record<string, Normalize>>;
