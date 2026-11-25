---
level: patch
changelog: The tree-sitter grammar parses a generic applied through a `uses` qualifier (`lib.y.Box[Int]`, `Y.Box[Int]`) and a record built or spread through one (`lib.y.Point { x: 1 }`, `Y.Point { ...p }`), which the compiler's parser already accepted, so an editor no longer marks them as errors (#4403)
---
