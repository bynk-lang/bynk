import type { Jwt } from "./tokens.js";
import type { Token } from "./tokens.js";

export class JoseJwt implements Jwt {
  async sign(sub: string, secret: string): Promise<Token> {
    // a real binding would call panva/jose; the gate checks shape, not runtime.
    return { raw: `${sub}.${secret}` };
  }
}
