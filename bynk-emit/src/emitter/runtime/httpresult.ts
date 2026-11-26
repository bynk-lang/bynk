// v0.9: HttpResult — the built-in HTTP-result sum.

// #3918: a server-sent event with more than a `data:` line, as the `bynk` record
// `SseEvent` is at run time (`Duration` erases to milliseconds).
// (`Option` is spelled out so this module reaches no other runtime module.)
type Opt<T> = { readonly tag: "Some"; readonly value: T } | { readonly tag: "None" };
export interface SseEvent {
  readonly data: string;
  readonly event: Opt<string>;
  readonly id: Opt<string>;
  readonly retry: Opt<number>;
}

// #4389: the `bynk` record `ContentRange`, as a `PartialContent` answer carries it.
export interface ContentRange {
  readonly start: number;
  readonly end: number;
  readonly total: Opt<number>;
}

// #4394: `E` is the element a `Streaming` body carries (`HttpResult[T, E]` in Bynk), `string | SseEvent` when a
// route names none, as it always was.
export type HttpResult<T, E = string | SseEvent> =
  // 2xx success — carries the serialised value.
  | { readonly tag: "Ok"; readonly value: T }
  // 2xx streamed body — SSE-framed (real-time track slice 1).
  | {
      readonly tag: "Streaming";
      readonly stream: AsyncIterable<E>;
      // #3918: `.heartbeat(every)`: write a comment line whenever the stream is idle this long.
      readonly heartbeatMs?: number;
    }
  // 2xx raw body — author-owned bytes with an explicit content-type (v0.111).
  | { readonly tag: "Raw"; readonly body: Uint8Array; readonly contentType: string }
  // 2xx raw body with a handler-supplied entity tag, sent as its `ETag` (caching raw responses).
  | {
      readonly tag: "RawTagged";
      readonly body: Uint8Array;
      readonly contentType: string;
      readonly etag: string;
    }
  // 2xx raw body streamed chunk by chunk, with no validator (K-streams, #2623).
  | {
      readonly tag: "RawStreamed";
      readonly body: AsyncIterable<Uint8Array>;
      readonly contentType: string;
    }
  // 201 raw body — author-owned bytes with an explicit content-type (#2403, L6).
  | { readonly tag: "RawCreated"; readonly body: Uint8Array; readonly contentType: string }
  | { readonly tag: "Created"; readonly value: T }
  | { readonly tag: "Accepted"; readonly value: T }
  | { readonly tag: "NoContent" }
  // 206 a raw body streamed chunk by chunk, answering a range request, with its `Content-Range` (#4389).
  | {
      readonly tag: "PartialContent";
      readonly body: AsyncIterable<Uint8Array>;
      readonly contentType: string;
      readonly range: ContentRange;
    }
  // 304 bodyless, a conditional answer the handler worked out itself (#4389).
  | { readonly tag: "NotModified" }
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
  | { readonly tag: "PreconditionFailed"; readonly message: string }
  | { readonly tag: "PayloadTooLarge"; readonly message: string }
  | { readonly tag: "UnsupportedMediaType"; readonly message: string }
  | { readonly tag: "UnprocessableEntity"; readonly message: string }
  | { readonly tag: "PreconditionRequired"; readonly message: string }
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
  Ok: <T>(value: T): HttpResult<T, never> => ({ tag: "Ok", value }),
  // 2xx streamed body — the argument is a stream of SSE event payloads.
  Streaming: <E = string | SseEvent>(stream: AsyncIterable<E>): HttpResult<never, E> => ({ tag: "Streaming", stream }),
  // 2xx raw body — the arguments are the octets and their content-type.
  Raw: (body: Uint8Array, contentType: string): HttpResult<never, never> => ({ tag: "Raw", body, contentType }),
  // 2xx raw body with its validator — the octets, their content-type, and the
  // entity tag the handler supplies (quoted, optionally `W/`-prefixed).
  RawTagged: (body: Uint8Array, contentType: string, etag: string): HttpResult<never, never> => ({
    tag: "RawTagged",
    body,
    contentType,
    etag,
  }),
  // 2xx raw body streamed — the chunks and their content-type.
  RawStreamed: (body: AsyncIterable<Uint8Array>, contentType: string): HttpResult<never, never> => ({
    tag: "RawStreamed",
    body,
    contentType,
  }),
  // 201 raw body — the octets and their content-type, as for `Raw`.
  RawCreated: (body: Uint8Array, contentType: string): HttpResult<never, never> => ({
    tag: "RawCreated",
    body,
    contentType,
  }),
  Created: <T>(value: T): HttpResult<T, never> => ({ tag: "Created", value }),
  Accepted: <T>(value: T): HttpResult<T, never> => ({ tag: "Accepted", value }),
  NoContent: { tag: "NoContent" } as HttpResult<never, never>,
  // 206 — the chunks, their content-type and the range they cover.
  PartialContent: (body: AsyncIterable<Uint8Array>, contentType: string, range: ContentRange): HttpResult<never, never> => ({
    tag: "PartialContent",
    body,
    contentType,
    range,
  }),
  NotModified: { tag: "NotModified" } as HttpResult<never, never>,
  // 3xx redirection — the argument is the target URL (Location header).
  MovedPermanently: (location: string): HttpResult<never, never> => ({ tag: "MovedPermanently", location }),
  Found: (location: string): HttpResult<never, never> => ({ tag: "Found", location }),
  SeeOther: (location: string): HttpResult<never, never> => ({ tag: "SeeOther", location }),
  TemporaryRedirect: (location: string): HttpResult<never, never> => ({ tag: "TemporaryRedirect", location }),
  PermanentRedirect: (location: string): HttpResult<never, never> => ({ tag: "PermanentRedirect", location }),
  // 4xx client error.
  BadRequest: (message: string): HttpResult<never, never> => ({ tag: "BadRequest", message }),
  Unauthorized: { tag: "Unauthorized" } as HttpResult<never, never>,
  Forbidden: { tag: "Forbidden" } as HttpResult<never, never>,
  NotFound: { tag: "NotFound" } as HttpResult<never, never>,
  MethodNotAllowed: { tag: "MethodNotAllowed" } as HttpResult<never, never>,
  NotAcceptable: { tag: "NotAcceptable" } as HttpResult<never, never>,
  RequestTimeout: { tag: "RequestTimeout" } as HttpResult<never, never>,
  Conflict: (message: string): HttpResult<never, never> => ({ tag: "Conflict", message }),
  Gone: { tag: "Gone" } as HttpResult<never, never>,
  LengthRequired: { tag: "LengthRequired" } as HttpResult<never, never>,
  PreconditionFailed: (message: string): HttpResult<never, never> => ({ tag: "PreconditionFailed", message }),
  PayloadTooLarge: (message: string): HttpResult<never, never> => ({ tag: "PayloadTooLarge", message }),
  UnsupportedMediaType: (message: string): HttpResult<never, never> => ({
    tag: "UnsupportedMediaType",
    message,
  }),
  UnprocessableEntity: (message: string): HttpResult<never, never> => ({
    tag: "UnprocessableEntity",
    message,
  }),
  PreconditionRequired: (message: string): HttpResult<never, never> => ({
    tag: "PreconditionRequired",
    message,
  }),
  TooManyRequests: (message: string): HttpResult<never, never> => ({ tag: "TooManyRequests", message }),
  UnavailableForLegalReasons: (message: string): HttpResult<never, never> => ({
    tag: "UnavailableForLegalReasons",
    message,
  }),
  // 5xx server error.
  ServerError: (message: string): HttpResult<never, never> => ({ tag: "ServerError", message }),
  NotImplemented: (message: string): HttpResult<never, never> => ({ tag: "NotImplemented", message }),
  BadGateway: (message: string): HttpResult<never, never> => ({ tag: "BadGateway", message }),
  ServiceUnavailable: (message: string): HttpResult<never, never> => ({ tag: "ServiceUnavailable", message }),
  GatewayTimeout: (message: string): HttpResult<never, never> => ({ tag: "GatewayTimeout", message }),
  // 4xx/5xx with an author-owned body — the compiler resolves the status name.
  RawError: (status: number, body: Uint8Array, contentType: string): HttpResult<never, never> => ({
    tag: "RawError",
    status,
    body,
    contentType,
  }),
};

