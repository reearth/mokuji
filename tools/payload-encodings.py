"""What a payload encoding would cost, measured on a real index.

Reads a corpus written by `colex dump`, so the payloads are in the document
order the index actually uses — which is term order, and therefore roughly
spatial order for addresses. That is the whole reason delta encoding works here.

Usage: python3 payload-encodings.py <dump.tsv>
"""

import sys
import zlib

BLOCK = 64
SCALE = 1_000_000

def varint(v):
    out = bytearray()
    while True:
        b = v & 0x7f; v >>= 7
        out.append(b | 0x80 if v else b)
        if not v: return bytes(out)

def zigzag(v):
    return (v << 1) ^ (v >> 63) if v < 0 else (v << 1)

def deflate(b):
    return len(zlib.compress(b, 9)[2:-4])

rows = []
for line in open(sys.argv[1] if len(sys.argv) > 1 else "dump.tsv", encoding="utf-8"):
    _, _, payload = line.rstrip("\n").partition("\t")
    lon, lat = payload.split(",")
    rows.append((payload, float(lon), float(lat)))
print(f"{len(rows):,} payloads")

def measure(name, encode_block):
    stored = raw = 0
    for i in range(0, len(rows), BLOCK):
        block = encode_block(rows[i:i + BLOCK])
        raw += len(block)
        stored += min(len(block), deflate(block) + 4)
    print(f"  {name:34} raw {raw/1e6:7.1f} MB   deflated {stored/1e6:7.1f} MB   "
          f"{stored/len(rows):5.2f} B/address")
    return stored

def text(group):
    out = bytearray()
    for p, _, _ in group:
        b = p.encode(); out += varint(len(b)) + b
    return bytes(out)

def text6(group):
    out = bytearray()
    for _, lon, lat in group:
        b = f"{lon:.6f},{lat:.6f}".encode(); out += varint(len(b)) + b
    return bytes(out)

def fixed(group):
    out = bytearray()
    for _, lon, lat in group:
        out += round(lon * SCALE).to_bytes(4, "little", signed=True)
        out += round(lat * SCALE).to_bytes(4, "little", signed=True)
    return bytes(out)

def fixed_delta(group):
    out = bytearray()
    plon = plat = None
    for _, lon, lat in group:
        x, y = round(lon * SCALE), round(lat * SCALE)
        if plon is None:
            out += varint(zigzag(x)) + varint(zigzag(y))
        else:
            out += varint(zigzag(x - plon)) + varint(zigzag(y - plat))
        plon, plat = x, y
    return bytes(out)

print("\npayload encodings, blocks of 64 documents in term order")
current = measure("text, as published (10 digits)", text)
measure("text, rounded to 6 decimals", text6)
measure("fixed point 1e-6, int32 pair", fixed)
delta = measure("fixed point 1e-6, zigzag varint delta", fixed_delta)
print(f"\n  delta against current: {current/delta:.1f}x smaller "
      f"({(current-delta)/1e6:.1f} MB off an 18.3 MB index)")

def soa_delta(group):
    """Longitudes then latitudes, so like values sit together."""
    out = bytearray()
    for values in (
        [round(lon * SCALE) for _, lon, _ in group],
        [round(lat * SCALE) for _, _, lat in group],
    ):
        previous = None
        for v in values:
            out += varint(zigzag(v if previous is None else v - previous))
            previous = v
    return bytes(out)

def coarse_delta(group, scale=100_000):
    out = bytearray()
    plon = plat = None
    for _, lon, lat in group:
        x, y = round(lon * scale), round(lat * scale)
        out += varint(zigzag(x if plon is None else x - plon))
        out += varint(zigzag(y if plat is None else y - plat))
        plon, plat = x, y
    return bytes(out)

print()
measure("fixed point 1e-6, delta, split streams", soa_delta)
measure("fixed point 1e-5 (~1 m), delta", coarse_delta)
