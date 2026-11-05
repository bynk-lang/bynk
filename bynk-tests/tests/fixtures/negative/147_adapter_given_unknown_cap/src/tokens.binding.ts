import type { Jwt } from "./tokens.js";
export class JwksJwt implements Jwt {
  async verify(token: string): Promise<string> { return token; }
}
