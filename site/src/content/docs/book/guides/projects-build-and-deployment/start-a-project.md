---
title: "Start a new project with `bynk new`"
---
**Goal:** go from nothing to a running service in three commands — no manifest to
hand-write, no layout to remember, no blank file to stare at.

```sh
bynk new hello
cd hello
bynk dev          # already serving on http://localhost:8787
```

`bynk new` scaffolds a **complete, runnable** project: a `bynk.toml`, a
`.gitignore`, and a starter HTTP service under `src/`. The scaffold is chosen so
[`bynk dev`](/book/guides/projects-build-and-deployment/run-locally/) serves it unmodified — that end-to-end loop is the
whole point. `bynk new` is the first step of the driver arc *is my machine ready*
([`bynk doctor`](/docs/editor-and-tooling/doctor/)) → *make me something*
(`bynk new`) → *run it* ([`bynk dev`](/book/guides/projects-build-and-deployment/run-locally/)).

> **`bynk new` needs no toolchain.** Unlike `dev`, it only writes files — it
> shells nothing, compiles nothing, and reads no network. It works *before*
> Node or `wrangler` are installed, which is exactly why it can be your
> very first command. (Run [`bynk doctor`](/docs/editor-and-tooling/doctor/) once
> you want to build or serve.)

## What it writes

```text
hello/
├── bynk.toml            # [project] name/version + optional [paths] include/exclude
├── .gitignore           # /.bynk — the build dir `bynk dev` writes
└── src/
    └── hello.bynk       # context hello — a GET "/" HTTP service
```

The starter is a minimal but real service — the single-file analogue of the
[`hello-world` example](https://github.com/bynk-lang/bynk/tree/main/examples/hello-world):

```bynk
context hello

consumes bynk { Logger }

service api from http {
	on GET("/") () -> Effect[HttpResult[String]] by v: Visitor given Logger {
		let _ <- Logger.info("hello from hello")
		Ok("Hello from hello!")
	}
}
```

That is everything you need to compile and serve. From here, edit `src/hello.bynk`
and let `bynk dev` reload it.

## Naming the project

By default the project takes its name from the target directory's final
component, and that name is used for **both** the `[project] name` and the
starter's **context** — so the name must be a legal Bynk identifier: a letter
followed by letters, digits, or underscores (no dashes or dots).

A directory whose name isn't a legal identifier (`my-app`, `2048`) is refused
with a fix-it rather than silently mangled — pass `--name` to choose the
identifier:

```sh
bynk new my-app --name myapp     # directory my-app/, project + context `myapp`
```

## Choosing the location

`PATH` is the directory to create:

```sh
bynk new hello          # create ./hello
bynk new ./apps/hello   # create ./apps/hello (parent dirs are created)
```

`bynk new` **never overwrites**. If the target already exists and isn't empty, it
fails before writing anything and touches nothing. An empty directory is fine
(the common "I just `mkdir`ed it" case) — and VCS/OS cruft like `.git` or
`.DS_Store` doesn't count as non-empty, so a freshly `git init`ed directory still
works.

`bynk new` doesn't run `git init` or write anything outside the project — the
scaffold drops cleanly into an existing repository, and `git init` is one command
away if you want a new one.

## Next steps

- [Run your project locally](/book/guides/projects-build-and-deployment/run-locally/) — `bynk dev`, which serves what
  `new` wrote.
- [Lay out a project](/book/guides/projects-build-and-deployment/layout/) — how the source tree maps to contexts and
  commons as the project grows beyond one file.
- Reference: [the `bynk` driver CLI](/docs/bynk-cli/).
