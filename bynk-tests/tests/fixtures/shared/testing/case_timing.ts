type CaseResult = {
  name: string;
  pass: boolean;
  error?: { message: string; location: string; actual?: string; violation?: { kind: string; agent: string; name: string; location: string }; refusals?: { index: number; call: string; kind: string; agent: string; name: string; location: string }[] };
  durationMs: number;
  retries?: number;
  kind?: string;
  seed?: string;
  counterexample?: string;
};
const __bynkCasePerformance = (globalThis as unknown as { performance?: { now(): number } }).performance;
const __bynkCaseNow = (): number => (__bynkCasePerformance ? __bynkCasePerformance.now() : Date.now());
const __bynkCaseSince = (started: number): number => Math.round((__bynkCaseNow() - started) * 100) / 100;
