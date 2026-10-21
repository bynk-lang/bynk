// Server provisioning for the Bynk LSP.
//
// Resolution order, most explicit first:
//   1. `bynk.executablePath` setting (absolute path the user pinned)
//   2. `bynkc-lsp` on PATH (a dev/global install)
//   3. a previously-downloaded binary cached in the extension's global storage
//   4. download-on-activate: fetch the matching per-platform `bynkc-lsp` from
//      this version's GitHub Release, verify it against the release's
//      SHA256SUMS, cache it, and use it.
//
// The download fetches the *raw* binary asset (`bynkc-lsp-<target>[.exe]`), not
// an archive, so the extension needs no tar/zip handling — just fetch + verify
// + chmod.
//
// The same mechanism provisions the rest of the toolchain (`bynk`, `bynkc`),
// so a user needs no `cargo install` to check, test, run and deploy: the three
// land side by side in one versioned directory (which is also how the `bynk`
// driver finds its sibling `bynkc`), and `bynk` is resolved the way the server
// is — `bynk.bynkPath`, then PATH, then that managed copy.

import * as path from "node:path";
import * as fs from "node:fs";
import * as cp from "node:child_process";
import * as crypto from "node:crypto";

import * as vscode from "vscode";

export type ResolvedServer = {
  path: string;
  source: "setting" | "path" | "cached" | "downloaded";
};

/** The compiler/server version this extension build targets, e.g. `v0.23.0`.
 *  Declared in package.json as `bynkServerVersion` so it is visible to users
 *  and bumped alongside a server release. */
export function serverVersion(context: vscode.ExtensionContext): string {
  const v = context.extension.packageJSON.bynkServerVersion as
    | string
    | undefined;
  if (!v) {
    throw new Error(
      "vscode-bynk is misconfigured: package.json is missing `bynkServerVersion`.",
    );
  }
  return v;
}

/** `<owner>/<repo>` parsed from package.json `repository.url`.
 *
 * Parsed with linear string ops rather than a single regex: a
 * `github\.com[/:]([^/]+\/…)` match backtracks polynomially on adversarial
 * input (CodeQL js/polynomial-redos). indexOf + split has no such blowup. */
function repoSlug(context: vscode.ExtensionContext): string {
  const fallback = "bynk-lang/bynk";
  const url: string = context.extension.packageJSON.repository?.url ?? "";
  const host = url.indexOf("github.com");
  if (host === -1) return fallback;
  const after = url.slice(host + "github.com".length).replace(/^[/:]/, "");
  const [owner, repoRaw] = after.split("/");
  const repo = repoRaw?.replace(/\.git$/, "");
  return owner && repo ? `${owner}/${repo}` : fallback;
}

/** Map the host to the Rust target triple used by the release assets, or
 *  undefined when no prebuilt server is published for this platform. */
export function targetTriple(
  platform: NodeJS.Platform = process.platform,
  arch: string = process.arch,
): string | undefined {
  switch (`${platform}-${arch}`) {
    case "win32-x64":
      return "x86_64-pc-windows-msvc";
    case "darwin-arm64":
      return "aarch64-apple-darwin";
    case "darwin-x64":
      return "x86_64-apple-darwin";
    case "linux-x64":
      return "x86_64-unknown-linux-gnu";
    case "linux-arm64":
      return "aarch64-unknown-linux-gnu";
    default:
      return undefined;
  }
}

/** The version a resolved server is judged against (#1673). The pin
 *  (`bynkServerVersion`) names the last *shipped* release, which is what a
 *  download fetches, so a downloaded or cached server is held to it. A server
 *  the user supplies (`PATH`, or the `bynk.executablePath` setting) is held to
 *  this extension build's own version instead. That is the compiler the build
 *  came from, so a contributor running a `bynkc-lsp` built from the same tree
 *  matches by construction, though it is newer than the pin. */
export function expectedServerVersion(
  source: ResolvedServer["source"],
  pin: string,
  extensionVersion: string,
): string {
  const v = source === "downloaded" || source === "cached" ? pin : extensionVersion;
  return v.replace(/^v/, "");
}

/** A binary the extension can provision from a release. */
export type Tool = "bynkc-lsp" | "bynk" | "bynkc";

/** The toolchain the extension manages beside the server: the driver and the
 *  compiler it runs. */
export const TOOLCHAIN: readonly Tool[] = ["bynk", "bynkc"];

/** The release asset basename for the raw `tool` binary on `target`. */
export function toolAssetName(tool: Tool, target: string): string {
  const exe = target.includes("windows") ? ".exe" : "";
  return `${tool}-${target}${exe}`;
}

/** The release asset basename for the raw server binary on `target`. */
export function serverAssetName(target: string): string {
  return toolAssetName("bynkc-lsp", target);
}

export function cachedToolPath(
  context: vscode.ExtensionContext,
  version: string,
  target: string,
  tool: Tool,
): string {
  const exe = target.includes("windows") ? ".exe" : "";
  return path.join(
    context.globalStorageUri.fsPath,
    "server",
    version,
    `${tool}${exe}`,
  );
}

function cachedServerPath(
  context: vscode.ExtensionContext,
  version: string,
  target: string,
): string {
  return cachedToolPath(context, version, target, "bynkc-lsp");
}

function findOnPath(bin: string): string | undefined {
  const PATH = process.env.PATH ?? "";
  const sep = process.platform === "win32" ? ";" : ":";
  for (const dir of PATH.split(sep)) {
    if (!dir) continue;
    const candidate = path.join(dir, bin);
    if (fs.existsSync(candidate)) return candidate;
    if (process.platform === "win32" && fs.existsSync(candidate + ".exe")) {
      return candidate + ".exe";
    }
  }
  return undefined;
}

/** Resolve a server without downloading. Returns undefined if only a download
 *  would satisfy it (the caller decides whether to fetch). */
export function resolveExistingServer(
  context: vscode.ExtensionContext,
): ResolvedServer | undefined {
  const configured = vscode.workspace
    .getConfiguration("bynk")
    .get<string>("executablePath", "")
    .trim();
  if (configured) {
    // An explicit setting is honoured as-is: absolute path, or a name on PATH.
    if (path.isAbsolute(configured) && fs.existsSync(configured)) {
      return { path: configured, source: "setting" };
    }
    const onPath = findOnPath(configured);
    if (onPath) return { path: onPath, source: "setting" };
    // Configured but missing — surface that rather than silently downloading.
    return undefined;
  }

  const onPath = findOnPath("bynkc-lsp");
  if (onPath) return { path: onPath, source: "path" };

  const target = targetTriple();
  if (target) {
    const cached = cachedServerPath(context, serverVersion(context), target);
    if (fs.existsSync(cached)) return { path: cached, source: "cached" };
  }
  return undefined;
}

/** Parse a SHA256SUMS file (`<hex>  <basename>` per line) for `basename`. */
export function expectedSha(sumsText: string, basename: string): string | undefined {
  for (const line of sumsText.split(/\r?\n/)) {
    const m = line.trim().match(/^([0-9a-f]{64})\s+\*?(.+)$/i);
    if (m && path.basename(m[2]) === basename) return m[1].toLowerCase();
  }
  return undefined;
}

async function fetchBuffer(url: string): Promise<Buffer> {
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) {
    throw new Error(`GET ${url} → ${res.status} ${res.statusText}`);
  }
  return Buffer.from(await res.arrayBuffer());
}

/** Download, verify and cache `tools` for this platform and the extension's
 *  pinned release, returning each one's path. Throws on any failure
 *  (unsupported platform, network, a release without the asset, checksum
 *  mismatch); nothing is cached for a tool unless all of them verified. */
async function downloadTools(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
  tools: readonly Tool[],
  what: string,
): Promise<Map<Tool, string>> {
  const target = targetTriple();
  if (!target) {
    throw new Error(
      `No prebuilt Bynk binaries for ${process.platform}/${process.arch}. ` +
        "Build them (`cargo build --release -p bynk -p bynkc -p bynk-lsp`) and set " +
        "`bynk.executablePath` / `bynk.bynkPath`.",
    );
  }
  const version = serverVersion(context);
  const slug = repoSlug(context);
  const base = `https://github.com/${slug}/releases/download/${version}`;
  const log = tools.includes("bynkc-lsp") ? "[server]" : "[toolchain]";

  return vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: `Bynk: downloading ${what} ${version} (${target})`,
      cancellable: false,
    },
    async () => {
      output.appendLine(`${log} fetching ${base}/SHA256SUMS`);
      const sums = (await fetchBuffer(`${base}/SHA256SUMS`)).toString("utf8");

      const verified: [Tool, Buffer][] = [];
      for (const tool of tools) {
        const asset = toolAssetName(tool, target);
        const want = expectedSha(sums, asset);
        if (!want) {
          throw new Error(`SHA256SUMS has no entry for ${asset}`);
        }
        output.appendLine(`${log} fetching ${base}/${asset}`);
        const bin = await fetchBuffer(`${base}/${asset}`);
        const got = crypto.createHash("sha256").update(bin).digest("hex");
        if (got !== want) {
          throw new Error(
            `checksum mismatch for ${asset}: expected ${want}, got ${got}`,
          );
        }
        verified.push([tool, bin]);
      }

      const paths = new Map<Tool, string>();
      for (const [tool, bin] of verified) {
        const dest = cachedToolPath(context, version, target, tool);
        await fs.promises.mkdir(path.dirname(dest), { recursive: true });
        // Write-then-rename so a crash mid-write can't poison the cache with a
        // truncated binary (the resolvers trust an existing file).
        const tmp = `${dest}.tmp-${process.pid}-${crypto.randomUUID()}`;
        await fs.promises.writeFile(tmp, bin, { mode: 0o755 });
        await fs.promises.rename(tmp, dest);
        output.appendLine(`${log} cached at ${dest}`);
        paths.set(tool, dest);
      }
      return paths;
    },
  );
}

/** Download, verify, cache, and return the path to the server binary for this
 *  platform and the extension's pinned server version. Throws on any failure
 *  (unsupported platform, network, checksum mismatch). */
export async function downloadServer(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
): Promise<string> {
  const paths = await downloadTools(context, output, ["bynkc-lsp"], "language server");
  return paths.get("bynkc-lsp")!;
}

/** Download, verify and cache the `bynk` driver and `bynkc` compiler beside the
 *  server, returning the `bynk` path. Throws like {@link downloadServer}; a
 *  release cut before these were published has no such asset and fails with the
 *  missing-entry error. */
export async function downloadToolchain(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
): Promise<string> {
  const paths = await downloadTools(context, output, TOOLCHAIN, "Bynk toolchain");
  return paths.get("bynk")!;
}

/** Where the extension caches a downloaded toolchain, when both halves of it
 *  are present: the `bynk` driver, and the `bynkc` it finds as its sibling. */
export function cachedToolchain(
  context: vscode.ExtensionContext,
): { dir: string; bynk: string } | undefined {
  const target = targetTriple();
  if (!target) return undefined;
  const version = serverVersion(context);
  const [bynk, bynkc] = TOOLCHAIN.map((t) =>
    cachedToolPath(context, version, target, t),
  );
  if (!fs.existsSync(bynk) || !fs.existsSync(bynkc)) return undefined;
  return { dir: path.dirname(bynk), bynk };
}

/** Whether `bynk` is reachable already, without the managed copy: the
 *  `bynk.bynkPath` setting (as given), or `bynk` on PATH. */
export function userSuppliedBynk(): boolean {
  const configured = vscode.workspace
    .getConfiguration("bynk")
    .get<string>("bynkPath", "")
    .trim();
  return configured !== "" || findOnPath("bynk") !== undefined;
}

/** The server's `--version` string, or undefined if it can't be run. */
export function readServerVersion(serverPath: string): string | undefined {
  try {
    return cp
      .spawnSync(serverPath, ["--version"], { timeout: 2000 })
      .stdout?.toString("utf8")
      .trim();
  } catch {
    return undefined;
  }
}

/** Pull a bare `MAJOR.MINOR.PATCH` out of a version string, tolerating a
 *  leading name and/or `v` (`"bynkc-lsp 0.129.0"`, `"v0.132.1"`,
 *  `"0.132.1"`). Undefined if no such pattern is present.
 *
 * Deliberately not an unanchored `/(\d+)\.(\d+)\.(\d+)/` match: `s` is the
 * stdout of an external `bynkc-lsp --version` process (untrusted input), and
 * that shape backtracks polynomially on adversarial input (CodeQL
 * js/polynomial-redos) — the same hazard `repoSlug` above avoids. Splitting
 * on a lone `\d+` has no such ambiguity (a digit run matches only one way),
 * so scanning the alternating non-digit/digit parts it produces stays linear. */
export function parseVersion(
  s: string,
): [number, number, number] | undefined {
  const parts = s.split(/(\d+)/);
  for (let i = 1; i + 4 < parts.length; i += 2) {
    if (parts[i + 1] === "." && parts[i + 3] === ".") {
      return [Number(parts[i]), Number(parts[i + 2]), Number(parts[i + 4])];
    }
  }
  return undefined;
}

/** -1 / 0 / 1 as `a` is older / equal / newer than `b`. */
export function compareVersions(
  a: [number, number, number],
  b: [number, number, number],
): number {
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
  }
  return 0;
}
