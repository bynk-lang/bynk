// `crypto-browser.binding`: the `bynk.crypto` binding for the browser platform
// (standard-library K4). The shared suite is `crypto.mjs`.

import * as binding from "./bynk/crypto-browser.binding.js";
import { cryptoSuite } from "./crypto.mjs";

cryptoSuite("crypto-browser.binding", binding, "browser");
