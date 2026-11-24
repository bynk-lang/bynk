// v0.44: QueueResult — the built-in queue verdict sum (non-generic). `Ack`
// confirms the message; `Retry` redelivers it, carrying a reason for the log.
export type QueueResult =
  | { readonly tag: "Ack" }
  | { readonly tag: "Retry"; readonly reason: string }
  // #3878: only a `system` case's address yields it (a body that did not decode); a handler cannot return it.
  | { readonly tag: "Undecodable" };

export const QueueResult = {
  Ack: { tag: "Ack" } as QueueResult,
  Retry: (reason: string): QueueResult => ({ tag: "Retry", reason }),
};

