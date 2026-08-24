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
bytes:    3049 of 12005031
```

Three range reads and about 3 KB, from a 12 MB file. Nothing was running.

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
| 8 | 1 | 1 | 1 | 3 | 3.0 KB |
| 20 | 1 | 1 | 1 | 3 | 3.1 KB |
| 100 | 2 | 1 | 2 | 5 | 6.7 KB |
| 500 | 5 | 1 | 8 | 14 | 22 KB |
| 2,000 | 17 | 1 | 32 | 50 | 83 KB |

Opening the file costs 2 reads and is paid once.

The dictionary column is `ceil(results / 128) + 1` and the payload column is
`ceil(results / 64)`, which is what `SPEC.md` §4.1 states. **Postings is always
1**, at every scale, because a contiguous term range has a contiguous run
range.

Nothing here grows with the size of the file. The same query costs the same
three reads against 300,000 addresses or 19 million.

## Sections

```
term_index                77 KB   resident
term_block_offsets        19 KB   resident
payload_offsets           37 KB   resident
meta                     261 B    resident
term_blocks              4.1 MB
postings                 2.0 MB
payload                  5.7 MB
                        ------
                        12.0 MB   from a 20 MB corpus
```

## What these numbers are not

**Compression is not implemented yet.** Every section is stored raw. `SPEC.md`
§5.6 provides for per-block compression against a shared dictionary, and the
two sections that would benefit most — payload at 5.7 MB of coordinate text,
term blocks at 4.1 MB — are exactly the ones not yet compressed. The size
column will move; the read column will not.

**The corpus is synthetic and flatters the format.** `generate.py` draws town
and city names from small pools, so neighbouring addresses share far longer
prefixes than real ones do. Front-coding therefore does better here than it
will on the national address base registry. Treat 13.8 bytes per address as a
lower bound, not an estimate.

**Postings are almost pure redundancy in this index.** With one term per
document, the postings section says nothing but "term *i* names document *i*" —
2 MB of the 12 MB. A codec that recognized the identity mapping would remove
it, and a dictionary-shaped index is common enough that it is worth
considering. It is not in the specification, and this is the measurement that
argues it should be.
