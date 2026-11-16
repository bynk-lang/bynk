// The front end's logic: plain functions over the generated client, so the same
// code runs in a page, a framework's server, or the test that drives it against
// the real Worker. Nothing here knows about a framework.
import type { createClient } from "./api";

type Shop = ReturnType<typeof createClient>["shop"];

export interface ProductRow {
  readonly sku: string;
  readonly label: string;
}

/** The catalogue as rows to render. An optional note is `string | null`. */
export async function loadCatalogue(shop: Shop): Promise<readonly ProductRow[]> {
  const res = await shop.getProducts();
  if (res.kind !== "Ok") return [];
  return res.body.map((p) => ({
    sku: p.sku,
    label: p.note === null ? p.name : `${p.name} (${p.note})`,
  }));
}

/** What the page tells the shopper after an order. Every outcome is handled. */
export async function placeOrder(shop: Shop, sku: string, qty: number): Promise<string> {
  const res = await shop.postOrders({ sku, qty, note: null });
  // Every answer and failure is a `kind`, so this `switch` is exhaustive: when
  // the server starts answering something new, it stops compiling.
  switch (res.kind) {
    case "Created":
      return `Ordered ${res.body.qty} × ${res.body.sku} (${res.body.id})`;
    case "Conflict":
      return `Sorry: ${res.error}`;
    case "StructuralMismatch":
      return "The server refused that order.";
    case "Invalid":
      return "Check the quantity (1 to 99); nothing was sent.";
    case "Transport":
      return "Could not reach the shop. Try again.";
    case "Unexpected":
      return "The shop has changed; reload the page.";
  }
}
