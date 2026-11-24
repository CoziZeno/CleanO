/**
 * Build a synthetic folder for benchmarking.
 *
 *   node scripts/make-test-tree.mjs <path> [fileCount]
 *
 * Deliberately realistic rather than uniform: a mix of extensions, a long tail
 * of sizes, some deliberate duplicates and some same-size-but-different files,
 * nested a few levels deep. A tree of 100k identical 1-byte files would make
 * every stage look fast for the wrong reasons.
 */

import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";

const target = process.argv[2];
const total = Number(process.argv[3] ?? 100_000);

if (!target) {
  console.error("usage: make-test-tree.mjs <path> [fileCount]");
  process.exit(2);
}

const EXTENSIONS = [
  "png", "jpg", "pdf", "mp4", "zip", "exe", "txt", "docx", "mp3", "ts",
  "json", "csv", "dmg", "7z", "webp", "mov", "xlsx", "", "tmp", "crdownload",
];

rmSync(target, { recursive: true, force: true });
mkdirSync(target, { recursive: true });

// ~250 folders, up to 4 levels deep.
const folders = [target];
for (let i = 0; i < 60; i++) {
  const parent = folders[Math.floor(Math.random() * folders.length)];
  const dir = join(parent, `folder-${i}`);
  mkdirSync(dir, { recursive: true });
  folders.push(dir);
  if (i % 3 === 0) {
    for (let j = 0; j < 3; j++) {
      const sub = join(dir, `sub-${j}`);
      mkdirSync(sub, { recursive: true });
      folders.push(sub);
    }
  }
}

// A handful of payloads reused across files, so duplicate detection has real
// work to do rather than finding nothing.
const payloads = [
  Buffer.alloc(1024, 1),
  Buffer.alloc(1024, 2), // same size as the above, different bytes
  Buffer.alloc(4096, 3),
  Buffer.alloc(16_384, 4),
];

let written = 0;
const started = Date.now();

for (let i = 0; i < total; i++) {
  const dir = folders[Math.floor(Math.random() * folders.length)];
  const ext = EXTENSIONS[i % EXTENSIONS.length];
  const name = ext ? `file-${i}.${ext}` : `file-${i}`;

  let body;
  if (i % 50 === 0) {
    body = payloads[i % payloads.length]; // deliberate duplicates
  } else {
    const size = 64 + (i % 2048);
    body = Buffer.alloc(size, i % 251);
  }

  try {
    writeFileSync(join(dir, name), body);
    written++;
  } catch {
    // Out of space or a path too long: stop rather than half-fail silently.
    break;
  }

  if (written % 20_000 === 0) {
    console.log(`  ${written} files...`);
  }
}

console.log(
  `wrote ${written} files across ${folders.length} folders in ${((Date.now() - started) / 1000).toFixed(1)}s`,
);
