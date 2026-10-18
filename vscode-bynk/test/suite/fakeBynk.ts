// Helpers for tests that drive the extension's CLI commands against the
// stand-in `bynk` in test/fixtures/fake-bynk (#2094). The extension host and the
// tests share one process, so setting `BYNK_FAKE_LOG` here reaches every `bynk`
// child the extension spawns.

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

export const FAKE_BYNK = path.resolve(__dirname, "../../../test/fixtures/fake-bynk/bynk");

export interface FakeLogEntry {
  argv?: string[];
  cwd?: string;
  signal?: string;
}

/** Point `bynk.bynkPath` at the stand-in and log its invocations; returns an
 *  undo function and a reader for the log. */
export async function useFakeBynk(): Promise<{
  read: () => FakeLogEntry[];
  restore: () => Promise<void>;
}> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-fake-"));
  const log = path.join(dir, "log.jsonl");
  const config = vscode.workspace.getConfiguration("bynk");
  const previous = config.inspect<string>("bynkPath")?.globalValue;
  fs.chmodSync(FAKE_BYNK, 0o755);
  await config.update("bynkPath", FAKE_BYNK, vscode.ConfigurationTarget.Global);
  process.env.BYNK_FAKE_LOG = log;
  return {
    read: () =>
      fs.existsSync(log)
        ? fs
            .readFileSync(log, "utf8")
            .split("\n")
            .filter(Boolean)
            .map((l) => JSON.parse(l) as FakeLogEntry)
        : [],
    restore: async () => {
      delete process.env.BYNK_FAKE_LOG;
      await vscode.workspace
        .getConfiguration("bynk")
        .update("bynkPath", previous, vscode.ConfigurationTarget.Global);
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

/** Poll `cond` until it holds, or fail after `timeoutMs`. */
export async function waitFor(cond: () => boolean, what: string, timeoutMs = 15_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}
