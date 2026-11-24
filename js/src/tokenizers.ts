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
  // White_Space, as TOKENIZERS.md says: String.prototype.trim also strips
  // U+FEFF and leaves U+0085, so it would disagree with the other readers.
  const chars = foldVariants([...text.normalize("NFKC").replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "")]);
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

/** The terms `bigram` makes of a text (TOKENIZERS.md): compatibility
 * composition, a run at everything that is not a letter or a digit, each run
 * split again where ASCII meets CJK, each piece lowercased, and a piece that is
 * all ASCII kept whole while any other becomes its character bigrams, or its
 * one character. */
export function bigram(text: string): string[] {
  return bigrams(text, true);
}

/** The blocks `bigram` splits from ASCII: CJK symbols, kana, Hangul, and the
 * unified and compatibility ideographs. Blocks rather than `\p{Script}`,
 * because `ー` is Common by script and `々` sits among symbols. */
const CJK: readonly (readonly [number, number])[] = [
  [0x1100, 0x11ff], [0x3000, 0x30ff], [0x3130, 0x318f], [0x31f0, 0x31ff],
  [0x3400, 0x4dbf], [0x4e00, 0x9fff], [0xac00, 0xd7af], [0xf900, 0xfaff],
  [0x20000, 0x3ffff],
];

function isCjk(c: string): boolean {
  const point = c.codePointAt(0)!;
  return CJK.some(([start, end]) => start <= point && point <= end);
}

function isAscii(c: string): boolean {
  return c.codePointAt(0)! < 0x80;
}

/** `bigram`, with the split where ASCII meets CJK only when `splitCjk`.
 * `ja-address-bigram` does not split there: `1丁` is a bigram an address query
 * leans on. */
function bigrams(text: string, splitCjk: boolean): string[] {
  const out: string[] = [];
  for (const run of text.normalize("NFKC").split(/[^\p{Alphabetic}\p{N}]/u)) {
    if (!run) continue;
    const chars = [...run];
    let start = 0;
    for (let i = 1; i <= chars.length; i++) {
      const boundary =
        i === chars.length ||
        (splitCjk &&
          ((isAscii(chars[i - 1]!) && isCjk(chars[i]!)) || (isCjk(chars[i - 1]!) && isAscii(chars[i]!))));
      if (!boundary) continue;
      pushBigrams(chars.slice(start, i).join(""), out);
      start = i;
    }
  }
  return out;
}

function pushBigrams(piece: string, out: string[]): void {
  const run = piece.toLowerCase();
  // eslint-disable-next-line no-control-regex
  if (/^[\x00-\x7f]*$/.test(run)) {
    out.push(run);
    return;
  }
  const chars = [...run];
  if (chars.length === 1) {
    out.push(chars[0]!);
    return;
  }
  for (let i = 0; i + 1 < chars.length; i++) out.push(chars[i]! + chars[i + 1]!);
}

/** What a query goes through, by the identifier the index records. */
export type Normalize = (query: string) => string;

/** The terms a query is looked up as, by the identifier the index records.
 *
 * A search, unlike a prefix, puts the query through the tokenizer whole: a
 * tokenizer that splits turns what was typed into several terms. */
export const SPLITTERS: Readonly<Record<string, (query: string) => string[]>> = {
  "space-split": (q) => q.split(" ").filter(Boolean),
  identity: (q) => [q],
  bigram,
  "ja-address": (q) => [jaAddress(q)],
  "ja-address-bigram": (q) => bigrams(jaAddress(q), false),
  "name-list": (q) =>
    q
      .split("\u001f")
      .map(nameList)
      .filter(Boolean),
};

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
