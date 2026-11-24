---
title: Unreleased
description: The increments merged since the latest Bynk release.
---
The increments merged since **v0.661.1**, the latest release. They ship in
the next one. Each increment's row is added here when it merges.

| Version | Highlights |
|---|---|
| **v0.662.1** | Every `401` Bynk sends carries a `WWW-Authenticate` challenge derived from the route's actor (`Bearer`, `Bynk-Signature`, `Bynk-Cookie` or `Bynk`), and a CORS service exposes it |
| **v0.662.0** | A failed `expect … is …`, and an `expect false` that is a `match` arm, report the value they tested (`r was {"tag":"Err",…}`), and `bynk test --format json` carries it as `actual` |
