---
level: patch
changelog: The editor knows a project's path dependencies — after `uses` and `consumes`, completion offers a dependency's exported commons and adapters as `<alias>.<unit>` (never their bare names), a dependency adapter's capabilities complete inside `consumes <alias>.<unit> { … }` and in `given`, hover on a `<alias>.<unit>` target shows the unit's module doc and the package it comes from, and go-to-definition opens the dependency's source file
---
