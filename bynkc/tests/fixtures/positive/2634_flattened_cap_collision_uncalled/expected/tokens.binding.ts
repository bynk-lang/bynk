import type { Jwt } from "./tokens.js";
import type { Token } from "./tokens.js";

export class JoseJwt implements Jwt {
  async sign(sub: string, secret: string): Promise<Token> {
    return { raw: `${sub}.${secret}` };
  }
  async ping(): Promise<number> {
    return 1;
  }
}
