// `bynk-cloudflare`: the `bynk` surface binding's behavioural tests (standard-library S2,
// #1952). The shared suite is `surface.mjs`; this file says which platform
// differences apply.

import * as binding from "./bynk-cloudflare.js";
import * as runtime from "./__bynk_runtime/result.js";
import * as bynk from "./bynk.js";
import { surfaceSuite } from "./surface.mjs";

surfaceSuite("bynk-cloudflare", binding, runtime, bynk, {
  fetch: "real",
  secrets: "env",
  locale: "negotiated",
  log: "object",
});
