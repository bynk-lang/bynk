/**
 * Models of the Cloudflare Workers runtime that `bynk compile --target workers`
 * emits (#255). CodeQL's JavaScript/TypeScript library has no model of a
 * module Worker, so without these the stock security queries see no user input
 * in Bynk's output at all, and a scan of it passes no matter what it contains.
 *
 * Why QL rather than models-as-data: a data-extension row names code by npm
 * package (or by a `file:` path that some other file imports). The emitted
 * runtime is reached only through relative imports, and the Worker entry
 * (`workers/<ctx>/index.ts`) is imported by nothing, so neither form can name
 * it. An AST-shape model can.
 *
 * Kept deliberately small. The canaries showed what is NOT needed:
 * - no `Fetch.send` sink: `FetchProvider` ends in the global `fetch(url, …)`,
 *   which `js/request-forgery` already treats as a sink, and default call
 *   resolution follows the `deps.Fetch` wiring in `compose.ts` to it;
 * - no `Result`/`Option` flow summaries: default taint tracking already
 *   follows the emitted `{ tag, value }` codecs and their switch-unwraps.
 * If a canary stops firing, add the missing piece here, not before.
 */

import javascript
private import semmle.javascript.security.dataflow.ServerSideUrlRedirectCustomizations

/**
 * The `request` parameter of a module Worker's entry point,
 * `export default { async fetch(request, env, ctx) { … } }`. Everything the
 * emitted boundary decodes (body, query, path, headers) is read from it.
 */
class WorkerFetchRequest extends RemoteFlowSource, DataFlow::ParameterNode {
  WorkerFetchRequest() {
    exists(ExportDefaultDeclaration d, ObjectExpr o, DataFlow::FunctionNode f |
      o = d.getOperand().(Expr).getUnderlyingValue() and
      f.getFunction() = o.getPropertyByName("fetch").getInit() and
      this = f.getParameter(0)
    )
  }

  override string getSourceType() { result = "Cloudflare Worker request" }
}

/**
 * `js/server-side-unvalidated-url-redirection` takes its sources from its own
 * class (third-party-controllable request inputs), not `RemoteFlowSource`.
 */
private class WorkerRequestAsRedirectSource extends ServerSideUrlRedirect::Source instanceof WorkerFetchRequest
{ }

/**
 * A `Location` header value on a Fetch API `Response`: the `location` property
 * of `new Response(body, { headers: { location: v } })` (how
 * `httpResultToResponse` writes `HttpResult`'s 3xx variants), or
 * `headers.set/append("location", v)`.
 */
class ResponseLocationHeader extends ServerSideUrlRedirect::Sink {
  ResponseLocationHeader() {
    exists(
      DataFlow::NewNode n, DataFlow::ObjectLiteralNode init, DataFlow::ObjectLiteralNode hdrs,
      string name
    |
      n = DataFlow::globalVarRef("Response").getAnInstantiation() and
      init.flowsTo(n.getArgument(1)) and
      hdrs.flowsTo(init.getAPropertyWrite("headers").getRhs()) and
      name.toLowerCase() = "location" and
      this = hdrs.getAPropertyWrite(name).getRhs()
    )
    or
    exists(DataFlow::MethodCallNode c |
      c.getMethodName() = ["set", "append"] and
      c.getArgument(0).getStringValue().toLowerCase() = "location" and
      this = c.getArgument(1)
    )
  }
}
