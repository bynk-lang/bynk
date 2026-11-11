export class JoseJwt {
  async sign(secret: string): Promise<string> {
    return secret;
  }
}
