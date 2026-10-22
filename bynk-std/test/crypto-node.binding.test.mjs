// `crypto-node.binding`: the `bynk.crypto` binding for the node platform
// (standard-library K4). The shared suite is `crypto.mjs`.

import * as binding from "./bynk/crypto-node.binding.js";
import { cryptoSuite } from "./crypto.mjs";

cryptoSuite("crypto-node.binding", binding, "node");
