---
level: patch
changelog: The emitter lowers a built-in method call's kernel dispatch in `lower_kernel_method_call` and its UFCS fallback in `lower_ufcs_method_call`, the first step of splitting `lower_method_call` per receiver kind (#3167)
---
