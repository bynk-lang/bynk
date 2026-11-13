---
title: Run a container from a Worker
---
**Goal:** run a server from a Docker image next to your Worker, call it from a
handler over HTTP, give it a name per tenant, keep it small and let it sleep, and
test the handler without starting anything.

`Container` is **experimental**: it carries no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.Container"]
```

This release emits the Worker's configuration and the generated class. Deploying
is done by hand with `wrangler deploy`; `bynk deploy` and `bynk dev` do not yet
know about containers.

## Write the image

A container is any server that listens on one port. Put its `Dockerfile` in a
directory of your project, for example `containers/renderer/Dockerfile`. Bynk does
not read, lint or generate the Dockerfile. The image must be built for
`linux/amd64`.

## Declare it

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Container, ContainerError }

capability Renderer = Container @build("containers/renderer") @port(8080) @sleepAfter(5.minutes) @instanceType("standard-1")
```

`@build` is the project-relative directory that holds the `Dockerfile`, and
`@port` the port the server listens on. `bynk check` refuses a `@build` that leaves
the project (`bynk.container.build_path`) or holds no `Dockerfile`
(`bynk.container.build_missing`). `@sleepAfter` stops an idle container after that
long (more than zero, at most 6 hours); without it the platform stops the
container shortly after it goes inactive. `@instanceType` is one of `"lite"` (the
default), `"standard-1"`, `"standard-2"`, `"standard-3"` or `"standard-4"`.

A container has **no internet access**: Bynk always starts it with outbound access
off, and this release has no option to change that. There is also no
`@maxInstances`: the platform sets no per-application ceiling for these
containers, so the only limits are your account's. See Cloudflare's
[limits](https://developers.cloudflare.com/containers/platform/limits/) page.

## Call it

```bynk,fragment
service render from http {
	on GET("/render/:tenant") (tenant: String) -> Effect[HttpResult[String]] by Visitor given Renderer {
		let req = Request {
			method: Get,
			url: "http://renderer/render",
			contentType: None,
			authorization: None,
			headers: Map.empty(),
			body: None,
		}
		let res <- Renderer.send("tenant-\(tenant)", req)
		match res {
			Ok(r) => Ok(r.body),
			Err(Unavailable) => ServiceUnavailable("no capacity"),
			Err(Timeout) => GatewayTimeout("the container was too slow"),
			Err(_) => ServerError("render failed"),
		}
	}
}
```

The first argument names an instance. The same name reaches the same running
container, and a new name starts a new one, so a name per tenant gives each tenant
a container of its own. Choose names with care: every distinct name can start
another container on your account, and names appear in Cloudflare's dashboards and
logs, so never put a credential in one. A name that is empty, over 256 bytes or
holds a control character faults the call. The first call to an instance waits for
the container to start, which takes noticeable time.

`receiveBytes` returns the body as `Bytes`, and `sendBytes` sends a `Bytes` body.
A response with a 5xx status is still `Ok`: it is the container's answer. A
`ContainerError` is the platform's: `StartFailed` (the image would not start, or
its port never listened), `Unavailable` (the account's limits), `Timeout` or
`Network`.

## Pass it a secret

`@forward(["RENDER_API_KEY"])` passes the Worker secret of that name to the
container's process as an environment variable. Its value stays in your Worker's
secrets and is never written to source, configuration or the lock file, but every
process and dependency in the image can read it, so forward only what the image
needs.

## Deploy it by hand

`bynk build --target workers` writes the Worker's `wrangler.toml` with a Durable
Object binding, its `exports` entry and a `[[containers]]` entry whose image is
built from the directory you named. Deploy with `wrangler deploy` from the
Worker's output directory: Docker must be running, and Wrangler must be 4.136.0 or
later. The `[[containers]]` entry names its build paths from the Worker's output
directory back to the project, computed from the directory the build is written
to, so `bynk build --out <dir>` works for any directory inside the project. A
build written outside the project is refused (`bynk.container.output_outside_project`),
naming each container and both paths, because the paths could not be written.

Images are pushed to Cloudflare's registry by `wrangler`; Docker Hub, Amazon ECR
and Google's registry references are not supported for this kind of
container. A running container costs money: see Cloudflare's
[Containers pricing](https://developers.cloudflare.com/containers/pricing/) page,
which this guide does not repeat.

## Test it

A test never starts a container. Every `Container` operation faults, naming the
instance, until it is stubbed:

```bynk,fragment
case "an unavailable container is answered" {
	stub Renderer.send(_, _) returns Err(Unavailable)
	let r <- render.GET("/render/:tenant", "acme") by Visitor
	expect r is ServiceUnavailable("no capacity")
}
```

Your image's behaviour is out of the language's reach: a test checks what the
handler does with each answer, including the errors.

## A slow call inside an agent

An agent handles one call at a time, so a handler that awaits a slow container
holds the agent's queue. Call the container from a service and hand the result to
the agent.
