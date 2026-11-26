// Builds the programs shipped next to the desktop application (Tauri
// "externalBin"): the daemon `devspaced`, the CLI `devspace` and the
// privileged helper `devspace-helper`, in release mode, for the target of
// the bundle, and copies them to `src-tauri/binaries/<name>-<target>[.exe]`
// where the Tauri bundler expects them.
//
// Run by `beforeBuildCommand` of `tauri.bundle.conf.json`. The target is
// `TAURI_ENV_TARGET_TRIPLE` (set by `tauri build`, also with `--target`),
// otherwise the host of `rustc`.

import { execFileSync } from "node:child_process";
import { copyFileSync, chmodSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const programs = [
  { pkg: "devspace-daemon", bin: "devspaced" },
  { pkg: "devspace-cli", bin: "devspace" },
  { pkg: "devspace-helper", bin: "devspace-helper" },
];

const here = dirname(fileURLToPath(import.meta.url));
const workspace = resolve(here, "..", "..", "..");
const binaries = resolve(here, "..", "src-tauri", "binaries");

function hostTarget() {
  const info = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const line = info.split("\n").find((l) => l.startsWith("host: "));
  if (!line) {
    throw new Error("cannot determine the host target from `rustc -vV`");
  }
  return line.slice("host: ".length).trim();
}

const target = process.env.TAURI_ENV_TARGET_TRIPLE || hostTarget();
const exe = target.includes("windows") ? ".exe" : "";

const args = ["build", "--release", "--locked", "--target", target];
for (const { pkg } of programs) {
  args.push("-p", pkg);
}
console.log(`sidecars: cargo ${args.join(" ")}`);
execFileSync("cargo", args, { cwd: workspace, stdio: "inherit" });

const targetDir = process.env.CARGO_TARGET_DIR
  ? resolve(process.env.CARGO_TARGET_DIR)
  : join(workspace, "target");
mkdirSync(binaries, { recursive: true });
for (const { bin } of programs) {
  const from = join(targetDir, target, "release", bin + exe);
  const to = join(binaries, `${bin}-${target}${exe}`);
  copyFileSync(from, to);
  if (!exe) {
    chmodSync(to, 0o755);
  }
  console.log(`sidecars: ${to}`);
}
