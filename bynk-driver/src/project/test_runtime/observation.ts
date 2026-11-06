function __bynkRecordDeps(deps: any, spec: Record<string, string[]>, obs: { log: Record<string, { args: any[]; order: number }[]>; n: number }): any {
  for (const cap of Object.keys(spec)) {
    if (!deps || !deps[cap]) continue;
    for (const op of spec[cap]) {
      const orig = deps[cap][op];
      if (typeof orig !== "function") continue;
      const key = cap + "." + op;
      obs.log[key] = obs.log[key] ?? [];
      deps[cap][op] = (...args: any[]) => {
        obs.log[key].push({ args, order: obs.n++ });
        return orig.apply(deps[cap], args);
      };
    }
  }
  return deps;
}
