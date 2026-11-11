import { Ok, type Result } from "./runtime.js";
import type { Mailer, MailError } from "./mail.js";

export class RealMailer implements Mailer {
  async send(to: string): Promise<Result<string, MailError>> {
    return Ok(to);
  }
}
