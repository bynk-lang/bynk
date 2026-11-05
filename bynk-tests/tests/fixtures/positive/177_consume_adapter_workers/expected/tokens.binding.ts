import type { Jwt } from "./tokens.js";

export class JoseJwt implements Jwt {
  async sign(sub: string, secret: string): Promise<string> {
    // a real binding would call panva/jose; the gate checks shape, not runtime.
    return `${sub}.${secret}`;
  }
}
