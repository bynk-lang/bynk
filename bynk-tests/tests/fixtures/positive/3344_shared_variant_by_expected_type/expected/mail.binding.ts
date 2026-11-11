import { Ok, type Result } from "./__bynk_runtime/result.js";
import type { Mailer, MailError } from "./mail.js";

export class RealMailer implements Mailer {
  async send(to: string): Promise<Result<string, MailError>> {
    return Ok(to);
  }
}
