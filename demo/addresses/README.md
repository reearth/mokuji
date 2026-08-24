# Address lookup

Forward geocoding from a single static file, over range reads, with no server.

```sh
python3 generate.py --count 300000 --out addresses.tsv
colex build addresses.tsv addresses.colex --tokenizer identity
colex prefix addresses.colex "東京都上足立区丸の内一丁目"
```

```
東京都上足立区丸の内一丁目1番地	35.69135,139.80123
東京都上足立区丸の内一丁目2番地	35.69150,139.80136
...
8 match(es)
open:     2 read(s)
query:    3 read(s) total - 1 dictionary, 1 postings, 1 payload
bytes:    600 of 3995624
```

Three range reads and 600 bytes, from a 4 MB file. Nothing was running.

## Why prefix and not search

An address is a hierarchical key. It is looked up whole or by prefix, never by
intersecting its parts — nobody searches for documents containing both `東京都`
and `丸の内`. So the whole normalized address is one term (`--tokenizer
identity`), and the query is a prefix range over the sorted dictionary.

This is the case the format is shaped for. Because terms are sorted, everything
sharing a prefix is contiguous; because postings are written in term order,
their runs are contiguous too; and because documents were added in the same
order, so is their payload. A prefix range is one contiguous slice of every
section at once.

## Measured

300,000 addresses, `--tokenizer identity`, default block sizes (128 terms,
64 documents).

| Results | Dictionary | Postings | Payload | Total reads | Bytes |
|---|---|---|---|---|---|
| 8 | 1 | 1 | 1 | 3 | 600 B |
| 20 | 1 | 1 | 1 | 3 | 684 B |
| 100 | 2 | 1 | 2 | 5 | 1.7 KB |
| 500 | 5 | 1 | 8 | 14 | 7.0 KB |
| 2,000 | 17 | 1 | 32 | 50 | 27 KB |

Opening the file costs 2 reads and is paid once.

The dictionary column is `ceil(results / 128) + 1` and the payload column is
`ceil(results / 64)`, which is what `SPEC.md` §4.1 states. **Postings is always
1**, at every scale, because a contiguous term range has a contiguous run
range.

Nothing here grows with the size of the file. The same query costs the same
three reads against 300,000 addresses or 19 million.

## Sections

Blocks are deflated individually, so a reader still fetches one block and
decompresses one block. Read counts are unchanged by compression; only the
bytes moved are.

```
                      stored      raw     codec
meta                    261 B    261 B    raw     resident
term_index              5.5 KB    77 KB   deflate resident
term_block_offsets       19 KB    19 KB   raw     resident
payload_offsets          37 KB    37 KB   raw     resident
postings                2.0 MB   2.0 MB   raw
term_blocks              574 KB  4.1 MB   deflate
payload                 1.3 MB   5.7 MB   deflate
                        ------  ------
                        3.8 MB  12.0 MB   from a 20 MB corpus
```

Building takes 0.7 s; `colex verify`, which reads and decodes every block,
takes 2.3 s.

The offset tables and the metadata are stored raw on purpose. Metadata stays
readable without a decompressor because it is the provenance record, and
tooling that only wants to know where a file came from should not have to
implement the format to find out. The offset tables stay raw because their size
feeds back into the placement they describe; compressing them is worthwhile and
wants a settled layout to build on.

## What these numbers are not

**Compression uses no shared dictionary.** Measured on these very blocks, a
trained shared dictionary takes term blocks from 7.06x to 27.2x. That is by far
the largest remaining win, and it is deliberately not taken: browsers cannot
supply a preset dictionary through their native decompression, so it would cost
a decompressor in the page. See `SPEC.md` §5.6.1.

**The corpus is synthetic and flatters the format.** `generate.py` draws town
and city names from small pools, so neighbouring addresses share far longer
prefixes than real ones do. Front-coding therefore does better here than it
will on the national address base registry. Treat 13.8 bytes per address as a
lower bound, not an estimate.

**Postings are almost pure redundancy in this index, and now they dominate it.**
With one term per document, the postings section says nothing but "term *i*
names document *i*". It cannot be compressed — its addressed unit is a single
term's seven-byte run — so it is now **more than half the file**: 2.0 MB of
3.8 MB, having been a sixth of it before the other sections shrank. A codec
that recognized the identity mapping would delete it outright. That is not in
the specification, and this is the measurement that argues it should be.
