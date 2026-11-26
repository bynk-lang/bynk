// #4111: the runtime form of a `Secret`. The checker keeps a `Secret` out of every place
// text could leave a program (interpolation, a codec, `==`, a record field, a boundary), so
// the one way its text is read is `reveal()`. This class is the same promise for the
// runtime's own paths: a `Secret` that reaches `String(...)`, `JSON.stringify`, a template
// literal, `console.log` or an error message renders as `<secret>`, never its text.
//
// The text lives in a `#private` field: it is not an own enumerable property, so it is not
// in `Object.keys`, a spread, a structured clone or a `JSON.stringify` of the object.

const REDACTED = "<secret>";

const encoder = new TextEncoder();

export class BynkSecret {
  readonly #value: string;

  constructor(value: string) {
    this.#value = value;
  }

  /** The text. The one deliberate exit, spelled `.reveal()` in source. */
  reveal(): string {
    return this.#value;
  }

  /**
   * Whether `candidate` is this secret, in time that depends on the longer of the two
   * lengths and never on where they first differ. Both are compared as UTF-8 octets, and
   * a length mismatch does not return early: it is folded into the result, so a caller
   * cannot learn the secret's length or prefix from how quickly this answers.
   */
  matches(candidate: string): boolean {
    const a = encoder.encode(this.#value);
    const b = encoder.encode(candidate);
    const longest = Math.max(a.length, b.length);
    let diff = a.length ^ b.length;
    for (let i = 0; i < longest; i++) {
      diff |= (a[i] ?? 0) ^ (b[i] ?? 0);
    }
    return diff === 0;
  }

  /**
   * The `Authorization` header value `Bearer <secret>`, itself a secret. The result type is the
   * caller's: the emitted code brands it as the `Authorization` it is assigned to.
   */
  bearer<A = BynkSecret>(): A {
    return new BynkSecret(`Bearer ${this.#value}`) as unknown as A;
  }

  /**
   * The `Authorization` header value `Basic <base64(user:secret)>` (RFC 7617), with this secret
   * as the password. A `user` holding `:` or a control character is not a Basic user and faults,
   * naming neither the user nor the password.
   */
  basic<A = BynkSecret>(user: string): A {
    // eslint-disable-next-line no-control-regex
    if (user.includes(":") || /[\u0000-\u001f\u007f]/.test(user)) {
      throw new Error("bynk: a Basic `Authorization` user cannot hold `:` or a control character");
    }
    const bytes = encoder.encode(`${user}:${this.#value}`);
    let binary = "";
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    return new BynkSecret(`Basic ${btoa(binary)}`) as unknown as A;
  }

  toString(): string {
    return REDACTED;
  }

  toJSON(): string {
    return REDACTED;
  }

  [Symbol.toPrimitive](): string {
    return REDACTED;
  }

  // Node's `util.inspect` (and so `console.log`) asks for this symbol by name.
  [Symbol.for("nodejs.util.inspect.custom")](): string {
    return REDACTED;
  }
}

/** Wrap platform-read text as a `Secret`. The platform bindings and a test's `Secret.of` use it. */
export function __bynkSecret(value: string): BynkSecret {
  return new BynkSecret(value);
}
