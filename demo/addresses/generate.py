#!/usr/bin/env python3
"""Generate a synthetic Japanese address corpus.

Real data (the national address base registry) is the target; this stands in
for it while the format is being proven, and is deliberately shaped like it:
a strict hierarchy, prefecture to city to town to block to number, with heavy
prefix sharing between neighbours. That shape is what a COLex address index
exploits, so a corpus without it would flatter the format.

What it does NOT reproduce is the real kanji distribution: the pools here are
small, so suffixes are shared more than they would be in practice. Sizes
measured on this corpus are therefore optimistic, and are reported as such.

Output is one address per line: the address, a tab, then `lat,lon`.

Usage: python3 generate.py [--count N] [--out PATH]
"""

import argparse
import pathlib

PREFECTURES = [
    ("北海道", 43.06, 141.35), ("青森県", 40.82, 140.74), ("岩手県", 39.70, 141.15),
    ("宮城県", 38.27, 140.87), ("秋田県", 39.72, 140.10), ("山形県", 38.24, 140.36),
    ("福島県", 37.75, 140.47), ("茨城県", 36.34, 140.45), ("栃木県", 36.57, 139.88),
    ("群馬県", 36.39, 139.06), ("埼玉県", 35.86, 139.65), ("千葉県", 35.61, 140.12),
    ("東京都", 35.69, 139.69), ("神奈川県", 35.45, 139.64), ("新潟県", 37.90, 139.02),
    ("富山県", 36.70, 137.21), ("石川県", 36.59, 136.63), ("福井県", 36.07, 136.22),
    ("山梨県", 35.66, 138.57), ("長野県", 36.65, 138.18), ("岐阜県", 35.39, 136.72),
    ("静岡県", 34.98, 138.38), ("愛知県", 35.18, 136.91), ("三重県", 34.73, 136.51),
    ("滋賀県", 35.00, 135.87), ("京都府", 35.02, 135.76), ("大阪府", 34.69, 135.52),
    ("兵庫県", 34.69, 135.18), ("奈良県", 34.69, 135.83), ("和歌山県", 34.23, 135.17),
    ("鳥取県", 35.50, 134.24), ("島根県", 35.47, 133.05), ("岡山県", 34.66, 133.93),
    ("広島県", 34.40, 132.46), ("山口県", 34.19, 131.47), ("徳島県", 34.07, 134.56),
    ("香川県", 34.34, 134.04), ("愛媛県", 33.84, 132.77), ("高知県", 33.56, 133.53),
    ("福岡県", 33.61, 130.42), ("佐賀県", 33.25, 130.30), ("長崎県", 32.74, 129.87),
    ("熊本県", 32.79, 130.74), ("大分県", 33.24, 131.61), ("宮崎県", 31.91, 131.42),
    ("鹿児島県", 31.56, 130.56), ("沖縄県", 26.21, 127.68),
]

CITY_STEMS = list("北南東西緑桜川山田上下大小新本広長高松原野林") + [""]
CITY_HEADS = (
    "千代田 中央 港 新宿 文京 台東 墨田 江東 品川 目黒 大田 世田谷 渋谷 "
    "中野 杉並 豊島 北 荒川 板橋 練馬 足立 葛飾 江戸川"
).split()
TOWN_HEADS = (
    "丸の内 大手町 有楽町 神田 日本橋 京橋 銀座 築地 八重洲 内幸町 霞が関 "
    "永田町 赤坂 六本木 麻布 白金 青山 表参道 代官山 恵比寿"
).split()
TOWN_TAILS = ["", "本町", "東", "西", "南", "北", "中央", "新町", "旧町", "台"]
KANSUJI = list("一二三四五六七八九十")


def cities(prefecture_index):
    """City names for one prefecture, distinct across the country."""
    out = []
    for i, head in enumerate(CITY_HEADS):
        stem = CITY_STEMS[(prefecture_index + i) % len(CITY_STEMS)]
        suffix = "区" if prefecture_index == 12 else ("市" if i % 3 else "町")
        out.append(f"{stem}{head}{suffix}")
    return out


def towns(city_index):
    out = []
    for i, head in enumerate(TOWN_HEADS):
        tail = TOWN_TAILS[(city_index + i) % len(TOWN_TAILS)]
        out.append(f"{head}{tail}")
    return out


def generate(count):
    """Addresses in sorted-by-construction hierarchical order."""
    rows = []
    for p, (prefecture, lat0, lon0) in enumerate(PREFECTURES):
        for c, city in enumerate(cities(p)):
            for t, town in enumerate(towns(c)):
                for chome in range(1, 4):
                    for banchi in range(1, 9):
                        # Coordinates fan out from the prefecture centre so
                        # that neighbouring addresses are neighbouring points,
                        # which is what makes payload delta encoding worth
                        # anything.
                        lat = lat0 + (t * 3 + chome) * 0.0012 + banchi * 0.00015
                        lon = lon0 + (c * 5 + chome) * 0.0011 + banchi * 0.00013
                        address = f"{prefecture}{city}{town}{KANSUJI[chome - 1]}丁目{banchi}番地"
                        rows.append(f"{address}\t{lat:.5f},{lon:.5f}")
                        if len(rows) >= count:
                            return rows
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--count", type=int, default=300_000)
    parser.add_argument("--out", default="addresses.tsv")
    args = parser.parse_args()

    rows = generate(args.count)
    # The dictionary is sorted anyway; sorting here means the corpus and the
    # index agree on order, which makes document IDs follow the hierarchy and
    # therefore makes a prefix range contiguous in the payload section too.
    rows.sort()
    path = pathlib.Path(args.out)
    path.write_text("\n".join(rows) + "\n")
    print(f"wrote {len(rows)} addresses to {path}")


if __name__ == "__main__":
    main()
