// The figures in "One layout, many kinds of search": what a term is, in each
// kind. Each is drawn from the mechanism it shows, not from a picture of it:
// the geohash cells are computed for the point, the bigrams cut from the
// strings, the sorted lists sorted. The point clouds are seeded noise.

const W = 320;
const H = 200;
const svg = (body) =>
  `<svg viewBox="0 0 ${W} ${H}" role="img" aria-hidden="true" preserveAspectRatio="xMidYMid meet">${body}</svg>`;
const esc = (s) => String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);

/** Seeded noise, so a figure is the same at every visit. */
function rng(seed) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// --- a prefix is a range --------------------------------------------------------

function address() {
  const prefix = "東京都千代田区丸の内";
  const rows = [
    "東京都中央区銀座1丁目",
    "東京都千代田区一番町",
    "東京都千代田区丸の内1丁目",
    "東京都千代田区丸の内2丁目",
    "東京都千代田区丸の内3丁目",
    "東京都千代田区有楽町1丁目",
    "東京都港区赤坂1丁目",
  ].sort();
  const y0 = 30;
  const dy = 22;
  const hit = rows.map((r) => r.startsWith(prefix));
  const first = hit.indexOf(true);
  const last = hit.lastIndexOf(true);
  const body = rows
    .map((r, i) => {
      const y = y0 + i * dy;
      const tail = hit[i] ? `<tspan class="hi-t">${esc(prefix)}</tspan>${esc(r.slice(prefix.length))}` : esc(r);
      return `<line class="rule" x1="20" x2="300" y1="${y + 7}" y2="${y + 7}"/><text class="tx-ja" x="20" y="${y}">${tail}</text>`;
    })
    .join("");
  const top = y0 + first * dy - 14;
  const bottom = y0 + last * dy + 4;
  return svg(
    `${body}<rect class="hi-band" x="14" y="${top}" width="244" height="${bottom - top}" rx="2"/>` +
      `<path class="hi-s" d="M268 ${top} h8 v${bottom - top} h-8"/>` +
      `<text class="tx-m hi-t" x="282" y="${(top + bottom) / 2 + 3}">1×</text>`,
  );
}

// --- two key spaces, one dictionary ---------------------------------------

function postal() {
  const x0 = 20;
  const x1 = 300;
  const split = 132;
  const y = 92;
  const lines = [];
  for (let x = x0; x < x1; x += 3) lines.push(`<line class="page" x1="${x}" x2="${x}" y1="${y}" y2="${y + 30}"/>`);
  return svg(
    lines.join("") +
      `<line class="sep" x1="${split}" x2="${split}" y1="${y - 4}" y2="${y + 34}"/>` +
      `<text class="tx-m" x="${x0}" y="${y + 48}">0000000 … 9999999</text>` +
      `<text class="tx-ja dim" x="${split + 8}" y="${y + 48}">三重県… 東京都… 北海道…</text>` +
      // the two queries, one into each space
      `<text class="tx-m hi-t" x="48" y="34">100-0005</text>` +
      `<path class="hi-s" d="M72 42 V${y - 2}"/><rect class="hi" x="70" y="${y}" width="4" height="30"/>` +
      `<text class="tx-ja hi-t" x="196" y="34">東京都千代田区丸の内</text>` +
      `<path class="hi-s" d="M226 42 V${y - 2}"/><rect class="hi" x="224" y="${y}" width="4" height="30"/>` +
      // and the record both reach
      `<path class="hi-s thin" d="M72 ${y + 56} C72 182, 150 172, 160 182 M226 ${y + 56} C226 182, 170 172, 160 182"/>` +
      `<circle class="hi" cx="160" cy="184" r="3"/>`,
  );
}

// --- pairs of characters ------------------------------------------------------

function bigrams(s) {
  const c = Array.from(s);
  return c.slice(0, -1).map((ch, i) => ch + c[i + 1]);
}

function fuzzy() {
  const typed = bigrams("千代田区丸乃内");
  const stored = bigrams("千代田区丸の内");
  const w = 40;
  const gap = 7;
  const x0 = (W - (typed.length * w + (typed.length - 1) * gap)) / 2;
  const chip = (g, i, y, on) =>
    `<rect class="${on ? "hi" : "chip"}" x="${x0 + i * (w + gap)}" y="${y}" width="${w}" height="26" rx="3"/>` +
    `<text class="tx-ja ${on ? "on" : "dim"}" x="${x0 + i * (w + gap) + w / 2}" y="${y + 17}" text-anchor="middle">${esc(g)}</text>`;
  const match = typed.map((g) => stored.includes(g));
  return svg(
    typed.map((g, i) => chip(g, i, 42, match[i])).join("") +
      typed
        .map((g, i) => (match[i] ? `<line class="hi-s" x1="${x0 + i * (w + gap) + w / 2}" x2="${x0 + i * (w + gap) + w / 2}" y1="70" y2="128"/>` : ""))
        .join("") +
      stored.map((g, i) => chip(g, i, 130, stored[i] === typed[i])).join("") +
      `<text class="tx-m dim" x="${x0}" y="32">千代田区丸乃内</text>` +
      `<text class="tx-m dim" x="${x0}" y="176">千代田区丸の内</text>` +
      `<text class="tx-m hi-t" x="${W - x0}" y="176" text-anchor="end">${match.filter(Boolean).length} / ${typed.length}</text>`,
  );
}

// --- geohash: a cell's prefixes are the cells holding it -----------------

const BASE32 = "0123456789bcdefghjkmnpqrstuvwxyz";
function geohash(lat, lon, precision) {
  let [lonLo, lonHi, latLo, latHi] = [-180, 180, -90, 90];
  let out = "";
  let code = 0;
  for (let i = 0; i < precision * 5; i++) {
    if (i % 2 === 0) {
      const mid = (lonLo + lonHi) / 2;
      code = code * 2 + (lon >= mid ? 1 : 0);
      lon >= mid ? (lonLo = mid) : (lonHi = mid);
    } else {
      const mid = (latLo + latHi) / 2;
      code = code * 2 + (lat >= mid ? 1 : 0);
      lat >= mid ? (latLo = mid) : (latHi = mid);
    }
    if (i % 5 === 4) {
      out += BASE32[code];
      code = 0;
    }
  }
  return out;
}
function bounds(hash) {
  let [lonLo, lonHi, latLo, latHi] = [-180, 180, -90, 90];
  let even = true;
  for (const ch of hash) {
    const v = BASE32.indexOf(ch);
    for (let b = 4; b >= 0; b--) {
      const bit = (v >> b) & 1;
      if (even) {
        const mid = (lonLo + lonHi) / 2;
        bit ? (lonLo = mid) : (lonHi = mid);
      } else {
        const mid = (latLo + latHi) / 2;
        bit ? (latLo = mid) : (latHi = mid);
      }
      even = !even;
    }
  }
  return { lonLo, lonHi, latLo, latHi };
}

function reverse() {
  const [lat, lon] = [35.6812, 139.7671]; // Tokyo Station
  const c5 = geohash(lat, lon, 5);
  const c6 = geohash(lat, lon, 6);
  const c7 = geohash(lat, lon, 7);
  const b5 = bounds(c5);
  const side = 168;
  const ox = 24;
  const oy = 16;
  const X = (x) => ox + ((x - b5.lonLo) / (b5.lonHi - b5.lonLo)) * side;
  const Y = (y) => oy + ((b5.latHi - y) / (b5.latHi - b5.latLo)) * side;
  const rect = (b, cls) =>
    `<rect class="${cls}" x="${X(b.lonLo)}" y="${Y(b.latHi)}" width="${X(b.lonHi) - X(b.lonLo)}" height="${Y(b.latLo) - Y(b.latHi)}"/>`;
  const grid = (b, cols, rows, cls) => {
    const out = [];
    for (let i = 1; i < cols; i++) {
      const x = X(b.lonLo + ((b.lonHi - b.lonLo) * i) / cols);
      out.push(`<line class="${cls}" x1="${x}" x2="${x}" y1="${Y(b.latHi)}" y2="${Y(b.latLo)}"/>`);
    }
    for (let j = 1; j < rows; j++) {
      const y = Y(b.latLo + ((b.latHi - b.latLo) * j) / rows);
      out.push(`<line class="${cls}" x1="${X(b.lonLo)}" x2="${X(b.lonHi)}" y1="${y}" y2="${y}"/>`);
    }
    return out.join("");
  };
  const b6 = bounds(c6);
  const b7 = bounds(c7);
  // Precision 5 to 6 splits longitude in 4 and latitude in 8; 6 to 7, the
  // other way round, as geohash alternates the bits.
  const label = (text, x, y, cls = "") => `<text class="tx-m ${cls}" x="${x}" y="${y}">${esc(text)}</text>`;
  const lx = ox + side + 18;
  return svg(
    rect(b5, "cell") +
      grid(b5, 4, 8, "grid") +
      rect(b6, "hi-band") +
      grid(b6, 8, 4, "grid fine") +
      rect(b7, "hi") +
      `<circle class="pt" cx="${X(lon)}" cy="${Y(lat)}" r="2.5"/>` +
      label(c5, lx, 40, "dim") +
      label(c6, lx, 64, "dim") +
      label(c7, lx, 88, "hi-t") +
      `<path class="hi-s thin" d="M${X(b7.lonHi) + 2} ${(Y(b7.latHi) + Y(b7.latLo)) / 2} L${lx - 6} 85"/>` +
      label("3 lookups", lx, 130, "dim"),
  );
}

// --- postings intersect; BM25 ranks ------------------------------------------

function bm25() {
  const r = rng(7);
  const n = 34;
  const x = (i) => 64 + (i * 226) / (n - 1);
  const a = new Set();
  const b = new Set();
  for (let i = 0; i < n; i++) {
    if (r() < 0.45) a.add(i);
    if (r() < 0.4) b.add(i);
  }
  const both = [...a].filter((i) => b.has(i));
  const row = (set, y) =>
    [...set].map((i) => `<rect class="${both.includes(i) ? "hi" : "mark"}" x="${x(i) - 2}" y="${y}" width="4" height="12" rx="1"/>`).join("");
  const scores = both.map(() => 0.35 + r() * 0.65).sort((p, q) => q - p).slice(0, 4);
  return svg(
    `<line class="rule" x1="60" x2="294" y1="46" y2="46"/><line class="rule" x1="60" x2="294" y1="76" y2="76"/>` +
      `<text class="tx-ja dim" x="20" y="45">建築</text>` +
      row(a, 34) +
      `<text class="tx-ja dim" x="20" y="75">築物</text>` +
      row(b, 64) +
      both.map((i) => `<line class="hi-s thin" x1="${x(i)}" x2="${x(i)}" y1="80" y2="104"/>`).join("") +
      scores
        .map((s, k) => `<rect class="${k === 0 ? "hi" : "bar"}" x="64" y="${116 + k * 18}" width="${s * 226}" height="10" rx="1"/>`)
        .join("") +
      `<text class="tx-m dim" x="20" y="125">BM25</text>`,
  );
}

// --- IVF: a cluster is a term ------------------------------------------------

function vector() {
  const r = rng(11);
  const gauss = () => (r() + r() + r() - 1.5) * 0.9;
  // Cluster centres on a jittered grid, so the clusters read as clusters.
  const centres = Array.from({ length: 8 }, (_, i) => [52 + (i % 4) * 72 + (r() - 0.5) * 30, 56 + Math.floor(i / 4) * 88 + (r() - 0.5) * 26]);
  const q = [150, 96];
  const d = (p) => Math.hypot(p[0] - q[0], p[1] - q[1]);
  const near = centres
    .map((c, i) => [d(c), i])
    .sort((p, s) => p[0] - s[0])
    .slice(0, 2)
    .map(([, i]) => i);
  const dots = [];
  centres.forEach((c, i) => {
    for (let k = 0; k < 22; k++) {
      const p = [c[0] + gauss() * 17, c[1] + gauss() * 15];
      dots.push(`<circle class="${near.includes(i) ? "hi" : "dot"}" cx="${p[0].toFixed(1)}" cy="${p[1].toFixed(1)}" r="1.7"/>`);
    }
  });
  return svg(
    dots.join("") +
      centres
        .map(
          (c, i) =>
            `<path class="${near.includes(i) ? "hi-s" : "cross"}" d="M${c[0] - 4} ${c[1]} h8 M${c[0]} ${c[1] - 4} v8"/>` +
            (near.includes(i) ? `<line class="hi-s thin dash" x1="${q[0]}" y1="${q[1]}" x2="${c[0]}" y2="${c[1]}"/>` : ""),
        )
        .join("") +
      `<circle class="pt ring" cx="${q[0]}" cy="${q[1]}" r="5"/><circle class="pt" cx="${q[0]}" cy="${q[1]}" r="2"/>`,
  );
}

// --- dates are terms; a period is a range -----------------------------------

function stac() {
  const r = rng(3);
  const x0 = 24;
  const x1 = 296;
  const months = 36; // 2023-01 to 2025-12
  const mx = (m) => x0 + ((x1 - x0) * m) / months;
  const from = 12;
  const to = 18; // 2024-01 to 2024-06
  const ticks = [];
  for (let m = 0; m < months; m += 1 / 3) {
    if (r() < 0.55) {
      const on = m >= from && m < to;
      const h = 10 + r() * 22;
      ticks.push(`<rect class="${on ? "hi" : "mark"}" x="${mx(m).toFixed(1)}" y="${120 - h}" width="2" height="${h}"/>`);
    }
  }
  const years = [2023, 2024, 2025]
    .map((y, i) => `<line class="rule" x1="${mx(i * 12)}" x2="${mx(i * 12)}" y1="124" y2="132"/><text class="tx-m dim" x="${mx(i * 12) + 3}" y="146">${y}</text>`)
    .join("");
  return svg(
    `<line class="axis" x1="${x0}" x2="${x1}" y1="122" y2="122"/>` +
      ticks.join("") +
      years +
      `<path class="hi-s" d="M${mx(from)} 74 v-8 H${mx(to)} v8"/>` +
      `<text class="tx-m hi-t" x="${mx(from)}" y="58">d:2024-01 … d:2024-06</text>` +
      `<text class="tx-m dim" x="${x0}" y="178">1 range</text>`,
  );
}

// --- your own --------------------------------------------------------------

function yours() {
  const lines = ["tokyo chiyoda marunouchi\t35.681,139.767", "tokyo chuo ginza\t35.671,139.772", "osaka kita umeda\t34.702,135.496"];
  const rows = lines
    .map((l, i) => {
      const [text, payload] = l.split("\t");
      return `<text class="tx-m" x="20" y="${46 + i * 18}">${esc(text)}</text><text class="tx-m dim" x="176" y="${46 + i * 18}">${esc(payload)}</text>`;
    })
    .join("");
  const pages = [];
  for (let x = 100; x < 220; x += 3) pages.push(`<line class="page" x1="${x}" x2="${x}" y1="132" y2="172"/>`);
  return svg(
    `<text class="tx-m dim" x="20" y="24">places.tsv</text>` +
      rows +
      `<path class="hi-s" d="M160 104 v18 m-5 -6 l5 6 l5 -6"/>` +
      pages.join("") +
      `<text class="tx-m dim" x="228" y="168">places.mokuji</text>`,
  );
}

const FIGURES = { address, postal, fuzzy, reverse, bm25, vector, stac, yours };

for (const el of document.querySelectorAll("[data-figure]")) {
  el.innerHTML = FIGURES[el.dataset.figure]?.() ?? "";
}
