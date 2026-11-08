/**
 * @name Server-side URL redirect (Bynk emitted Workers)
 * @description `js/server-side-unvalidated-url-redirection` with Bynk's
 *              Workers runtime models in scope: a `Location` header built
 *              from data a Worker received.
 * @kind path-problem
 * @problem.severity warning
 * @security-severity 6.1
 * @precision high
 * @id bynk/emitted-url-redirection
 * @tags security
 *       external/cwe/cwe-601
 */

import javascript
import BynkEmitted
import semmle.javascript.security.dataflow.ServerSideUrlRedirectQuery
import ServerSideUrlRedirectFlow::PathGraph

from ServerSideUrlRedirectFlow::PathNode source, ServerSideUrlRedirectFlow::PathNode sink
where ServerSideUrlRedirectFlow::flowPath(source, sink)
select sink.getNode(), source, sink, "Untrusted URL redirection depends on a $@.", source.getNode(),
  "user-provided value"
