// Query-side normalization for the tokenizers in TOKENIZERS.md.
//
// A query has to go through whatever the terms went through when the index was
// built, or it will not match them. The index records which tokenizer that was;
// these are the ones a reader can apply to a query without a dictionary.
const JA_NUMERALS = new Map([
    ["〇", 0], ["零", 0], ["一", 1], ["二", 2], ["三", 3], ["四", 4], ["五", 5],
    ["六", 6], ["七", 7], ["八", 8], ["九", 9], ["十", 10], ["百", 100], ["千", 1000],
]);
// See TOKENIZERS.md. 十 百 千 multiply the digit before them, or one when there
// is none; two bare digits in sequence are positional notation, which this rule
// does not claim to read.
function parseNumeral(run) {
    let total = 0;
    let digit = null;
    for (const c of run) {
        const value = JA_NUMERALS.get(c) ?? 0;
        if (value >= 10) {
            total += (digit ?? 1) * value;
            digit = null;
        }
        else {
            if (digit !== null)
                return null;
            digit = value;
        }
    }
    return total + (digit ?? 0);
}
// Characters a Japanese place name is written with in more than one way. See
// TOKENIZERS.md: `丸ノ内` and `丸の内` are the same place and the registry
// spells both, so both have to fold onto one before anything is compared.
const JA_VARIANTS = new Map([
    ["ノ", "の"],
    ["ケ", "ヶ"],
    ["ヵ", "ヶ"],
]);
const isKatakana = (c) => c !== undefined && c >= "゠" && c <= "ヿ";
/** Folds the variant spellings, but only where they are connectives.
 *
 * Inside a katakana word these characters are letters: `マルノウチ` is not
 * `マルのウチ`, and `ケーキ` is not `ヶーキ`.
 */
function foldVariants(chars) {
    return chars.map((c, i) => {
        // A kanji is never part of a katakana word, so it needs no guard.
        if (c === "之")
            return "の";
        const folded = JA_VARIANTS.get(c);
        if (folded === undefined)
            return c;
        if (isKatakana(chars[i - 1]) || isKatakana(chars[i + 1]))
            return c;
        return folded;
    });
}
/** The `ja-address` tokenizer, applied to a query. */
export function jaAddress(text) {
    // White_Space, as TOKENIZERS.md says: String.prototype.trim also strips
    // U+FEFF and leaves U+0085, so it would disagree with the other readers.
    const chars = foldVariants([...text.normalize("NFKC").replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "")]);
    let out = "";
    let i = 0;
    while (i < chars.length) {
        const start = i;
        while (i < chars.length && JA_NUMERALS.has(chars[i]))
            i++;
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
export function nameList(text) {
    return text
        .normalize("NFKC")
        .toLowerCase()
        .split(/\p{White_Space}+/u)
        .filter(Boolean)
        .join(" ");
}
/** The terms `bigram` makes of a text (TOKENIZERS.md): compatibility
 * composition, a run at everything that is not a letter or a digit, each run
 * lowercased, and a run that is all ASCII kept whole while any other becomes
 * its character bigrams, or its one character. */
export function bigram(text) {
    const out = [];
    for (const piece of text.normalize("NFKC").split(/[^\p{Alphabetic}\p{N}]/u)) {
        if (!piece)
            continue;
        const run = piece.toLowerCase();
        // eslint-disable-next-line no-control-regex
        if (/^[\x00-\x7f]*$/.test(run)) {
            out.push(run);
            continue;
        }
        const chars = [...run];
        if (chars.length === 1) {
            out.push(chars[0]);
            continue;
        }
        for (let i = 0; i + 1 < chars.length; i++)
            out.push(chars[i] + chars[i + 1]);
    }
    return out;
}
/** The terms a query is looked up as, by the identifier the index records.
 *
 * A search, unlike a prefix, puts the query through the tokenizer whole: a
 * tokenizer that splits turns what was typed into several terms. */
export const SPLITTERS = {
    "space-split": (q) => q.split(" ").filter(Boolean),
    identity: (q) => [q],
    bigram,
    "ja-address": (q) => [jaAddress(q)],
    "ja-address-bigram": (q) => bigram(jaAddress(q)),
    "name-list": (q) => q
        .split("\u001f")
        .map(nameList)
        .filter(Boolean),
};
export const TOKENIZERS = {
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
