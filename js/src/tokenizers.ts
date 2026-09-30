// Query-side normalization for the tokenizers in TOKENIZERS.md.
//
// A query has to go through whatever the terms went through when the index was
// built, or it will not match them. The index records which tokenizer that was;
// these are the ones a reader can apply to a query without a dictionary.

const JA_NUMERALS = new Map<string, number>([
  ["〇", 0], ["零", 0], ["一", 1], ["二", 2], ["三", 3], ["四", 4], ["五", 5],
  ["六", 6], ["七", 7], ["八", 8], ["九", 9], ["十", 10], ["百", 100], ["千", 1000],
]);

// See TOKENIZERS.md. 十 百 千 multiply the digit before them, or one when there
// is none; two bare digits in sequence are positional notation, which this rule
// does not claim to read.
function parseNumeral(run: string): number | null {
  let total = 0;
  let digit: number | null = null;
  for (const c of run) {
    const value = JA_NUMERALS.get(c) ?? 0;
    if (value >= 10) {
      total += (digit ?? 1) * value;
      digit = null;
    } else {
      if (digit !== null) return null;
      digit = value;
    }
  }
  return total + (digit ?? 0);
}

// Characters a Japanese place name is written with in more than one way. See
// TOKENIZERS.md: `丸ノ内` and `丸の内` are the same place and the registry
// spells both, so both have to fold onto one before anything is compared.
const JA_VARIANTS = new Map<string, string>([
  ["ノ", "の"],
  ["ケ", "ヶ"],
  ["ヵ", "ヶ"],
]);

const isKatakana = (c: string | undefined): boolean =>
  c !== undefined && c >= "゠" && c <= "ヿ";

/** Folds the variant spellings, but only where they are connectives.
 *
 * Inside a katakana word these characters are letters: `マルノウチ` is not
 * `マルのウチ`, and `ケーキ` is not `ヶーキ`.
 */
function foldVariants(chars: string[]): string[] {
  return chars.map((c, i) => {
    // A kanji is never part of a katakana word, so it needs no guard.
    if (c === "之") return "の";
    const folded = JA_VARIANTS.get(c);
    if (folded === undefined) return c;
    if (isKatakana(chars[i - 1]) || isKatakana(chars[i + 1])) return c;
    return folded;
  });
}

/** The `ja-address` tokenizer, applied to a query. */
export function jaAddress(text: string): string {
  const chars = foldVariants([...text.normalize("NFKC").trim()]);
  let out = "";
  let i = 0;
  while (i < chars.length) {
    const start = i;
    while (i < chars.length && JA_NUMERALS.has(chars[i]!)) i++;
    if (i === start) {
      out += chars[i++];
      continue;
    }
    const run = chars.slice(start, i).join("");
    const value = chars[i] === "丁" && chars[i + 1] === "目" ? parseNumeral(run) : null;
    out += value === null ? run : String(value);
  }
  return out;
}

/** One name as `name-list` normalizes it (TOKENIZERS.md): NFKC, the default
 * lowercase mapping, and every run of White_Space one space. `\s` is not
 * White_Space — it takes U+FEFF and misses U+0085 — so the property is named. */
export function nameList(text: string): string {
  return text
    .normalize("NFKC")
    .toLowerCase()
    .split(/\p{White_Space}+/u)
    .filter(Boolean)
    .join(" ");
}

/** What a query goes through, by the identifier the index records. */
export type Normalize = (query: string) => string;

export const TOKENIZERS: Readonly<Record<string, Normalize>> = {
  "space-split": (q) => q,
  identity: (q) => q,
  bigram: (q) => q,
  "ja-address": jaAddress,
  // The same normalization as `ja-address`; only the token boundary differs,
  // and a prefix is not split in either.
  "ja-address-bigram": jaAddress,
  // What was typed is a prefix of one name, so it is normalized, not split.
  "name-list": nameList,
};
