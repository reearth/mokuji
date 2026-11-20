# The tokenizers against TOKENIZERS.md, with the vectors the Rust and
# TypeScript tests use. A tokenizer is outside the format, but an index is
# unreachable if two implementations disagree about it.

import unittest

import support  # noqa: F401  (puts the package on the path)

from mokuji import SPLITTERS, bigram, ja_address, name_list


class JaAddress(unittest.TestCase):
    def test_agrees_with_the_other_implementations(self):
        vectors = [
            ("東京都千代田区丸の内一丁目", "東京都千代田区丸の内1丁目"),
            ("東京都千代田区丸の内１丁目", "東京都千代田区丸の内1丁目"),
            ("札幌市北区北十一丁目", "札幌市北区北11丁目"),
            ("二十丁目", "20丁目"),
            ("百丁目", "100丁目"),
            ("千代田区一番町", "千代田区一番町"),
            ("京都市下京区五条", "京都市下京区五条"),
            ("一二丁目", "一二丁目"),
            ("  東京都　", "東京都"),
            ("ﾏﾙﾉｳﾁ", "マルノウチ"),
        ]
        for given, expected in vectors:
            self.assertEqual(ja_address(given), expected, given)

    def test_folds_the_spellings_of_a_place_name_onto_one(self):
        # The registry writes both, so a reader that folded only one of them
        # would find the address it was not given.
        self.assertEqual(ja_address("東京都千代田区丸ノ内一丁目"), ja_address("東京都千代田区丸の内1丁目"))
        self.assertEqual(ja_address("神奈川県茅ケ崎市"), ja_address("神奈川県茅ヶ崎市"))
        self.assertEqual(ja_address("三重県いなべ市北勢町二之瀬"), ja_address("三重県いなべ市北勢町二の瀬"))

        # Left alone on purpose: `が` is an ordinary particle, and `ハイツ` is
        # a word.
        self.assertNotEqual(ja_address("三重県亀山市みずきが丘"), ja_address("三重県亀山市みずきヶ丘"))
        self.assertEqual(ja_address("三重県亀山市関町富士ハイツ"), "三重県亀山市関町富士ハイツ")

        # Inside a katakana word these are letters, not connectives.
        self.assertEqual(ja_address("マルノウチ"), "マルノウチ")
        self.assertEqual(ja_address("ケーキ"), "ケーキ")

    def test_strips_white_space_and_nothing_else(self):
        # `White_Space`, as Rust's `trim` reads it: U+0085 is whitespace, and
        # U+001F, which Python's `strip` would also take, is not.
        self.assertEqual(ja_address("\u0085東京都　"), "東京都")
        self.assertEqual(ja_address("\u001f東京都"), "\u001f東京都")


class Bigram(unittest.TestCase):
    def test_splits_as_tokenizers_md_says(self):
        self.assertEqual(bigram("建築物"), ["建築", "築物"])
        self.assertEqual(bigram("CityGML"), ["citygml"])
        self.assertEqual(bigram("3D都市"), ["3d", "d都", "都市"])
        self.assertEqual(bigram("都"), ["都"])
        self.assertEqual(bigram(ja_address("丸の内一丁目")), ["丸の", "の内", "内1", "1丁", "丁目"])

    def test_a_vowel_sign_is_a_letter(self):
        # Devanagari vowel signs are combining marks by category and
        # `Alphabetic` by property, so `हिंदी` is one run, not three.
        self.assertEqual(bigram("हिंदी"), ["हि", "िं", "ंद", "दी"])


class NameList(unittest.TestCase):
    def test_agrees_with_the_other_implementations(self):
        self.assertEqual(name_list("  Black  Sea "), "black sea")
        self.assertEqual(name_list("ＴＯＫＹＯ"), "tokyo")
        self.assertEqual(name_list("İstanbul"), "i̇stanbul")
        self.assertEqual(name_list("ΟΔΟΣ"), "οδος")

    def test_the_separator_is_not_a_space(self):
        # Python's `str.split()` would split on U+001F itself.
        self.assertEqual(SPLITTERS["name-list"]("Tokyo\u001f東京\u001f  "), ["tokyo", "東京"])


if __name__ == "__main__":
    unittest.main()
