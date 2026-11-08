/**
 * @name Server-side request forgery (Bynk emitted Workers)
 * @description `js/request-forgery` with Bynk's Workers runtime models in
 *              scope: a request URL built from data a Worker received.
 * @kind path-problem
 * @problem.severity error
 * @security-severity 9.1
 * @precision high
 * @id bynk/emitted-request-forgery
 * @tags security
 *       external/cwe/cwe-918
 */

import javascript
import BynkEmitted
import semmle.javascript.security.dataflow.RequestForgeryQuery
import RequestForgeryFlow::PathGraph

from RequestForgeryFlow::PathNode source, RequestForgeryFlow::PathNode sink, DataFlow::Node request
where
  RequestForgeryFlow::flowPath(source, sink) and
  request = sink.getNode().(Sink).getARequest()
select request, source, sink, "The $@ of this request depends on a $@.", sink.getNode(),
  sink.getNode().(Sink).getKind(), source, "user-provided value"
