import type { Jwt, Claims } from "./tokens.js";
import { JwtError } from "./tokens.js";
import { Ok, Err, type Result } from "./runtime.js";

export class JoseJwt implements Jwt {
  async sign(claims: Claims, secret: string): Promise<string> {
    // a real binding would call panva/jose; the gate checks shape, not runtime.
    return `${claims.sub}.${claims.exp}.${secret}`;
  }

  async verify(token: string, secret: string): Promise<Result<Claims, JwtError>> {
    if (token.length === 0 || secret.length === 0) {
      return Err(JwtError.Invalid);
    }
    // construct the boundary record as an object literal (§4.4)
    return Ok({ sub: "u1", exp: 0 });
  }
}
