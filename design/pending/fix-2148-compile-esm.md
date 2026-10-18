---
level: patch
changelog: '`bynkc compile` emits a `tsconfig.json` that builds the output as ES modules (`module: ES2022`, `moduleResolution: Bundler`, `moduleDetection: force`) instead of `module: NodeNext`, so `tsc -p out/tsconfig.json` with emit on no longer down-levels it to CommonJS when no `package.json` declares `"type": "module"`, and a declaration named `require` or (under TypeScript 7) `Object` no longer fails with TS2441 (#2148)'
---
