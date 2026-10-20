// #2093: the page half of a live model view (the host half is
// `livePanel.ts`). Shared by every view bundle; runs in the webview's browser
// context, so no `vscode` import.
//
// It reads the payload and view state the host embedded, renders, and then
// keeps rendering: the host posts `{type: "update", payload, view}` into the
// live page (DECISION C) instead of replacing its HTML, so scroll position and
// in-page state (an expanded architecture node, the "Hide undocumented"
// toggle) survive a refresh. It also persists the view's target with
// `setState` — what the host's `WebviewPanelSerializer` re-requests from after
// a window reload — and draws the toolbar: Refresh always (the only refresh
// under `bynk.views.refresh: "manual"`), and a pin toggle on the
// cursor-following views (DECISION B).

export interface VsCodeApi {
  postMessage(message: unknown): void;
  setState(state: unknown): void;
}

export interface ViewState {
  target: { uri: string; pinned?: boolean };
  followsCursor: boolean;
  /** #2196: an owner-scoped view's handler under the cursor. */
  focus?: string;
  /** #2196: what the view follows, for the pin's wording (default "handler"). */
  scope?: string;
}

type HostMessage<P> =
  | { type: "update"; payload: P | null; view: ViewState }
  | { type: "view"; view: ViewState };

/** What a started view can ask of the shared render queue. */
export interface LiveView {
  /** Re-render the current payload through the queue — for a page control
   *  (a selector, a toggle), so it can never interleave with a live update or
   *  another control's render. */
  rerender(): void;
}

/**
 * Start a live view: render the embedded payload now, and again on each
 * update the host posts. `render` must cope with being called repeatedly on
 * the same `root` (and with `null`: nothing to show).
 */
export function startLiveView<P>(
  api: VsCodeApi,
  globalName: string,
  render: (root: HTMLElement, payload: P | null) => void | Promise<void>,
  onView?: (view: ViewState) => void,
): LiveView | undefined {
  const root = document.getElementById("root");
  if (!root) return undefined;
  const w = window as unknown as Record<string, unknown>;
  const initial = (w[globalName] ?? null) as P | null;
  const apply = (view: ViewState | null): void => {
    applyView(api, view);
    if (view && onView) onView(view);
  };
  apply((w.__BYNK_VIEW__ ?? null) as ViewState | null);

  // Renders run one at a time, latest wins: an update arriving mid-render (a
  // Mermaid render is async) is queued, and an older queued one is dropped.
  let running = false;
  let queued: { payload: P | null } | undefined;
  let latest: P | null = initial;
  const run = async (payload: P | null): Promise<void> => {
    latest = payload;
    if (running) {
      queued = { payload };
      return;
    }
    running = true;
    try {
      await render(root, payload);
    } finally {
      running = false;
    }
    if (queued) {
      const next = queued;
      queued = undefined;
      await run(next.payload);
    }
  };

  window.addEventListener("message", (event: MessageEvent<HostMessage<P>>) => {
    const msg = event.data;
    if (msg?.type === "update") {
      apply(msg.view);
      void run(msg.payload);
    } else if (msg?.type === "view") {
      apply(msg.view);
    }
  });

  void run(initial).then(() => api.postMessage({ type: "ready" }));
  return { rerender: () => void run(latest) };
}

/** Persist the target and redraw the toolbar for it. */
function applyView(api: VsCodeApi, view: ViewState | null): void {
  if (!view || !view.target?.uri) return;
  api.setState(view.target);
  const bar = document.getElementById("bynk-toolbar");
  if (!bar) return;
  bar.textContent = "";
  if (view.followsCursor) {
    const pinned = view.target.pinned === true;
    const pin = document.createElement("button");
    pin.id = "bynk-pin";
    pin.setAttribute("aria-pressed", String(pinned));
    pin.textContent = pinned ? "Pinned" : "Following cursor";
    const scope = view.scope ?? "handler";
    pin.title = pinned
      ? `Pinned to this ${scope}. Click to follow the cursor again.`
      : `Follows the ${scope} under the cursor. Click to pin this one.`;
    pin.addEventListener("click", () => api.postMessage({ type: "pin", pinned: !pinned }));
    bar.append(pin);
  }
  const refresh = document.createElement("button");
  refresh.id = "bynk-refresh";
  refresh.textContent = "Refresh";
  refresh.title = "Re-request this view from the language server";
  refresh.addEventListener("click", () => api.postMessage({ type: "refresh" }));
  bar.append(refresh);
}
