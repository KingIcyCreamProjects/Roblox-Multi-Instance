// Copies the shared remote viewer from a HomeLab checkout into public/remote-viewer/ and records
// its SHA-256 in src/features/devices/viewer.sha256.json (checked by viewer.test.ts).
// Usage: node tools/sync-remote-viewer.mjs <path to the HomeLab checkout>
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const from = process.argv[2];
if (!from) {
  console.error("usage: node tools/sync-remote-viewer.mjs <HomeLab checkout>");
  process.exit(1);
}
const out = join(root, "public", "remote-viewer");
mkdirSync(out, { recursive: true });
const hashes = {};
for (const f of ["remote-viewer.js", "remote-viewer.css"]) {
  copyFileSync(join(from, "src", "web", f), join(out, f));
  hashes[f] = createHash("sha256")
    .update(readFileSync(join(out, f)))
    .digest("hex");
  console.log(`${f} ${hashes[f]}`);
}
writeFileSync(
  join(root, "src", "features", "devices", "viewer.sha256.json"),
  JSON.stringify(hashes, null, 2) + "\n",
);
