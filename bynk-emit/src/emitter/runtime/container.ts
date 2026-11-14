// #3477 (slice C1): the Durable Object class generated for a `Container`
// instance, built on Cloudflare's own container API (`ctx.container`, the
// `durable_object` scheduling policy). There is no npm dependency.
//
// The generated `containers.ts` exports one class per instance:
//
//   export const __Container_Renderer = __containerClass({ image: "Renderer", port: 8080, ... });
//
// A request to the class starts the container when it is not running, waits
// for its port, and proxies the request to it. The platform's own failures
// reach the caller as a response carrying `bynk-container-error`, which
// `WorkersContainer` (the `bynk.cloudflare` provider) turns into a
// `ContainerError`; any other response is the container's own, a 5xx included.
// The header is removed from the container's own responses, so a container
// cannot forge one.

/** The header that marks a response as the class's own failure report. */
export const CONTAINER_ERROR_HEADER = "bynk-container-error";

/**
 * The header that marks a response as a *fault*: a call that cannot work in this
 * environment whatever the data, which the provider throws rather than returning
 * as a `ContainerError` value. `bynk dev` without Docker is the one case.
 */
export const CONTAINER_FAULT_HEADER = "bynk-container-fault";

/**
 * The `bynk dev` variable that says containers are off here. `bynk dev` sets it
 * (to the reason, `docker`) on a Worker it starts without Docker, with
 * `wrangler dev --enable-containers=false`, so `ctx.container` is absent.
 */
export const CONTAINERS_UNAVAILABLE_VAR = "BYNK_CONTAINERS_UNAVAILABLE";

/**
 * What a `Container` declaration fixes. `image` is the key of the image in the
 * Worker's `[containers.images]` map (the capability's name), `port` the one
 * port the server listens on, `sleepAfterSeconds` the inactivity timeout when
 * the declaration gives one, `instanceType` the size passed to `start()` when it
 * gives one (the platform's default is `lite`), and `forward` the names of the
 * Worker secrets passed to the process as environment variables.
 */
export interface ContainerSettings {
  readonly image: string;
  readonly port: number;
  readonly sleepAfterSeconds?: number;
  readonly instanceType?: string;
  readonly forward: readonly string[];
}

/** The part of Cloudflare's `ctx.container` this module uses. */
export interface ContainerHandle {
  readonly running: boolean;
  readonly images?: Readonly<Record<string, string>>;
  start(options: {
    image: string;
    enableInternet: boolean;
    env?: Record<string, string>;
    instance?: string;
  }): void;
  setInactivityTimeout(durationMs: number): Promise<void>;
  getTcpPort(port: number): { fetch(url: string, init?: unknown): Promise<Response> };
}

/** The part of a Durable Object's state this module uses. */
export interface ContainerState {
  readonly container?: ContainerHandle;
  blockConcurrencyWhile?<T>(callback: () => Promise<T>): Promise<T>;
}

type ContainerEnv = Readonly<Record<string, unknown>>;

/**
 * The fault a call gets under `bynk dev` when containers are off: it names the
 * container, Docker, and the way out. The platform could not run it, and no
 * value a handler can answer would be honest, so it faults.
 */
function unavailableFault(image: string): Response {
  return new Response(
    `bynk dev: the container \`${image}\` is not running because Docker is not available, so \`bynk dev\` started with containers off — start Docker and restart \`bynk dev\`, or stub the call in a test`,
    { status: 500, headers: { [CONTAINER_FAULT_HEADER]: "docker", "content-type": "text/plain" } },
  );
}

/** How long a call waits for a started container's port to answer. */
export const CONTAINER_READY_TIMEOUT_MS = 30_000;
/** The pause between attempts while the port is not yet listening. */
export const CONTAINER_READY_RETRY_MS = 250;

/** What the class reports when it cannot serve a request. */
type Failure = { kind: "start-failed" | "unavailable" | "timeout"; message: string };

function failure(f: Failure): Response {
  return new Response(f.message, {
    status: 502,
    headers: { [CONTAINER_ERROR_HEADER]: f.kind, "content-type": "text/plain" },
  });
}

// The platform's refusal to start another instance, by its message: the pages
// document no error code for it. Anything unrecognised is a failed start.
function startFailure(e: unknown): Failure {
  const message = e instanceof Error ? e.message : String(e);
  return /limit|capacity|quota|insufficient|no (?:more )?instances?/i.test(message)
    ? { kind: "unavailable", message }
    : { kind: "start-failed", message };
}

// The Worker secrets named by `@forward`, read at start. A name the Worker does
// not hold is a failed start that names the secret and never a value.
function forwarded(
  names: readonly string[],
  env: ContainerEnv,
): { env: Record<string, string> } | { missing: string } {
  const out: Record<string, string> = {};
  for (const name of names) {
    const value = env[name];
    if (typeof value !== "string") return { missing: name };
    out[name] = value;
  }
  return { env: out };
}

/**
 * Starts the container if it is not running, then applies `@sleepAfter`.
 * Returns what went wrong, or `undefined` when the container is running.
 * `setInactivityTimeout` is applied on every start: a Durable Object that
 * restarts begins without one.
 */
export async function ensureContainerRunning(
  state: ContainerState,
  env: ContainerEnv,
  settings: ContainerSettings,
): Promise<{ failure?: Failure; started: boolean }> {
  const container = state.container;
  if (container === undefined) {
    return {
      failure: { kind: "start-failed", message: "no container is configured for this Durable Object" },
      started: false,
    };
  }
  if (container.running) return { started: false };
  const image = container.images?.[settings.image];
  if (image === undefined) {
    return {
      failure: { kind: "start-failed", message: `the image \`${settings.image}\` is not in the Worker's containers.images` },
      started: false,
    };
  }
  const passed = forwarded(settings.forward, env);
  if ("missing" in passed) {
    return {
      failure: { kind: "start-failed", message: `the forwarded secret \`${passed.missing}\` is not set on the Worker` },
      started: false,
    };
  }
  try {
    container.start({
      image,
      // Egress is always off (#3477 J); opting in is a later slice.
      enableInternet: false,
      ...(settings.forward.length > 0 ? { env: passed.env } : {}),
      ...(settings.instanceType !== undefined ? { instance: settings.instanceType } : {}),
    });
    if (settings.sleepAfterSeconds !== undefined) {
      await container.setInactivityTimeout(settings.sleepAfterSeconds * 1000);
    }
  } catch (e) {
    return { failure: startFailure(e), started: false };
  }
  return { started: true };
}

/**
 * Applies `@sleepAfter` to a container that is already running when the Durable
 * Object is constructed (after a restart or a deploy), which begins without
 * the timeout.
 */
export function resumeContainer(state: ContainerState, settings: ContainerSettings): void {
  const container = state.container;
  if (container?.running !== true || settings.sleepAfterSeconds === undefined) return;
  const apply = (): Promise<void> => container.setInactivityTimeout(settings.sleepAfterSeconds! * 1000);
  void (state.blockConcurrencyWhile ? state.blockConcurrencyWhile(apply) : apply()).catch(() => undefined);
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Proxies `request` to the container's port. A container that has just been
 * started is not yet listening, so a failed attempt is retried until
 * [`CONTAINER_READY_TIMEOUT_MS`]; the body is buffered so each attempt can send
 * it again. A request that failed on a container that was already running is
 * not retried: it may have reached the process.
 */
export async function proxyToContainer(
  container: ContainerHandle,
  settings: ContainerSettings,
  request: Request,
  started: boolean,
): Promise<Response> {
  const body = request.method === "GET" || request.method === "HEAD" ? undefined : await request.arrayBuffer();
  const deadline = Date.now() + CONTAINER_READY_TIMEOUT_MS;
  for (;;) {
    try {
      const res = await container.getTcpPort(settings.port).fetch(request.url, {
        method: request.method,
        headers: request.headers,
        ...(body !== undefined ? { body } : {}),
      });
      const headers = new Headers(res.headers);
      // Both markers are the class's own: a container must not be able to
      // forge a platform fault or a failure value with a header of its choosing.
      headers.delete(CONTAINER_ERROR_HEADER);
      headers.delete(CONTAINER_FAULT_HEADER);
      return new Response(res.body, { status: res.status, statusText: res.statusText, headers });
    } catch (e) {
      if (!started) {
        return failure({ kind: "start-failed", message: e instanceof Error ? e.message : String(e) });
      }
      if (Date.now() + CONTAINER_READY_RETRY_MS >= deadline) {
        return failure({ kind: "timeout", message: `the container did not listen on port ${settings.port} in time` });
      }
      await sleep(CONTAINER_READY_RETRY_MS);
    }
  }
}

/**
 * The Durable Object class of one `Container` instance. A legacy-style class
 * (a constructor taking `(state, env)` and a `fetch`), as the generated agent
 * and fan-out classes are.
 */
export function containerClass(settings: ContainerSettings): new (state: ContainerState, env?: unknown) => {
  fetch(request: Request): Promise<Response>;
} {
  return class {
    private state: ContainerState;
    private env: ContainerEnv;
    constructor(state: ContainerState, env?: unknown) {
      this.state = state;
      this.env = (env ?? {}) as ContainerEnv;
      resumeContainer(state, settings);
    }
    async fetch(request: Request): Promise<Response> {
      if (typeof this.env[CONTAINERS_UNAVAILABLE_VAR] === "string") return unavailableFault(settings.image);
      const up = await ensureContainerRunning(this.state, this.env, settings);
      if (up.failure !== undefined) return failure(up.failure);
      return proxyToContainer(this.state.container as ContainerHandle, settings, request, up.started);
    }
  };
}

// #1653: the `__` aliases emitted code imports.
export { containerClass as __containerClass };
