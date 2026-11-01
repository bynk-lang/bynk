import type { Jwt, Claims } from "./tokens.js";

export class JoseJwt implements Jwt {
  async sign(claims: Claims): Promise<string> {
    return claims.sub;
  }
}
