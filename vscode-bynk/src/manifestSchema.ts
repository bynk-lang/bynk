// #2096: editing support for `bynk.toml` comes from a JSON Schema, generated
// from the toolchain's manifest types (`cargo xtask manifest-schema`) and served
// by the docs site. `package.json` associates it with `bynk.toml` through
// `contributes.tomlValidation`, which a TOML extension built on taplo (Even
// Better TOML) honours: completion, hover and validation of every table and key.
//
// The extension recommends one rather than depending on it. This module is the
// dependency-free half (no `vscode` import), so the unit tests can compile it.

/** Where the docs site serves the schema; `package.json`'s `tomlValidation`
 *  names the same URL (pinned by `manifest-schema.unit.test.ts` and by
 *  `xtask/tests/manifest_schema.rs`). */
export const MANIFEST_SCHEMA_URL = "https://bynk-lang.org/schemas/bynk.toml.json";

/** The TOML extension the recommendation points at: it reads other
 *  extensions' `tomlValidation` contributions. */
export const TOML_EXTENSION_ID = "tamasfe.even-better-toml";

/** The `globalState` key that remembers "Don't show again". */
export const TOML_RECOMMENDATION_DISMISSED = "bynk.tomlRecommendationDismissed";

/** Whether `fileName` is a project manifest (`.../bynk.toml`, either separator). */
export function isManifestPath(fileName: string): boolean {
  return /(^|[\\/])bynk\.toml$/.test(fileName);
}

/** Whether to suggest installing a schema-aware TOML extension, on opening
 *  `fileName`: only for a manifest, only when none is installed, and never
 *  again once dismissed or already shown this session. */
export function shouldRecommendTomlExtension(opts: {
  fileName: string;
  tomlExtensionInstalled: boolean;
  dismissed: boolean;
  shownThisSession: boolean;
}): boolean {
  return (
    isManifestPath(opts.fileName) &&
    !opts.tomlExtensionInstalled &&
    !opts.dismissed &&
    !opts.shownThisSession
  );
}
