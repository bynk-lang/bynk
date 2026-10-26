// v0.9: HttpResult — the built-in HTTP-result sum.

export type HttpResult<T> =
  // 2xx success — carries the serialised value.
  | { readonly tag: "Ok"; readonly value: T }
  // 2xx streamed body — SSE-framed (real-time track slice 1).
  | { readonly tag: "Streaming"; readonly stream: AsyncIterable<string> }
  // 2xx raw body — author-owned bytes with an explicit content-type (v0.111).
  | { readonly tag: "Raw"; readonly body: Uint8Array; readonly contentType: string }
  // 2xx raw body with a handler-supplied entity tag, sent as its `ETag` (caching raw responses).
  | {
      readonly tag: "RawTagged";
      readonly body: Uint8Array;
      readonly contentType: string;
      readonly etag: string;
    }
  // 201 raw body — author-owned bytes with an explicit content-type (#2403, L6).
  | { readonly tag: "RawCreated"; readonly body: Uint8Array; readonly contentType: string }
  | { readonly tag: "Created"; readonly value: T }
  | { readonly tag: "Accepted"; readonly value: T }
  | { readonly tag: "NoContent" }
  // 3xx redirection — carries the target URL, emitted as a Location header.
  | { readonly tag: "MovedPermanently"; readonly location: string }
  | { readonly tag: "Found"; readonly location: string }
  | { readonly tag: "SeeOther"; readonly location: string }
  | { readonly tag: "TemporaryRedirect"; readonly location: string }
  | { readonly tag: "PermanentRedirect"; readonly location: string }
  // 4xx client error.
  | { readonly tag: "BadRequest"; readonly message: string }
  | { readonly tag: "Unauthorized" }
  | { readonly tag: "Forbidden" }
  | { readonly tag: "NotFound" }
  | { readonly tag: "MethodNotAllowed" }
  | { readonly tag: "NotAcceptable" }
  | { readonly tag: "RequestTimeout" }
  | { readonly tag: "Conflict"; readonly message: string }
  | { readonly tag: "Gone" }
  | { readonly tag: "LengthRequired" }
  | { readonly tag: "PayloadTooLarge"; readonly message: string }
  | { readonly tag: "UnsupportedMediaType"; readonly message: string }
  | { readonly tag: "UnprocessableEntity"; readonly message: string }
  | { readonly tag: "TooManyRequests"; readonly message: string }
  | { readonly tag: "UnavailableForLegalReasons"; readonly message: string }
  // 5xx server error.
  | { readonly tag: "ServerError"; readonly message: string }
  | { readonly tag: "NotImplemented"; readonly message: string }
  | { readonly tag: "BadGateway"; readonly message: string }
  | { readonly tag: "ServiceUnavailable"; readonly message: string }
  | { readonly tag: "GatewayTimeout"; readonly message: string }
  // 4xx/5xx with an author-owned body — an error status the author chooses,
  // under its own content-type, no codec (#2341, L4).
  | {
      readonly tag: "RawError";
      readonly status: number;
      readonly body: Uint8Array;
      readonly contentType: string;
    };

export const HttpResult = {
  // 2xx success.
  Ok: <T>(value: T): HttpResult<T> => ({ tag: "Ok", value }),
  // 2xx streamed body — the argument is a stream of SSE event payloads.
  Streaming: (stream: AsyncIterable<string>): HttpResult<never> => ({ tag: "Streaming", stream }),
  // 2xx raw body — the arguments are the octets and their content-type.
  Raw: (body: Uint8Array, contentType: string): HttpResult<never> => ({ tag: "Raw", body, contentType }),
  // 2xx raw body with its validator — the octets, their content-type, and the
  // entity tag the handler supplies (quoted, optionally `W/`-prefixed).
  RawTagged: (body: Uint8Array, contentType: string, etag: string): HttpResult<never> => ({
    tag: "RawTagged",
    body,
    contentType,
    etag,
  }),
  // 201 raw body — the octets and their content-type, as for `Raw`.
  RawCreated: (body: Uint8Array, contentType: string): HttpResult<never> => ({
    tag: "RawCreated",
    body,
    contentType,
  }),
  Created: <T>(value: T): HttpResult<T> => ({ tag: "Created", value }),
  Accepted: <T>(value: T): HttpResult<T> => ({ tag: "Accepted", value }),
  NoContent: { tag: "NoContent" } as HttpResult<never>,
  // 3xx redirection — the argument is the target URL (Location header).
  MovedPermanently: (location: string): HttpResult<never> => ({ tag: "MovedPermanently", location }),
  Found: (location: string): HttpResult<never> => ({ tag: "Found", location }),
  SeeOther: (location: string): HttpResult<never> => ({ tag: "SeeOther", location }),
  TemporaryRedirect: (location: string): HttpResult<never> => ({ tag: "TemporaryRedirect", location }),
  PermanentRedirect: (location: string): HttpResult<never> => ({ tag: "PermanentRedirect", location }),
  // 4xx client error.
  BadRequest: (message: string): HttpResult<never> => ({ tag: "BadRequest", message }),
  Unauthorized: { tag: "Unauthorized" } as HttpResult<never>,
  Forbidden: { tag: "Forbidden" } as HttpResult<never>,
  NotFound: { tag: "NotFound" } as HttpResult<never>,
  MethodNotAllowed: { tag: "MethodNotAllowed" } as HttpResult<never>,
  NotAcceptable: { tag: "NotAcceptable" } as HttpResult<never>,
  RequestTimeout: { tag: "RequestTimeout" } as HttpResult<never>,
  Conflict: (message: string): HttpResult<never> => ({ tag: "Conflict", message }),
  Gone: { tag: "Gone" } as HttpResult<never>,
  LengthRequired: { tag: "LengthRequired" } as HttpResult<never>,
  PayloadTooLarge: (message: string): HttpResult<never> => ({ tag: "PayloadTooLarge", message }),
  UnsupportedMediaType: (message: string): HttpResult<never> => ({
    tag: "UnsupportedMediaType",
    message,
  }),
  UnprocessableEntity: (message: string): HttpResult<never> => ({
    tag: "UnprocessableEntity",
    message,
  }),
  TooManyRequests: (message: string): HttpResult<never> => ({ tag: "TooManyRequests", message }),
  UnavailableForLegalReasons: (message: string): HttpResult<never> => ({
    tag: "UnavailableForLegalReasons",
    message,
  }),
  // 5xx server error.
  ServerError: (message: string): HttpResult<never> => ({ tag: "ServerError", message }),
  NotImplemented: (message: string): HttpResult<never> => ({ tag: "NotImplemented", message }),
  BadGateway: (message: string): HttpResult<never> => ({ tag: "BadGateway", message }),
  ServiceUnavailable: (message: string): HttpResult<never> => ({ tag: "ServiceUnavailable", message }),
  GatewayTimeout: (message: string): HttpResult<never> => ({ tag: "GatewayTimeout", message }),
  // 4xx/5xx with an author-owned body — the compiler resolves the status name.
  RawError: (status: number, body: Uint8Array, contentType: string): HttpResult<never> => ({
    tag: "RawError",
    status,
    body,
    contentType,
  }),
};
