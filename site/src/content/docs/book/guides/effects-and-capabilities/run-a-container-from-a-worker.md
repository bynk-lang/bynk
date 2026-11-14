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

`bynk deploy` and `bynk dev` both handle containers.

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

## Deploy it

`bynk deploy` plans the container, checks for Docker when it has an image to
build, and hands the Worker to `wrangler deploy`, which builds and pushes the image
and creates the application. Docker must be running, and wrangler must be 4.136.0
or later (an older one is refused with the version named). Without Docker, push a
prebuilt image to Cloudflare's registry and adopt it in `bynk.toml`:

```toml
[deploy.containers.Renderer]
image = { production = "registry.cloudflare.com/<ACCOUNT_ID>/renderer@sha256:<DIGEST>" }
```

The image must be named by digest and be in Cloudflare's registry; Docker Hub,
Amazon ECR and Google's registry are not supported for this kind of container.
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#containers)
covers the plan, the Docker check, adoption, CI and `bynk revert`. The
`[[containers]]` entry names its build paths from the Worker's output directory
back to the project, computed from the directory the build is written to, so
`bynk build --out <dir>` works for any directory inside the project; a build
written outside it is refused (`bynk.container.output_outside_project`). A running
container costs money: see Cloudflare's
[Containers pricing](https://developers.cloudflare.com/containers/pricing/) page,
which this guide does not repeat.

## Run it locally

`bynk dev` runs the container under Docker on your machine, through `wrangler dev`:
there is no remote call, and none of the cost warnings the remote-only kinds print.
Wrangler must be 4.136.0 or later for a Worker with a container (an older one is
refused with the version named). With Docker running, the first start builds each
image, which can take minutes, and `bynk dev` says which container is building;
Wrangler decides when a changed Dockerfile is built again. `bynk dev` looks for
Docker and for containers once, when it starts: restart it after adding the first
`Container` to a Worker that is already being served.

Without Docker, `bynk dev` warns once, naming the containers, and starts those
Workers with containers off. The Worker still serves, and a call to a container
**faults**, with a message that names Docker, rather than failing as an obscure
start error. Start Docker and restart `bynk dev` to run it. `bynk dev --remote`
is refused for a Worker with a container, because remote bindings are not
documented for containers. A container's request limit under `bynk dev` is
whatever your Docker allows: there is no instance ceiling locally, as there is
none on the platform.

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
