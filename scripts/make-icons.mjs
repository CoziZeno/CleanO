/**
 * Generate the app icon set with no image dependencies.
 *
 * Tauri's Windows build embeds `icons/icon.ico` as a resource at compile time,
 * so a missing icon is a build failure, not a cosmetic issue. Rather than
 * commit binary blobs nobody can review, this script rasterises the mark
 * directly and writes valid PNG, ICO and ICNS containers.
 *
 *   node scripts/make-icons.mjs
 *
 * Replace it with `npm run icons` (which calls `tauri icon`) once there is real
 * artwork in src-tauri/app-icon.png.
 */

import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const iconDir = join(here, "..", "src-tauri", "icons");

// --- the mark -------------------------------------------------------------
// A rounded square in the accent colour with three tidy bars of decreasing
// width. Readable at 32px, which is the only size that really has to work.

const ACCENT = [99, 102, 241]; // indigo-500, matches --accent in theme.css
const INK = [250, 250, 252];

function renderRGBA(size) {
  const px = Buffer.alloc(size * size * 4, 0);
  const radius = size * 0.22;
  const inset = size * 0.06;
  const min = inset;
  const max = size - inset;

  const bars = [
    { y: 0.36, x0: 0.26, x1: 0.74 },
    { y: 0.5, x0: 0.26, x1: 0.63 },
    { y: 0.64, x0: 0.26, x1: 0.52 },
  ];
  const barHeight = Math.max(1, size * 0.075);

  const set = (x, y, [r, g, b], a) => {
    const i = (y * size + x) * 4;
    // Simple source-over composite so antialiased edges blend correctly.
    const sa = a / 255;
    const dr = px[i], dg = px[i + 1], db = px[i + 2], da = px[i + 3] / 255;
    const outA = sa + da * (1 - sa);
    if (outA <= 0) return;
    px[i] = Math.round((r * sa + dr * da * (1 - sa)) / outA);
    px[i + 1] = Math.round((g * sa + dg * da * (1 - sa)) / outA);
    px[i + 2] = Math.round((b * sa + db * da * (1 - sa)) / outA);
    px[i + 3] = Math.round(outA * 255);
  };

  // Coverage of a rounded rect at a point, sampled 3x3 for antialiasing.
  const roundedCoverage = (x, y) => {
    let hits = 0;
    for (let sy = 0; sy < 3; sy++) {
      for (let sx = 0; sx < 3; sx++) {
        const px_ = x + (sx + 0.5) / 3;
        const py_ = y + (sy + 0.5) / 3;
        if (px_ < min || px_ > max || py_ < min || py_ > max) continue;
        const cx = Math.min(Math.max(px_, min + radius), max - radius);
        const cy = Math.min(Math.max(py_, min + radius), max - radius);
        const dx = px_ - cx;
        const dy = py_ - cy;
        if (dx * dx + dy * dy <= radius * radius) hits++;
      }
    }
    return hits / 9;
  };

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const coverage = roundedCoverage(x, y);
      if (coverage > 0) set(x, y, ACCENT, Math.round(coverage * 255));
    }
  }

  for (const bar of bars) {
    const y0 = bar.y * size - barHeight / 2;
    const y1 = y0 + barHeight;
    const x0 = bar.x0 * size;
    const x1 = bar.x1 * size;
    const r = barHeight / 2;
    for (let y = Math.floor(y0); y < Math.ceil(y1); y++) {
      for (let x = Math.floor(x0); x < Math.ceil(x1); x++) {
        if (x < 0 || y < 0 || x >= size || y >= size) continue;
        let hits = 0;
        for (let sy = 0; sy < 3; sy++) {
          for (let sx = 0; sx < 3; sx++) {
            const px_ = x + (sx + 0.5) / 3;
            const py_ = y + (sy + 0.5) / 3;
            if (py_ < y0 || py_ > y1) continue;
            const cx = Math.min(Math.max(px_, x0 + r), x1 - r);
            const dx = px_ - cx;
            const dy = py_ - (y0 + y1) / 2;
            if (dx * dx + dy * dy <= r * r || (px_ >= x0 + r && px_ <= x1 - r)) hits++;
          }
        }
        if (hits > 0) set(x, y, INK, Math.round((hits / 9) * 255));
      }
    }
  }

  return px;
}

// --- PNG ------------------------------------------------------------------

const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  return table;
})();

function crc32(buf) {
  let c = -1;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

function encodePNG(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  // Each scanline is prefixed with filter type 0 (none).
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    const at = y * (size * 4 + 1);
    raw[at] = 0;
    rgba.copy(raw, at + 1, y * size * 4, (y + 1) * size * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// --- ICO (PNG-in-ICO, valid for Windows Vista and later) ------------------

function encodeICO(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(entries.length, 4);

  const directory = Buffer.alloc(16 * entries.length);
  let offset = header.length + directory.length;
  entries.forEach((entry, i) => {
    const at = i * 16;
    directory[at] = entry.size >= 256 ? 0 : entry.size;
    directory[at + 1] = entry.size >= 256 ? 0 : entry.size;
    directory[at + 2] = 0; // palette
    directory[at + 3] = 0;
    directory.writeUInt16LE(1, at + 4); // colour planes
    directory.writeUInt16LE(32, at + 6); // bits per pixel
    directory.writeUInt32BE(0, at + 8);
    directory.writeUInt32LE(entry.png.length, at + 8);
    directory.writeUInt32LE(offset, at + 12);
    offset += entry.png.length;
  });

  return Buffer.concat([header, directory, ...entries.map((e) => e.png)]);
}

// --- ICNS -----------------------------------------------------------------

function encodeICNS(entries) {
  const blocks = entries.map((entry) => {
    const header = Buffer.alloc(8);
    header.write(entry.type, 0, 4, "ascii");
    header.writeUInt32BE(entry.png.length + 8, 4);
    return Buffer.concat([header, entry.png]);
  });
  const body = Buffer.concat(blocks);
  const header = Buffer.alloc(8);
  header.write("icns", 0, 4, "ascii");
  header.writeUInt32BE(body.length + 8, 4);
  return Buffer.concat([header, body]);
}

// --- write ----------------------------------------------------------------

mkdirSync(iconDir, { recursive: true });

const sizes = [16, 32, 48, 64, 128, 256, 512];
const png = new Map(sizes.map((size) => [size, encodePNG(size, renderRGBA(size))]));

const files = {
  "32x32.png": png.get(32),
  "128x128.png": png.get(128),
  "128x128@2x.png": png.get(256),
  "icon.png": png.get(512),
  "Square30x30Logo.png": png.get(32),
  "Square44x44Logo.png": png.get(48),
  "Square71x71Logo.png": png.get(64),
  "Square89x89Logo.png": png.get(128),
  "Square107x107Logo.png": png.get(128),
  "Square142x142Logo.png": png.get(256),
  "Square150x150Logo.png": png.get(256),
  "Square284x284Logo.png": png.get(512),
  "Square310x310Logo.png": png.get(512),
  "StoreLogo.png": png.get(64),
  "icon.ico": encodeICO([16, 32, 48, 64, 128, 256].map((size) => ({ size, png: png.get(size) }))),
  "icon.icns": encodeICNS([
    { type: "ic07", png: png.get(128) },
    { type: "ic08", png: png.get(256) },
    { type: "ic09", png: png.get(512) },
  ]),
};

for (const [name, data] of Object.entries(files)) {
  writeFileSync(join(iconDir, name), data);
}
writeFileSync(join(here, "..", "src-tauri", "app-icon.png"), png.get(512));

console.log(`wrote ${Object.keys(files).length + 1} icon files to src-tauri/icons`);
