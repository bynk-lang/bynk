---
title: Call an mTLS origin
---
**Goal:** call an origin that requires a client certificate, presenting one that
lives in your Cloudflare account, and test the code without reaching the origin.

An origin that requires **mutual TLS** checks the caller during the handshake: the
server proves its identity, and so does the Worker, by presenting a certificate.
The certificate and its private key belong to the account, not to Bynk or to your
Worker: you upload them once, and the Worker is bound to the certificate by name.

`MtlsCertificate` is **experimental**: it carries no compatibility promise yet. Opt
in in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.MtlsCertificate"]
```

## Upload the certificate

Bynk never uploads a certificate, so its private key never passes through it. Upload
it yourself, naming it, with Wrangler (the dashboard and the API work too):

```sh
npx wrangler mtls-certificate upload --cert cert.pem --key key.pem --name acme-client
```

That needs an API token with the "SSL and Certificates: Edit" permission (Wrangler's own
login has it). The name is what your code uses.

## Declare it and call through it

Consume `MtlsCertificate` from `bynk.cloudflare`, and `bynk` for `Request` and
`Response`. Declare a capability per certificate, naming it with `@certificate`; a bare
`given MtlsCertificate` names no certificate and is refused.

```bynk,fragment
context shop.backend

consumes bynk
consumes bynk.cloudflare { MtlsCertificate }

capability Backend = MtlsCertificate @certificate("acme-client")

service api from http {
	on GET("/orders") () -> Effect[HttpResult[String]] by Visitor given Backend {
		let req = Request {
			method: Get,
			url: "https://orders.acme.example/v1/orders",
			contentType: None,
			authorization: None,
			headers: Map.empty(),
			body: None,
		}
		let res <- Backend.send(req)
		match res {
			Ok(r) => Ok("status \(r.status)"),
			Err(_) => ServerError("the origin is unreachable"),
		}
	}
}
```

`send` is `Fetch.send` over the certificate: the same request, the same response, and
`Network` when the call fails, which includes a handshake the origin refuses. Unlike
`Service`, the URL's host **is** the destination, so this is a real outbound call, and the
certificate is presented to whatever origin the URL names. `receiveBytes` and `sendBytes` are
the byte forms, for a binary response or request body.

Cloudflare does not support presenting a certificate to an origin that is a **proxied zone on
Cloudflare**: the call fails with a `520`. Call an origin that is not.

## Deploy

`bynk deploy` **links** the certificate: before anything is pushed it lists the account's mTLS
certificates and finds the name in `@certificate`, writes the certificate's id into the Worker's
configuration and records it in `bynk.deploy.lock`. It needs `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`, the token with "SSL and Certificates: Read". A name the account does not
have fails the deploy, naming it and where to upload one; a name two client certificates share
fails it naming both ids. A certificate that has expired, or expires within 30 days, is a
warning: the deploy itself succeeds, and calls fail at the handshake once it has expired. The
plan shows the link before it is made, and a `--dry-run` does not look the certificate up:

```
will link mtls-certificate acme-client (MTLS_BACKEND; not created; not checked in a dry run)
```

The name is used as written in every environment, so staging presents the same certificate as
production. See [Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#mtls-certificates)
for the whole of it.

## Run it locally

There is no local simulation of the binding, so under `bynk dev` a call runs on Cloudflare,
presenting your account's certificate to the real origin. It uses the certificate id your last
deploy recorded, so run `bynk deploy` for the environment first. `bynk dev` warns once, on the
first call, and you silence it once you have read it:

```toml
[dev]
mtls = "allow"
```

## Test it

A test never presents a certificate to an origin. Every operation **faults** until you stub it,
so a test cannot reach the origin by accident:

```bynk,fragment
suite shop.backend {
	consumes bynk

	case "an unstubbed call faults" {
		expect api.GET("/orders") by Visitor faults
	}

	case "a stubbed response is what the handler sees" {
		stub Backend.send(_) returns Ok(Response { status: 200, headers: Map.empty(), body: "[]" })
		let r <- api.GET("/orders") by Visitor
		expect r is Ok("status 200")
	}

	case "a refused handshake is matched" {
		stub Backend.send(_) returns Err(Network)
		let r <- api.GET("/orders") by Visitor
		expect r is ServerError("the origin is unreachable")
	}
}
```

The reference is [Mutual TLS: `MtlsCertificate`](/book/reference/bynk-capabilities/#mutual-tls-mtlscertificate).
