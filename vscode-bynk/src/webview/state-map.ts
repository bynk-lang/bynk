// #2196: map a rendered Mermaid `stateDiagram-v2` back to the model, for
// click-to-code (see `state-gen.ts` for why it is not Mermaid's `click`).
// Structural over DOM-like elements so it is unit-testable without a browser.

export interface ElementLike {
  id: string;
  getAttribute(name: string): string | null;
}

/** The node element of state id `id` (`s3`, `unknown`): Mermaid names it
 *  `state-<id>-<n>`, possibly prefixed by the SVG's own id. */
export function stateNode<E extends ElementLike>(nodes: Iterable<E>, id: string): E | undefined {
  const marker = `state-${id}-`;
  for (const n of nodes) {
    const at = n.id.indexOf(marker);
    // The marker must be followed only by the counter, so `state-s1-` does
    // not also match `state-s10-…`.
    if (at >= 0 && /^\d+$/.test(n.id.slice(at + marker.length))) return n;
  }
  return undefined;
}

/** Transition label elements in statement order: each carries
 *  `data-id="edge<n>"`, `n` increasing in the order the diagram text named the
 *  transitions. Labels of any other kind (note edges) are dropped. */
export function transitionLabels<E extends ElementLike>(labels: Iterable<E>): E[] {
  const numbered: Array<[number, E]> = [];
  for (const l of labels) {
    const m = /^edge(\d+)$/.exec(l.getAttribute("data-id") ?? "");
    if (m) numbered.push([Number(m[1]), l]);
  }
  numbered.sort((a, b) => a[0] - b[0]);
  return numbered.map(([, l]) => l);
}

/** Transition labels lined up with the generator's groups. In Mermaid 12.2 the
 *  label-less `[*] --> initial` gets no label element, so the numbered labels
 *  are exactly the groups'; should a later Mermaid emit an (empty) label for
 *  it too, there is one label more than there are groups, and the first —
 *  the initial transition, numbered first — is dropped. */
export function alignLabels<E>(labels: E[], groups: number, hasInitial: boolean): E[] {
  return hasInitial && labels.length === groups + 1 ? labels.slice(1) : labels;
}
