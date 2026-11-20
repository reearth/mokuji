// The landing page's live parts. Every query here is answered in this page,
// by @reearth/mokuji reading .mokuji files on object storage with range
// requests. The only other request is /v1/datasets, once, to learn the
// files' current URLs.
//
// The files are Re:Earth Places' (https://places.reearth.land). How the
// Japanese files are asked mirrors that API's src/jp.js and src/reverse.js
// (reearth/reearth-places); if those change how a file is read, change this
// too.

import { MokujiIndex } from "./mokuji/index.js";

const ORIGIN = "https://places.reearth.land";
const DATASETS = `${ORIGIN}/v1/datasets`;

const $ = (id) => document.getElementById(id);
const html = document.documentElement;
const lang = () => html.dataset.lang;
const t = (en, ja) => (lang() === "ja" ? ja : en);
const escape = (s) =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

function size(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`;
  return `${(n / 1048576).toFixed(n < 10485760 ? 1 : 0)} MB`;
}
const plural = (n, en) => `${n} ${en}${n === 1 ? "" : "s"}`;

// --- reading a file, and seeing every request ---------------------------

/** A byte source that times each range request and reports it. The reader
 * counts requests either way; this adds which file, and how long. */
class Watched {
  constructor(url) {
    this.url = url;
    this.name = url.split("/").pop();
    this.length = undefined;
    this.listener = null;
  }
  async read(offset, length) {
    const start = performance.now();
    const response = await fetch(this.url, { headers: { Range: `bytes=${offset}-${offset + length - 1}` } });
    if (response.status !== 206) throw new Error(`${this.name}: the server answered ${response.status}, not 206`);
    const range = response.headers.get("Content-Range");
    if (range && this.length === undefined) this.length = Number(range.split("/")[1]);
    const bytes = new Uint8Array(await response.arrayBuffer());
    this.listener?.({ file: this.book, offset, length: bytes.length, ms: performance.now() - start });
    return bytes;
  }
}

/** One file: its reader, its source, and what opening it cost. */
class Book {
  constructor(url) {
    this.source = new Watched(url);
    this.source.book = this;
    this.index = null;
    this.opening = null;
    this.openCost = null;
    this.slices = []; // every range read so far: { offset, length, kind }
  }
  get name() {
    return this.source.name;
  }
  get length() {
    return this.source.length ?? 0;
  }
  open(log) {
    this.opening ??= (async () => {
      const mine = [];
      const outer = log ?? this.source.listener;
      this.source.listener = (r) => {
        r.open = true; // shown apart from the query's own reads
        mine.push(r);
        outer?.(r);
      };
      const start = performance.now();
      try {
        this.index = await MokujiIndex.open(this.source);
      } finally {
        this.source.listener = outer;
      }
      this.index.fileLength = this.source.length;
      this.openCost = {
        reads: mine.length,
        bytes: mine.reduce((n, r) => n + r.length, 0),
        ms: performance.now() - start,
      };
      for (const r of mine) this.slices.push({ offset: r.offset, length: r.length, kind: "resident" });
      return this;
    })();
    return this.opening;
  }
}

let catalog = null;
async function files() {
  catalog ??= fetch(DATASETS)
    .then((r) => r.json())
    .then(({ datasets }) => {
      const url = (id, role) => {
        const f = datasets.find((d) => d.id === id)?.files.find((f) => f.role === role);
        if (!f) throw new Error(`no ${role} file in ${id}`);
        return new Book(ORIGIN + f.url);
      };
      return {
        address: url("jp-address", "segment"),
        fuzzy: url("jp-address", "fuzzy"),
        postal: url("jp-postal", "segment"),
        planet: url("places", "planet"),
        reverse: url("jp-reverse", "segment"),
      };
    });
  return catalog;
}

// --- the shelf: each file drawn as a book's fore-edge -----------------------

// Sections read whole when a file is opened, and kept: the table of contents.
const RESIDENT = new Set([0x0001, 0x0010, 0x0012, 0x0013, 0x0031, 0x0032, 0x0033]);
const SECTION_NAMES = {
  0x0011: ["dictionary", "辞書"],
  0x0020: ["postings", "ポスティング"],
  0x0030: ["payload", "本文"],
  0x0060: ["positions", "位置"],
};

function regions(file) {
  const out = [];
  let head = 0;
  for (const [id, s] of file.index.sections) {
    if (s.length === 0) continue;
    if (RESIDENT.has(id)) head = Math.max(head, s.offset + s.length);
    else if (SECTION_NAMES[id]) out.push({ id, start: s.offset, end: s.offset + s.length });
  }
  out.sort((a, b) => a.start - b.start);
  return { head, sections: out };
}

function sectionOf(file, offset) {
  if (!file.index) return t("contents", "目次");
  const { head, sections } = regions(file);
  if (offset < head) return t("contents", "目次");
  const s = sections.find((s) => offset >= s.start && offset < s.end);
  return s ? t(...SECTION_NAMES[s.id]) : "";
}

function drawShelf(el, shown) {
  el.innerHTML = shown
    .filter((f) => f.index)
    .map((file) => {
      const L = file.length || 1;
      const pct = (n) => `${((n / L) * 100).toFixed(4)}%`;
      const { head, sections } = regions(file);
      // Labels flow left to right, each given room in proportion to its
      // section, so a tiny section's label cannot sit on top of the next.
      const labels = [{ name: t("contents", "目次"), w: head }, ...sections.map((s) => ({ name: t(...SECTION_NAMES[s.id]), w: s.end - s.start }))]
        .map((l) => `<span style="flex-grow:${l.w}">${escape(l.name)}</span>`)
        .join("");
      const lines = sections.map((s) => `<div class="book-section" style="left:${pct(s.start)}"></div>`).join("");
      const slices = file.slices
        .map(
          (s) =>
            `<div class="slice ${s.kind}" style="left:${pct(s.offset)};width:${pct(s.length)}" title="${escape(
              `${size(s.length)} @ ${s.offset.toLocaleString()}`,
            )}"></div>`,
        )
        .join("");
      // Each page this query pulled out carries a tag with what it weighed.
      // A tag that would collide steps up a row; with no row free it is left
      // off, and the page itself still shows. A tag is about 64 px wide.
      const room = 68 / Math.max(el.clientWidth, 1);
      const rowEnds = [-Infinity, -Infinity, -Infinity];
      const tags = file.slices
        .filter((s) => s.kind === "query")
        .sort((a, b) => a.offset - b.offset)
        .map((s) => {
          const at = s.offset / L;
          const row = rowEnds.findIndex((end) => at > end + room);
          if (row < 0) return null;
          rowEnds[row] = at;
          const flip = at > 0.86 ? " flip" : "";
          return { row, html: `<div class="tag r${row}${flip}" style="left:${pct(s.offset + s.length / 2)}"><span>${size(s.length)}</span></div>` };
        })
        .filter(Boolean)
        // Lower rows last, so their tags sit over the stems of the rows above.
        .sort((a, b) => b.row - a.row)
        .map((x) => x.html)
        .join("");
      return `<div class="book">
        <div class="book-title"><span>${escape(file.name)}</span><span class="size">${size(L)}</span></div>
        <div class="book-edge">${lines}<div class="head" style="width:${pct(head)}"></div>${slices}${tags}</div>
        <div class="book-labels">${labels}</div>
      </div>`;
    })
    .join("");
  el.dataset.count = String(shown.filter((f) => f.index).length);
}

/** Marks a file's earlier query reads as old, before a new query draws its own. */
function age(file) {
  for (const s of file.slices) if (s.kind === "query") s.kind = "old";
}

// --- the receipt: what a query cost -----------------------------------------

function drawReceipt(el, { reads, bytes, of, ms, opened }) {
  const stat = (value, unit, label) =>
    `<div class="stat"><span class="figure">${value}<small>${unit}</small></span><span class="label">${label}</span></div>`;
  const [n, u] = size(bytes).split(" ");
  const [fn, fu] = size(of).split(" ");
  const main =
    reads === null
      ? ""
      : `<div class="main">` +
        stat(reads, t("", "回"), t(reads === 1 ? "range read" : "range reads", "読み出し")) +
        stat(n, u, t("read", "読んだ量")) +
        stat(fn, fu, t("in the file", "ファイル全体")) +
        stat(Math.round(ms), "ms", t("time", "時間")) +
        `</div>`;
  const open = opened.length
    ? `<div class="opened">${opened
        .map((f) =>
          t(
            `Opened ${escape(f.name)}: ${plural(f.openCost.reads, "read")}, ${size(f.openCost.bytes)} — its table of contents, read once and kept`,
            `${escape(f.name)} を開いた：${f.openCost.reads} 回・${size(f.openCost.bytes)}（目次。最初に1度だけ読んで、手元に置く）`,
          ),
        )
        .join("<br>")}</div>`
    : "";
  el.innerHTML = main + open;
}

function drawLog(entries) {
  $("log").innerHTML = entries
    .map(
      (r) =>
        `<li><span class="what">${escape(r.file.name)} <em>bytes=${r.offset}-${r.offset + r.length - 1} · ${escape(
          sectionOf(r.file, r.offset),
        )}</em></span><span class="n">${size(r.length)}</span><span class="n">${Math.round(r.ms)} ms</span></li>`,
    )
    .join("");
}

function curl(entries) {
  return entries
    .map((r) => `curl -s -r ${r.offset}-${r.offset + r.length - 1} ${r.file.source.url} | xxd | head -4`)
    .join("\n");
}

// --- answering: Japan --------------------------------------------------------

// As reearth-places src/jp.js: a postal code is seven digits, typed with or
// without its separators.
const CODE_SEPARATORS = /[\s〒\-‐‑–—−ー－]/g;
function postalDigits(q) {
  const digits = q.normalize("NFKC").replace(CODE_SEPARATORS, "");
  return /^\d{1,7}$/.test(digits) ? digits : null;
}
// `1-9-1` read as `1丁目9-1`, as reearth-places src/jp.js chomeReading.
function chomeReading(normalized) {
  if (normalized.includes("丁目")) return null;
  const m = /^(\D+)(\d+)-(?=\d)/u.exec(normalized);
  return m ? `${m[1]}${m[2]}丁目${normalized.slice(m[0].length)}` : null;
}

/** Terms beginning with `normalized`; failing that, the longest term a
 * reading begins with (an address typed past what the file keys). As
 * reearth-places src/jp.js lookup. */
async function lookup(index, normalized, limit, readings = []) {
  let entries = await index.prefix(normalized, limit);
  let within = false;
  if (!entries.length && readings.length) {
    const prefixes = [
      ...new Set(
        readings.flatMap((r) => {
          const chars = Array.from(r);
          return chars.slice(1, -1).map((_, i) => chars.slice(0, i + 2).join(""));
        }),
      ),
    ];
    const found = prefixes.length ? await index.exactMany(prefixes) : [];
    let longest = null;
    found.forEach((entry, i) => {
      if (entry && (!longest || prefixes[i].length > longest.text.length)) longest = { entry, text: prefixes[i] };
    });
    if (longest) {
      entries = [longest.entry];
      within = true;
    }
  }
  const postings = await index.postingsSpan(entries);
  const decoder = new TextDecoder();
  const hits = [];
  entries.forEach((entry, i) => {
    const term = decoder.decode(entry.term);
    for (const doc of postings[i].docs) hits.push({ term, doc, within });
  });
  const shown = hits.slice(0, limit);
  const payloads = await index.payloadSpan(shown.map((h) => h.doc));
  return shown.map((h, i) => ({ ...h, payload: payloads[i] }));
}

const point = (lonlat) => {
  const [lon, lat] = String(lonlat).split(",").map(Number);
  return Number.isFinite(lat) ? `${lat.toFixed(4)}, ${lon.toFixed(4)}` : "";
};
const postalCode = (d) => (d.length === 7 ? `〒${d.slice(0, 3)}-${d.slice(3)}` : d);

async function answerJapan(q, f, limit) {
  const digits = postalDigits(q);
  if (digits) {
    await f.postal.open();
    const hits = await lookup(f.postal.index, digits, limit);
    return {
      used: [f.postal],
      rows: hits.map((h) => {
        const [code, address] = h.payload.split("|");
        return { entry: address, locator: postalCode(code) };
      }),
      note: hits.length ? "" : t("No postal code begins with that.", "その番号で始まる郵便番号はありません。"),
      code: `const postal = await MokujiIndex.open("${f.postal.source.url}");\nawait postal.lookup(${JSON.stringify(digits)}, ${limit});`,
    };
  }

  const index = f.address.index;
  const normalized = index.normalize(q);
  const chome = chomeReading(normalized);
  const hits = await lookup(index, normalized, limit, [normalized, ...(chome ? [chome] : [])]);
  if (hits.length) {
    const within = hits[0].within;
    return {
      used: [f.address],
      rows: hits.map((h) => ({ entry: h.term, locator: point(h.payload) })),
      note: within ? t("Found the town the address is in.", "その住所を含む町字が見つかりました。") : "",
      code: `const index = await MokujiIndex.open("${f.address.source.url}");\nawait index.lookup(${JSON.stringify(q)}, ${limit});`,
    };
  }

  // Nothing begins with it: the approximate index, by bigrams, as
  // reearth-places src/jp.js FUZZY.
  await f.fuzzy.open();
  const found = (
    await f.fuzzy.index.search(q, { any: true, probe: 6, lengthNorm: 0.75, boost: "municipality", limit })
  ).results;
  return {
    used: [f.address, f.fuzzy],
    rows: found.map((r) => {
      const cut = r.payload.lastIndexOf("|");
      return { entry: r.payload.slice(0, cut), locator: point(r.payload.slice(cut + 1)), match: "≈" };
    }),
    note: found.length
      ? t(
          "No address begins with that, so a second file answered, by the pairs of characters in it.",
          "その文字列で始まる住所はないので、2文字ずつの組で探す2つめのファイルが答えました。",
        )
      : t("Nothing like it either.", "似たものもありません。"),
    code: `const fuzzy = await MokujiIndex.open("${f.fuzzy.source.url}");\nawait fuzzy.search(${JSON.stringify(q)}, { any: true, probe: 6, boost: "municipality" });`,
  };
}

// --- answering: the world ------------------------------------------------------

// A payload in the places file: name, lon, lat, population, rank, id,
// labels (lang:name, separated by U+001F), country.
function place(payload) {
  const [name, lon, lat, , , id, labels = "", country = ""] = payload.split("\t");
  const local = Object.fromEntries(
    labels
      .split("\u001f")
      .map((l) => l.split(/:(.*)/s))
      .filter(([, v]) => v),
  );
  return { name, lon: Number(lon), lat: Number(lat), id, local, country };
}

async function answerWorld(q, f, limit) {
  const r = await f.planet.index.complete(q, limit + 2);
  const seen = new Set();
  const rows = [];
  for (const p of r.payloads.map(place)) {
    if (seen.has(p.id) || rows.length >= limit) continue;
    seen.add(p.id);
    const shown = lang() === "ja" ? p.local.ja || p.name : p.name;
    rows.push({
      entry: shown,
      small: p.country,
      locator: `${p.lat.toFixed(2)}, ${p.lon.toFixed(2)}`,
    });
  }
  return {
    used: [f.planet],
    rows,
    note: rows.length ? "" : t("No place name begins with that.", "その文字列で始まる地名はありません。"),
    code: `const planet = await MokujiIndex.open("${f.planet.source.url}");\nawait planet.complete(${JSON.stringify(q)}, ${limit + 2});`,
  };
}

// --- the hero ------------------------------------------------------------------

const MODES = {
  jp: {
    open: (f) => [f.address, f.postal],
    answer: answerJapan,
    placeholder: () => t("An address, or a postal code", "住所、または郵便番号"),
    examples: ["東京都千代田区丸の内", "100-0005", "和光市", "大阪市北区梅田"],
  },
  world: {
    open: (f) => [f.planet],
    answer: answerWorld,
    placeholder: () => t("A place, anywhere", "世界のどこかの地名"),
    examples: ["tokyo", "reykj", "san fr", "東京タ", "timbu", "Zürich"],
  },
};

const hero = {
  mode: null,
  chosen: false, // whether the visitor picked the mode, rather than the language
  f: null,
  last: null, // the last answer, kept to redraw it in the other language
  running: false,
  again: false,
  typing: null,
};

function defaultMode() {
  return lang() === "ja" ? "jp" : "world";
}

function orderTabs() {
  const modes = document.querySelector(".modes");
  const order = lang() === "ja" ? ["jp", "world"] : ["world", "jp"];
  for (const m of order) modes.append(modes.querySelector(`[data-mode="${m}"]`));
}

async function setMode(mode) {
  hero.mode = mode;
  document.querySelectorAll(".modes button").forEach((b) => b.setAttribute("aria-selected", String(b.dataset.mode === mode)));
  const m = MODES[mode];
  $("q").placeholder = m.placeholder();
  $("examples").innerHTML = m.examples.map((e) => `<button type="button">${escape(e)}</button>`).join("");
  $("results").innerHTML = "";
  $("note").textContent = "";
  $("q").value = "";
  hero.last = null;

  const f = hero.f;
  const opening = m.open(f);
  const opened = [];
  const log = [];
  await Promise.all(
    opening.map(async (file) => {
      const fresh = !file.opening;
      await file.open((r) => log.push(r));
      if (fresh) opened.push(file);
    }),
  );
  if (hero.mode !== mode) return;
  hero.last = { reads: null, opened, shelf: [opening[0]], log, code: "" };
  draw();
  credit(opening);
}

function credit(used) {
  const lines = [...new Set(used.map((f) => f.index?.attribution).filter(Boolean))];
  $("credit").textContent = lines.join(" · ");
}

function draw() {
  const a = hero.last;
  if (!a) return;
  if (a.rows) {
    $("results").innerHTML = a.rows
      .map(
        (r) =>
          `<li><span class="entry">${escape(r.entry)}${r.small ? `<small>${escape(r.small)}</small>` : ""}</span>` +
          `<span class="leader"></span>${r.match ? `<span class="match">${escape(r.match)}</span>` : ""}` +
          `<span class="locator">${escape(r.locator)}</span></li>`,
      )
      .join("");
    $("note").textContent = a.note ?? "";
  }
  drawReceipt($("receipt"), {
    reads: a.reads,
    bytes: a.bytes,
    of: a.shelf.reduce((n, f) => n + f.length, 0),
    ms: a.ms,
    opened: a.opened,
  });
  drawShelf($("shelf"), a.shelf);
  drawLog(a.log);
  $("code").firstElementChild.textContent = a.code || "";
}

async function query() {
  if (hero.running) {
    hero.again = true;
    return;
  }
  hero.running = true;
  try {
    do {
      hero.again = false;
      await once();
    } while (hero.again);
  } catch (err) {
    console.error(err);
    $("note").textContent = err.message;
  } finally {
    hero.running = false;
  }
}

async function once() {
  const q = $("q").value.trim();
  const mode = hero.mode;
  if (!q || !hero.f) {
    $("results").innerHTML = "";
    $("note").textContent = "";
    return;
  }
  const f = hero.f;
  const log = [];
  const all = Object.values(f);
  for (const file of all) {
    file.source.listener = (r) => log.push(r);
  }
  const touchedBefore = new Set(all.filter((x) => x.opening));
  const start = performance.now();
  let answer;
  try {
    answer = await MODES[mode].answer(q, f, 8);
  } catch (err) {
    $("note").textContent = err.message;
    return;
  }
  const ms = performance.now() - start;
  if (mode !== hero.mode || q !== $("q").value.trim()) return; // a later keystroke will draw

  const opened = answer.used.filter((x) => !touchedBefore.has(x));
  for (const file of all) age(file);
  // Reads made by opening a file are on its shelf already, as resident.
  const queryReads = log.filter((r) => !r.open);
  for (const r of queryReads) r.file.slices.push({ offset: r.offset, length: r.length, kind: "query" });

  hero.last = {
    ...answer,
    reads: queryReads.length,
    bytes: queryReads.reduce((n, r) => n + r.length, 0),
    ms,
    opened,
    shelf: answer.used,
    log,
  };
  draw();
  credit(answer.used);
}

/** Types the first example into the box, a keystroke at a time, each one a
 * real query. Stops as soon as the visitor does anything. */
function demo() {
  const text = MODES[hero.mode].examples[0];
  const chars = Array.from(text);
  if (reduced) {
    $("q").value = text;
    query();
    return;
  }
  let i = 0;
  hero.typing = setInterval(() => {
    $("q").value = chars.slice(0, ++i).join("");
    query();
    if (i >= chars.length) stopDemo();
  }, 140);
}
function stopDemo() {
  clearInterval(hero.typing);
  hero.typing = null;
}

async function startHero() {
  try {
    hero.f = await files();
  } catch (err) {
    $("note").textContent = t(`Could not reach the files: ${err.message}`, `ファイルに届きませんでした：${err.message}`);
    return;
  }
  orderTabs();
  await setMode(defaultMode());
  demo();

  document.querySelector(".modes").addEventListener("click", async (e) => {
    const b = e.target.closest("button[data-mode]");
    if (!b || b.dataset.mode === hero.mode) return;
    stopDemo();
    hero.chosen = true;
    await setMode(b.dataset.mode);
    $("q").focus();
  });
  $("examples").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    stopDemo();
    $("q").value = b.textContent;
    query();
  });
  $("q").addEventListener("focus", stopDemo);
  $("q").addEventListener("input", () => {
    stopDemo();
    query();
  });
  $("copy-curl").addEventListener("click", async () => {
    const text = curl(hero.last?.log ?? []);
    try {
      await navigator.clipboard.writeText(text);
      flash($("copy-curl"), t("Copied", "コピーしました"));
    } catch {
      flash($("copy-curl"), t("Could not copy", "コピーできませんでした"));
    }
  });
}

function flash(button, text) {
  const span = button.querySelector(`[lang="${lang()}"]`);
  const was = span.textContent;
  span.textContent = text;
  setTimeout(() => (span.textContent = was), 1400);
}

// --- reverse geocoding ------------------------------------------------------

// The geohash cell a point falls in, and the polygons' rings, as
// reearth-places src/reverse.js and mokuji demo/web/reverse.js read them.
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
function decodeRings(text, scale) {
  return text.split(";").map((part) => {
    const v = part.split(",");
    const ring = [];
    let x = 0;
    let y = 0;
    for (let i = 0; i + 1 < v.length; i += 2) {
      x += Number(v[i]);
      y += Number(v[i + 1]);
      ring.push([x / scale, y / scale]);
    }
    ring.push(ring[0]);
    return ring;
  });
}
function contains(rings, x, y) {
  let inside = false;
  for (const ring of rings) {
    for (let i = 0; i < ring.length - 1; i++) {
      const [x1, y1] = ring[i];
      const [x2, y2] = ring[i + 1];
      if (y1 > y !== y2 > y && x < x1 + ((y - y1) * (x2 - x1)) / (y2 - y1)) inside = !inside;
    }
  }
  return inside;
}
async function locate(index, lat, lon) {
  const precisions = String(index.meta?.attrs?.["geohash-precisions"] ?? "5,6,7").split(",").map(Number);
  const scale = 10 ** Number(/1e(\d+)/.exec(String(index.meta?.attrs?.geometry ?? "1e5"))?.[1] ?? 5);
  const cell = geohash(lat, lon, Math.max(...precisions));
  const found = (await index.exactMany(precisions.map((p) => cell.slice(0, p)))).filter(Boolean);
  const postings = await Promise.all(
    found.map((e) => (e.postingsLength === 0 ? index.implied(e) : index.postingsSpan([e]).then(([p]) => p))),
  );
  const docs = [...new Set(postings.flatMap((p) => p.docs))].sort((a, b) => a - b);
  const payloads = await index.payloadSpan(docs);
  const candidates = docs.map((doc, i) => {
    const cut = payloads[i].indexOf("|");
    return { label: payloads[i].slice(0, cut), rings: decodeRings(payloads[i].slice(cut + 1), scale) };
  });
  return { cell, candidates, inside: candidates.filter((c) => contains(c.rings, lon, lat)) };
}

const MAPLIBRE = "https://unpkg.com/maplibre-gl@5.24.0/dist";
const dark = matchMedia("(prefers-color-scheme: dark)");
const mapStyle = () => `https://tiles.openfreemap.org/styles/${dark.matches ? "dark" : "positron"}`;

function load(tag, attrs) {
  return new Promise((resolve, reject) => {
    const el = Object.assign(document.createElement(tag), attrs, { onload: resolve, onerror: reject });
    document.head.append(el);
  });
}

const rev = { file: null, last: null, map: null };

function drawReverse() {
  const a = rev.last;
  if (!a) return;
  const place = a.inside[0];
  $("reverse-answer").innerHTML = place
    ? `${escape(place.label)}<small>${a.lat.toFixed(5)}, ${a.lon.toFixed(5)} · geohash ${escape(a.cell)}</small>`
    : `${escape(t("In no 町丁目", "どの町丁目にも入っていません"))}<small>${a.lat.toFixed(5)}, ${a.lon.toFixed(5)}</small>`;
  drawReceipt($("reverse-receipt"), { reads: a.reads, bytes: a.bytes, of: rev.file.length, ms: a.ms, opened: a.opened });
  drawShelf($("reverse-shelf"), [rev.file]);
}

async function reverseAt(lat, lon) {
  const file = rev.file;
  const log = [];
  file.source.listener = (r) => log.push(r);
  const start = performance.now();
  const found = await locate(file.index, lat, lon);
  const ms = performance.now() - start;
  age(file);
  for (const r of log) file.slices.push({ offset: r.offset, length: r.length, kind: "query" });
  rev.last = {
    ...found,
    lat,
    lon,
    ms,
    reads: log.length,
    bytes: log.reduce((n, r) => n + r.length, 0),
    opened: rev.last ? [] : [file],
  };
  drawReverse();

  const features = found.inside.map((c) => ({
    type: "Feature",
    properties: {},
    geometry: { type: "Polygon", coordinates: c.rings },
  }));
  rev.map.getSource("found")?.setData({ type: "FeatureCollection", features });
  rev.map.getSource("point")?.setData({ type: "Point", coordinates: [lon, lat] });
}

function addLayers(map) {
  const shu = getComputedStyle(html).getPropertyValue("--shu").trim();
  map.addSource("found", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addSource("point", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addLayer({ id: "found-fill", type: "fill", source: "found", paint: { "fill-color": shu, "fill-opacity": 0.14 } });
  map.addLayer({ id: "found-line", type: "line", source: "found", paint: { "line-color": shu, "line-width": 2 } });
  map.addLayer({
    id: "point",
    type: "circle",
    source: "point",
    paint: { "circle-radius": 5, "circle-color": shu, "circle-stroke-color": "#fff", "circle-stroke-width": 2 },
  });
}

async function startReverse() {
  try {
    const f = await files();
    rev.file = f.reverse;
    await Promise.all([
      rev.file.open(),
      load("link", { rel: "stylesheet", href: `${MAPLIBRE}/maplibre-gl.css` }),
      load("script", { src: `${MAPLIBRE}/maplibre-gl.js` }),
    ]);
  } catch (err) {
    $("reverse-answer").textContent = err.message;
    return;
  }
  $("reverse-credit").textContent = rev.file.index.attribution ?? "";
  const start = [139.7671, 35.6812];
  const map = new maplibregl.Map({
    container: "map",
    style: mapStyle(),
    center: start,
    zoom: 13.5,
    attributionControl: { compact: true },
    cooperativeGestures: matchMedia("(pointer: coarse)").matches,
  });
  rev.map = map;
  map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");
  map.on("load", () => {
    // Compact attribution starts open; on a phone it covers the map.
    map.getContainer().querySelector(".maplibregl-compact-show")?.classList.remove("maplibregl-compact-show");
    addLayers(map);
    reverseAt(start[1], start[0]);
  });
  map.on("click", (e) => reverseAt(e.lngLat.lat, e.lngLat.lng));
  dark.addEventListener("change", () => {
    map.setStyle(mapStyle());
    map.once("style.load", () => {
      addLayers(map);
      if (rev.last) reverseAt(rev.last.lat, rev.last.lon);
    });
  });
}

// --- the page ----------------------------------------------------------------

// The switch is a link to the other page; following it is a choice, kept so
// that / stops sending this visitor to /ja/, or starts.
document.querySelectorAll("[data-set-lang]").forEach((a) =>
  a.addEventListener("click", () => {
    try {
      localStorage.setItem("mokuji-lang", a.dataset.setLang);
    } catch {}
  }),
);

const bar = document.querySelector(".bar");
addEventListener("scroll", () => bar.classList.toggle("scrolled", scrollY > 8), { passive: true });

// Back to top without leaving a # in the address.
document.querySelector(".to-top").addEventListener("click", (e) => {
  e.preventDefault();
  scrollTo({ top: 0 });
});

startHero();

// The map and its file load when the section comes near.
new IntersectionObserver(
  (entries, observer) => {
    if (entries.some((e) => e.isIntersecting)) {
      observer.disconnect();
      startReverse();
    }
  },
  { rootMargin: "400px" },
).observe($("reverse"));
