// `crypto-cloudflare.binding`: the `bynk.crypto` binding for the cloudflare platform
// (standard-library K4). The shared suite is `crypto.mjs`.

import * as binding from "./bynk/crypto-cloudflare.binding.js";
import { cryptoSuite } from "./crypto.mjs";

cryptoSuite("crypto-cloudflare.binding", binding, "cloudflare");
