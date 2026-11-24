let __bynkClockNow = 0;
function __bynkAdvanceClock(by: number): void {
  if (!(by >= 0)) throw new globalThis.Error(`\`advance Clock by\` takes a non-negative Duration, but got ${by} ms`);
  __bynkClockNow += by;
}
const __bynkClock = { async now(): Promise<number> { return __bynkClockNow; } };
