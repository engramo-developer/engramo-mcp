// `npm audit --audit-level=high` with a reviewed allowlist. npm has no native
// way to ignore an advisory, and one with no upstream fix would otherwise block
// every release. Fails on any high/critical advisory not listed below.
import { execFileSync } from "node:child_process";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

// Each entry needs a reason; drop it as soon as a fixed version ships.
const ALLOWED = {
  // zod: unbounded issue accumulation on oversized arrays can exhaust memory.
  // Fixed only in zod 4.6.6; @anthropic-ai/mcpb 2.1.2 (latest) requires
  // zod ^3.25.67 and no 3.x patch exists. mcpb's zod only validates our own
  // checked-in manifest.json at pack time — never untrusted input.
  "GHSA-mrhx-c7xc-m3w5": "zod memory exhaustion, only parses our manifest",
};
const BLOCKING = new Set(["high", "critical"]);

let raw;
try {
  raw = execFileSync("npm", ["audit", "--json"], {
    cwd: dirname(fileURLToPath(import.meta.url)),
    encoding: "utf8",
    shell: process.platform === "win32",
  });
} catch (e) {
  // npm audit exits non-zero when it finds anything; the report is on stdout.
  if (!e.stdout) throw e;
  raw = e.stdout;
}
const report = JSON.parse(raw);
if (report.error) {
  console.error("npm audit failed:", report.error.summary ?? report.error);
  process.exit(2);
}

const advisories = new Map();
for (const vuln of Object.values(report.vulnerabilities ?? {})) {
  // String entries are transitive "depends on <pkg>" links; objects are advisories.
  for (const via of vuln.via) {
    if (typeof via === "object") advisories.set(via.url, via);
  }
}

let failed = false;
for (const adv of advisories.values()) {
  const id = adv.url.split("/").pop();
  const blocking = BLOCKING.has(adv.severity);
  const status = !blocking ? "ok (below high)" : ALLOWED[id] ? "allowed" : "FAIL";
  if (status === "FAIL") failed = true;
  console.log(`${status.padEnd(16)} ${adv.severity.padEnd(8)} ${adv.name}: ${adv.title} (${adv.url})`);
}

const present = new Set([...advisories.values()].map((a) => a.url.split("/").pop()));
for (const id of Object.keys(ALLOWED)) {
  if (!present.has(id)) console.log(`note: allowlisted ${id} no longer reported — remove it`);
}

if (failed) {
  console.error("Unallowed high/critical npm advisories in mcpb/tools.");
  process.exit(1);
}
console.log("mcpb tools audit passed.");
