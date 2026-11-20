# Query-side normalization for the tokenizers in TOKENIZERS.md.
#
# A query has to go through whatever the terms went through when the index was
# built, or it will not match them. The index records which tokenizer that was;
# these are the ones a reader can apply to a query without a dictionary.
#
# The same rules as js/src/tokenizers.ts and the Rust ones, written against the
# Unicode properties the registry names rather than against what Python's
# string methods happen to mean: `str.isspace` counts U+001C to U+001F, which
# would treat the `name-list` separator itself as a space, and `str.isalpha`
# misses the combining vowel signs that are letters to `\p{Alphabetic}`.

import bisect
import re
import unicodedata

_JA_NUMERALS = {
    "〇": 0, "零": 0, "一": 1, "二": 2, "三": 3, "四": 4, "五": 5,
    "六": 6, "七": 7, "八": 8, "九": 9, "十": 10, "百": 100, "千": 1000,
}

# `White_Space`, which has not changed since Unicode 6.0. Rust's
# `char::is_whitespace` is exactly this set.
_WHITE_SPACE = (
    "\u0009\u000a\u000b\u000c\u000d \u0085  "
    "           "
    "    　"
)
_WHITE_SPACE_RUN = re.compile(f"[{_WHITE_SPACE}]+")

# `Other_Alphabetic`: the code points that are `\p{Alphabetic}` without being
# letters or numbers by their general category — mostly the vowel signs of
# Indic scripts, which are combining marks. `unicodedata` gives the category
# but not this property, so it is written out, as of Unicode 18.
_OTHER_ALPHABETIC = (
    (0x0345, 0x0345), (0x0363, 0x036F), (0x05B0, 0x05BD), (0x05BF, 0x05BF),
    (0x05C1, 0x05C2), (0x05C4, 0x05C5), (0x05C7, 0x05C9), (0x0610, 0x061A),
    (0x064B, 0x0657), (0x0659, 0x065F), (0x0670, 0x0670), (0x06D6, 0x06DC),
    (0x06E1, 0x06E4), (0x06E7, 0x06E8), (0x06ED, 0x06ED), (0x0711, 0x0711),
    (0x0730, 0x073F), (0x07A6, 0x07B0), (0x0816, 0x0817), (0x081B, 0x0823),
    (0x0825, 0x0827), (0x0829, 0x082C), (0x0897, 0x0897), (0x08D4, 0x08DF),
    (0x08E3, 0x08E9), (0x08F0, 0x0903), (0x093A, 0x093B), (0x093E, 0x094C),
    (0x094E, 0x094F), (0x0955, 0x0957), (0x0962, 0x0963), (0x0981, 0x0983),
    (0x09BE, 0x09C4), (0x09C7, 0x09C8), (0x09CB, 0x09CC), (0x09D7, 0x09D7),
    (0x09E2, 0x09E3), (0x0A01, 0x0A03), (0x0A3E, 0x0A42), (0x0A47, 0x0A48),
    (0x0A4B, 0x0A4C), (0x0A51, 0x0A51), (0x0A70, 0x0A71), (0x0A75, 0x0A75),
    (0x0A81, 0x0A83), (0x0ABE, 0x0AC5), (0x0AC7, 0x0AC9), (0x0ACB, 0x0ACC),
    (0x0AE2, 0x0AE3), (0x0AFA, 0x0AFC), (0x0B01, 0x0B03), (0x0B3E, 0x0B44),
    (0x0B47, 0x0B48), (0x0B4B, 0x0B4C), (0x0B56, 0x0B57), (0x0B62, 0x0B63),
    (0x0B82, 0x0B82), (0x0BBE, 0x0BC2), (0x0BC6, 0x0BC8), (0x0BCA, 0x0BCC),
    (0x0BD7, 0x0BD7), (0x0C00, 0x0C04), (0x0C3E, 0x0C44), (0x0C46, 0x0C48),
    (0x0C4A, 0x0C4C), (0x0C55, 0x0C56), (0x0C62, 0x0C63), (0x0C81, 0x0C83),
    (0x0CBE, 0x0CC4), (0x0CC6, 0x0CC8), (0x0CCA, 0x0CCC), (0x0CD5, 0x0CD6),
    (0x0CE2, 0x0CE3), (0x0CF3, 0x0CF3), (0x0D00, 0x0D03), (0x0D3E, 0x0D44),
    (0x0D46, 0x0D48), (0x0D4A, 0x0D4C), (0x0D57, 0x0D57), (0x0D62, 0x0D63),
    (0x0D81, 0x0D83), (0x0DCF, 0x0DD4), (0x0DD6, 0x0DD6), (0x0DD8, 0x0DDF),
    (0x0DF2, 0x0DF3), (0x0E31, 0x0E31), (0x0E34, 0x0E3A), (0x0E4D, 0x0E4D),
    (0x0EB1, 0x0EB1), (0x0EB4, 0x0EB9), (0x0EBB, 0x0EBC), (0x0ECD, 0x0ECD),
    (0x0F71, 0x0F83), (0x0F8D, 0x0F97), (0x0F99, 0x0FBC), (0x102B, 0x1036),
    (0x1038, 0x1038), (0x103B, 0x103E), (0x1056, 0x1059), (0x105E, 0x1060),
    (0x1062, 0x1064), (0x1067, 0x106D), (0x1071, 0x1074), (0x1082, 0x108D),
    (0x108F, 0x108F), (0x109A, 0x109D), (0x1712, 0x1713), (0x1732, 0x1733),
    (0x1752, 0x1753), (0x1772, 0x1773), (0x17B6, 0x17C8), (0x1885, 0x1886),
    (0x18A9, 0x18A9), (0x1920, 0x192B), (0x1930, 0x1938), (0x1A17, 0x1A1B),
    (0x1A55, 0x1A5E), (0x1A61, 0x1A74), (0x1ABF, 0x1AC0), (0x1ACC, 0x1ACE),
    (0x1B00, 0x1B04), (0x1B35, 0x1B43), (0x1B80, 0x1B82), (0x1BA1, 0x1BA9),
    (0x1BAC, 0x1BAD), (0x1BE7, 0x1BF1), (0x1C24, 0x1C36), (0x1DD3, 0x1DF4),
    (0x24B6, 0x24E9), (0x2DE0, 0x2DFF), (0xA674, 0xA67B), (0xA69E, 0xA69F),
    (0xA802, 0xA802), (0xA80B, 0xA80B), (0xA823, 0xA827), (0xA880, 0xA881),
    (0xA8B4, 0xA8C3), (0xA8C5, 0xA8C5), (0xA8FF, 0xA8FF), (0xA926, 0xA92A),
    (0xA947, 0xA952), (0xA980, 0xA983), (0xA9B4, 0xA9BF), (0xA9E5, 0xA9E5),
    (0xAA29, 0xAA36), (0xAA43, 0xAA43), (0xAA4C, 0xAA4D), (0xAA7B, 0xAA7D),
    (0xAAB0, 0xAAB0), (0xAAB2, 0xAAB4), (0xAAB7, 0xAAB8), (0xAABE, 0xAABE),
    (0xAAEB, 0xAAEF), (0xAAF5, 0xAAF5), (0xABE3, 0xABEA), (0xFB1E, 0xFB1E),
    (0x10376, 0x1037A), (0x10A01, 0x10A03), (0x10A05, 0x10A06),
    (0x10A0C, 0x10A0F), (0x10D24, 0x10D27), (0x10D69, 0x10D69),
    (0x10EAB, 0x10EAC), (0x10ECB, 0x10ECD), (0x10EF3, 0x10EF3),
    (0x10EF5, 0x10EF5), (0x10EF7, 0x10EF8), (0x10EFA, 0x10EFC),
    (0x11000, 0x11002), (0x11038, 0x11045), (0x11073, 0x11074),
    (0x11080, 0x11082), (0x110B0, 0x110B8), (0x110C2, 0x110C2),
    (0x11100, 0x11102), (0x11127, 0x11132), (0x11145, 0x11146),
    (0x11180, 0x11182), (0x111B3, 0x111BF), (0x111CE, 0x111CF),
    (0x1122C, 0x11234), (0x11237, 0x11237), (0x1123E, 0x1123E),
    (0x11241, 0x11241), (0x112DF, 0x112E8), (0x11300, 0x11303),
    (0x1133E, 0x11344), (0x11347, 0x11348), (0x1134B, 0x1134C),
    (0x11357, 0x11357), (0x11362, 0x11363), (0x113B8, 0x113C0),
    (0x113C2, 0x113C2), (0x113C5, 0x113C5), (0x113C7, 0x113CA),
    (0x113CC, 0x113CD), (0x11435, 0x11441), (0x11443, 0x11445),
    (0x114B0, 0x114C1), (0x115AF, 0x115B5), (0x115B8, 0x115BE),
    (0x115DC, 0x115DD), (0x11630, 0x1163E), (0x11640, 0x11640),
    (0x116AB, 0x116B5), (0x1171D, 0x1172A), (0x1182C, 0x11838),
    (0x11930, 0x11935), (0x11937, 0x11938), (0x1193B, 0x1193C),
    (0x11940, 0x11940), (0x11942, 0x11942), (0x119D1, 0x119D7),
    (0x119DA, 0x119DF), (0x119E4, 0x119E4), (0x11A01, 0x11A0A),
    (0x11A35, 0x11A39), (0x11A3B, 0x11A3E), (0x11A51, 0x11A5B),
    (0x11A8A, 0x11A97), (0x11B60, 0x11B67), (0x11C2F, 0x11C36),
    (0x11C38, 0x11C3E), (0x11C92, 0x11CA7), (0x11CA9, 0x11CB6),
    (0x11D31, 0x11D36), (0x11D3A, 0x11D3A), (0x11D3C, 0x11D3D),
    (0x11D3F, 0x11D41), (0x11D43, 0x11D43), (0x11D47, 0x11D47),
    (0x11D8A, 0x11D8E), (0x11D90, 0x11D91), (0x11D93, 0x11D96),
    (0x11DF0, 0x11DF0), (0x11EF3, 0x11EF6), (0x11F00, 0x11F01),
    (0x11F03, 0x11F03), (0x11F34, 0x11F3A), (0x11F3E, 0x11F40),
    (0x1611E, 0x1612E), (0x16F4F, 0x16F4F), (0x16F51, 0x16F87),
    (0x16F8F, 0x16F92), (0x16FF0, 0x16FF1), (0x1BC9E, 0x1BC9E),
    (0x1E000, 0x1E006), (0x1E008, 0x1E018), (0x1E01B, 0x1E021),
    (0x1E023, 0x1E024), (0x1E026, 0x1E02A), (0x1E08F, 0x1E08F),
    (0x1E6E3, 0x1E6E3), (0x1E6E6, 0x1E6E6), (0x1E6EE, 0x1E6EF),
    (0x1E6F5, 0x1E6F5), (0x1E947, 0x1E947), (0x1F130, 0x1F149),
    (0x1F150, 0x1F169), (0x1F170, 0x1F189),
)
_OTHER_ALPHABETIC_STARTS = [start for start, _ in _OTHER_ALPHABETIC]


def _is_alphanumeric(c):
    """Whether `c` is `\\p{Alphabetic}` or `\\p{N}`, what `bigram` keeps."""
    if unicodedata.category(c)[0] in "LN":
        return True
    point = ord(c)
    at = bisect.bisect_right(_OTHER_ALPHABETIC_STARTS, point) - 1
    return at >= 0 and point <= _OTHER_ALPHABETIC[at][1]


def _parse_numeral(run):
    """A run of numerals as a number, or None.

    See TOKENIZERS.md. 十 百 千 multiply the digit before them, or one when there
    is none; two bare digits in sequence are positional notation, which this rule
    does not claim to read.
    """
    total = 0
    digit = None
    for c in run:
        value = _JA_NUMERALS[c]
        if value >= 10:
            total += (1 if digit is None else digit) * value
            digit = None
        else:
            if digit is not None:
                return None
            digit = value
    return total + (0 if digit is None else digit)


# Characters a Japanese place name is written with in more than one way. See
# TOKENIZERS.md: `丸ノ内` and `丸の内` are the same place and the registry
# spells both, so both have to fold onto one before anything is compared.
_JA_VARIANTS = {"ノ": "の", "ケ": "ヶ", "ヵ": "ヶ"}


def _is_katakana(c):
    return c is not None and "゠" <= c <= "ヿ"


def _fold_variants(chars):
    """Folds the variant spellings, but only where they are connectives.

    Inside a katakana word these characters are letters: `マルノウチ` is not
    `マルのウチ`, and `ケーキ` is not `ヶーキ`.
    """
    out = []
    for i, c in enumerate(chars):
        # A kanji is never part of a katakana word, so it needs no guard.
        if c == "之":
            out.append("の")
            continue
        folded = _JA_VARIANTS.get(c)
        before = chars[i - 1] if i > 0 else None
        after = chars[i + 1] if i + 1 < len(chars) else None
        if folded is None or _is_katakana(before) or _is_katakana(after):
            out.append(c)
        else:
            out.append(folded)
    return out


def ja_address(text):
    """The `ja-address` tokenizer, applied to a query."""
    chars = _fold_variants(list(unicodedata.normalize("NFKC", text).strip(_WHITE_SPACE)))
    out = []
    i = 0
    while i < len(chars):
        start = i
        while i < len(chars) and chars[i] in _JA_NUMERALS:
            i += 1
        if i == start:
            out.append(chars[i])
            i += 1
            continue
        run = "".join(chars[start:i])
        chome = chars[i : i + 2] == ["丁", "目"]
        value = _parse_numeral(run) if chome else None
        out.append(run if value is None else str(value))
    return "".join(out)


def name_list(text):
    """One name as `name-list` normalizes it (TOKENIZERS.md): NFKC, the default
    lowercase mapping, and every run of White_Space one space."""
    lowered = unicodedata.normalize("NFKC", text).lower()
    return " ".join(word for word in _WHITE_SPACE_RUN.split(lowered) if word)


def bigram(text):
    """The terms `bigram` makes of a text (TOKENIZERS.md): compatibility
    composition, a run at everything that is not a letter or a digit, each run
    lowercased, and a run that is all ASCII kept whole while any other becomes
    its character bigrams, or its one character."""
    out = []
    runs = []
    current = []
    for c in unicodedata.normalize("NFKC", text):
        if _is_alphanumeric(c):
            current.append(c)
        elif current:
            runs.append("".join(current))
            current = []
    if current:
        runs.append("".join(current))
    for piece in runs:
        run = piece.lower()
        if run.isascii():
            out.append(run)
        elif len(run) == 1:
            out.append(run)
        else:
            out.extend(run[i : i + 2] for i in range(len(run) - 1))
    return out


# The terms a query is looked up as, by the identifier the index records.
#
# A search, unlike a prefix, puts the query through the tokenizer whole: a
# tokenizer that splits turns what was typed into several terms.
SPLITTERS = {
    "space-split": lambda q: [t for t in q.split(" ") if t],
    "identity": lambda q: [q],
    "bigram": bigram,
    "ja-address": lambda q: [ja_address(q)],
    "ja-address-bigram": lambda q: bigram(ja_address(q)),
    "name-list": lambda q: [n for n in (name_list(p) for p in q.split("\u001f")) if n],
}

# What a query goes through before it is looked up as a prefix, by the
# identifier the index records.
TOKENIZERS = {
    "space-split": lambda q: q,
    "identity": lambda q: q,
    "bigram": lambda q: q,
    "ja-address": ja_address,
    # The same normalization as `ja-address`; only the token boundary differs,
    # and a prefix is not split in either.
    "ja-address-bigram": ja_address,
    # What was typed is a prefix of one name, so it is normalized, not split.
    "name-list": name_list,
}
