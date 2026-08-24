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
query:    2 read(s) total - 1 dictionary, 0 postings, 1 payload
bytes:    541 of 1957831
```

Two range reads and 541 bytes, from a 2 MB file. Nothing was running.

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
| 8 | 1 | 0 | 1 | 2 | 541 B |
| 20 | 1 | 0 | 1 | 2 | 541 B |
| 100 | 2 | 0 | 2 | 4 | 1.0 KB |
| 500 | 5 | 0 | 7 | 12 | 3.5 KB |
| 2,000 | 17 | 0 | 32 | 49 | 13 KB |

Opening the file costs 2 reads and is paid once.

The dictionary column is `ceil(results / 128) + 1` and the payload column is
`ceil(results / 64)`, which is what `SPEC.md` §4.1 states.

**Postings is always 0.** Every term here names exactly the one record that is
about it, and its document ID is its own term ID, so the dictionary entry states
the postings and none are stored (`SPEC.md` §5.3.4). The section is present with
a length of zero.

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
postings                    0    2.0 MB   raw     implied
term_blocks              570 KB  4.1 MB   deflate
payload                 1.3 MB   5.7 MB   deflate
                        ------  ------
                        1.9 MB  12.0 MB   from a 20 MB corpus
```

Building takes 0.7 s; `colex verify`, which reads and decodes every block,
takes about 2 s.

The offset tables and the metadata are stored raw on purpose. Metadata stays
readable without a decompressor because it is the provenance record, and
tooling that only wants to know where a file came from should not have to
implement the format to find out. The offset tables stay raw because their size
feeds back into the placement they describe; compressing them is worthwhile and
wants a settled layout to build on.

## In a browser

`../web` serves the same file to a page that reads it with no WebAssembly and
no bundled decompressor, because the required codec is one browsers already
have.

```sh
cd ../web && python3 serve.py --index ../addresses/addresses.colex
```

Completing a prefix costs three `Range` requests and about 700 bytes out of the
3.8 MB file, which is byte-for-byte what the command line tool spends — the two
readers fetch the same ranges.

## What these numbers are not

**Compression uses no shared dictionary.** Measured on these very blocks, a
trained shared dictionary takes term blocks from 7.06x to 27.2x. That is by far
the largest remaining win, and it is deliberately not taken: browsers cannot
supply a preset dictionary through their native decompression, so it would cost
a decompressor in the page. See `SPEC.md` §5.6.1.

**Sorted means byte order, not numeric order.** `一丁目` sorts before `三丁目`
before `二丁目`, because those are the code points' order. A real address
normalizer resolves this before indexing — that is what a `ja-address`
tokenizer is for, and why the specification puts normalization outside itself
and records which one was used.

**The corpus is synthetic and flatters the format.** `generate.py` draws town
and city names from small pools, so neighbouring addresses share far longer
prefixes than real ones do. Front-coding therefore does better here than it
will on the national address base registry. Treat 13.8 bytes per address as a
lower bound, not an estimate.

**Payload is now most of the file.** 1.3 MB of 1.9 MB, and it is coordinate
text: eighteen ASCII characters per record where a pair of scaled integers would
be eight bytes before compression. The format has nothing to say about that —
payload contents are outside the specification (`SPEC.md` §5.5) — but an address
product would encode coordinates rather than print them, and that is where the
next large saving is.
