// #2469: the pure core of "Show capability path" (see `capabilityPath.ts`):
// validating the JSON arguments the server's code action sends. No `vscode`
// import, so it unit-tests under plain mocha.

export interface WirePosition {
  line: number;
  character: number;
}

/** One step's site, as the server serialises an LSP `Location`. */
export interface WireLocation {
  uri: string;
  range: { start: WirePosition; end: WirePosition };
}

export interface ShowArguments {
  uri: string;
  position: WirePosition;
  locations: WireLocation[];
}

function isPosition(p: unknown): p is WirePosition {
  return (
    typeof p === "object" &&
    p !== null &&
    typeof (p as { line?: unknown }).line === "number" &&
    typeof (p as { character?: unknown }).character === "number"
  );
}

function isWireLocation(l: unknown): l is WireLocation {
  if (typeof l !== "object" || l === null) {
    return false;
  }
  const { uri, range } = l as { uri?: unknown; range?: { start?: unknown; end?: unknown } };
  return typeof uri === "string" && !!range && isPosition(range.start) && isPosition(range.end);
}

/** The command's arguments if they have the shape the server sends
 *  (`[uri, position, locations]`), else `undefined`. */
export function parseArguments(
  uri: unknown,
  position: unknown,
  locations: unknown,
): ShowArguments | undefined {
  if (typeof uri !== "string" || !isPosition(position) || !Array.isArray(locations)) {
    return undefined;
  }
  if (!locations.every(isWireLocation)) {
    return undefined;
  }
  return { uri, position, locations };
}
