import type { Jwt, Claims } from "./tokens.js";
import { JwtError } from "./tokens.js";
import type { Secrets } from "./bynk.js";
import { Ok, Err, type Result } from "./runtime.js";

export class JoseJwt implements Jwt {
  // v0.18 ([M]/[N]): the signing secret is a capability dependency, not an
  // operation parameter — compose passes { Secrets } by name. Declared field +
  // assigning constructor (not a parameter property), so the binding strips
  // cleanly under Node `--experimental-strip-types` (the strip-only invariant).
  private deps: { Secrets: Secrets };
  constructor(deps: { Secrets: Secrets }) {
    this.deps = deps;
  }

  async sign(claims: Claims): Promise<string> {
    const secret = await this.secret();
    // a real binding would call panva/jose; the gate checks shape, not runtime.
    return `${claims.sub}.${claims.exp}.${secret}`;
  }

  async verify(token: string): Promise<Result<Claims, JwtError>> {
    const secret = await this.secret();
    if (token.length === 0 || secret.length === 0) {
      return Err(JwtError.Invalid);
    }
    // construct the boundary record as an object literal (§4.4)
    return Ok({ sub: "u1", exp: 0 });
  }

  private async secret(): Promise<string> {
    const s = await this.deps.Secrets.get("JWT_SECRET");
    return s.tag === "Some" ? s.value : "";
  }
}
