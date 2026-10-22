// #855: hand-mirrored TS types for `bynk-lsp`'s `bynk/wireContract` wire
// shape (`bynk-lsp/src/wire_contract_request.rs`). No codegen exists in this
// repo for LSP payload types, so this is maintained by hand alongside the
// Rust side — the same posture `webview/types.ts` (#846), `webview/docTypes.ts`
// (#847) and `webview/architectureTypes.ts` (#851) take.
//
// Naming mirrors the Rust side 1:1 (`Wc*` here, `Wc*` there) rather than
// dropping the prefix the way `architectureTypes.ts` drops `Wire` from
// `WireArch*` — the shapes here (`Type`, `Field`, `Ref`, `Body`) are common
// enough words that an unprefixed `Type`/`Field`/`Ref` would collide badly
// with everything else in a TS module, so the direct mirror is kept.
//
// This is file-scoped + position, like `webview/types.ts` (#846): the model
// resolves to one handler, and most locations in it are relative to the
// request document. The one exception is `WcType.loc` — a boundary type may
// be declared in a *different* file (a `uses` target's), so it carries its
// own `uri` (see the Rust module doc's "Cross-file type locations" section).

export interface Position {
  line: number;
  character: number;
}

export interface Range {
  start: Position;
  end: Position;
}

/** A click-to-code target outside the request document (a boundary type may
 *  live in a different file than the handler being inspected). */
export interface WcLoc {
  uri: string;
  range: Range;
}

export type WcBaseName = "Int" | "String" | "Bool" | "Float" | "Duration" | "Instant" | "Bytes";
export type WcJsonKind = "Number" | "String" | "Boolean" | "Object" | "Array" | "Null";
export type WcBaseGuard = "Integral" | "Finite";
export type WcRevalidation = "ViaConstructor" | "Inline" | "StructuralOnly" | "Base64Decode";
export type WcUncheckedReason =
  | "Effect"
  | "ValidationError"
  | "JsonError"
  | "HttpResult"
  | "QueueResult";
export type WcNoCrossContextReason = "NotACallHandler" | "SingleContext";
export type WcHttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export type WcBoundaryKind =
  | { kind: "Http"; method: WcHttpMethod; path: string }
  | { kind: "Call" }
  | { kind: "Cron"; expr: string }
  | { kind: "Message" }
  | { kind: "Open" }
  | { kind: "Close" }
  | { kind: "Event" };

export type WcExpected =
  | { kind: "Json"; json: WcJsonKind }
  | { kind: "Integer" }
  | { kind: "FiniteNumber" }
  | { kind: "Base64String" }
  | { kind: "SumVariantKind" };

/** A boundary-crossing shape reference — the wire form of one field/element/
 *  envelope-param, as opposed to `WcType`'s full declaration. */
export type WcRef =
  | { kind: "Base"; base: WcBaseName; json: WcJsonKind; guards: WcBaseGuard[]; expected: WcExpected }
  | { kind: "Bytes" }
  | { kind: "Named"; name: string }
  | { kind: "Inst"; key: string }
  | { kind: "Unit" }
  | { kind: "Unchecked"; reason: WcUncheckedReason };

export interface WcField {
  name: string;
  shape: WcRef;
  pathSegment: string;
  /** The default initialiser's source rendering, `null` when the field has
   *  none. */
  default: string | null;
}

export interface WcVariant {
  name: string;
  payload: WcField[];
}

export type WcBody =
  | {
      kind: "Scalar";
      base: WcBaseName;
      json: WcJsonKind;
      opaque: boolean;
      /** Declaration order — never sorted (see `bynk_check::wire`'s module
       *  doc / risk 1). */
      predicates: string[];
      baseGuards: WcBaseGuard[];
      revalidation: WcRevalidation;
    }
  | { kind: "Record"; fields: WcField[] }
  | {
      kind: "Sum";
      wireDiscriminant: string;
      memoryDiscriminant: string;
      variants: WcVariant[];
    };

export type WcProvenance = { kind: "Owned" } | { kind: "Consumed"; ownerUnit: string };

export interface WcType {
  name: string;
  codecSuffix: string;
  provenance: WcProvenance;
  body: WcBody;
  /** `null` when the declaring file could not be resolved through the
   *  index (defensive only — see the Rust module doc). */
  loc: WcLoc | null;
}

export type WcInst =
  | { kind: "ResultInst"; key: string; ok: WcRef; err: WcRef }
  | { kind: "OptionInst"; key: string; inner: WcRef }
  | { kind: "ListInst"; key: string; elem: WcRef }
  | { kind: "MapInst"; key: string; mapKey: WcRef; val: WcRef }
  | { kind: "RecordInst"; key: string; name: string; args: WcRef[] }
  | { kind: "SumInst"; key: string; name: string; args: WcRef[] };

export type WcEnvelope =
  | { kind: "Empty" }
  | { kind: "Bare"; param: string; shape: WcRef }
  | { kind: "Keyed"; params: WcParam[] }
  | { kind: "Raw"; param: string; contentType: string };

export interface WcParam {
  name: string;
  shape: WcRef;
}

export interface WcContractForm {
  normalForm: string;
  hash: string;
}

export type WcResponseOrigin =
  | { kind: "DeclaredSuccess" }
  | { kind: "Constructed"; range: Range }
  | { kind: "BoundaryImplicit"; why: string };

export interface WcHttpResponse {
  status: number;
  variant: string;
  origin: WcResponseOrigin;
}

export interface WcModel {
  unit: string;
  service: string;
  kind: WcBoundaryKind;
  /** The handler's own header/signature span, against the request document
   *  (never cross-file, unlike a boundary type's `loc`). */
  range: Range;
  handlerLine: number;
  envelope: WcEnvelope;
  /** `on call` only; `null` whenever `noCrossContext` is set. */
  contract: WcContractForm | null;
  types: WcType[];
  instantiations: WcInst[];
  recursive: string[];
  /** Empty for a non-HTTP handler. */
  responses: WcHttpResponse[];
  noCrossContext: WcNoCrossContextReason | null;
}

/** The payload the extension host embeds into the webview — the model plus
 *  the document URI its own `range` (and any `Constructed` response range)
 *  is relative to. Every `WcType.loc`, by contrast, already carries its own
 *  `uri` (see the module doc). */
export interface WcPayload {
  model: WcModel;
  uri: string;
}

/** A click-to-code message the webview posts back to the extension host —
 *  kept as its own type here (rather than importing `webviewHost`'s, or
 *  `webview/types.ts`'s `RevealMessage`) so this module has no import
 *  dependency on another view's, the same posture `architectureTypes.ts`
 *  takes for `ArchRevealMessage`. */
export interface WcRevealMessage {
  type: "reveal";
  uri: string;
  range: Range;
}
