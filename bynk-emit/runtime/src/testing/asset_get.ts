import { HttpResult } from "../httpresult.ts";
import { __bynkAssetFiles } from "./harness.ts";

export function __bynkAssetGet(url: string): HttpResult<never> {
  const f = globalThis.Object.hasOwn(__bynkAssetFiles, url) ? __bynkAssetFiles[url] : undefined;
  if (f === undefined) return HttpResult.NotFound;
  const bin = atob(f.body);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return HttpResult.Raw(bytes, f.contentType);
}
