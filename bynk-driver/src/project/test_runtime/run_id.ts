const __bynkRunId: string = (globalThis as { process?: { env?: globalThis.Record<string, string | undefined> } }).process?.env?.["BYNK_TEST_RUN_ID"] ?? "local";
