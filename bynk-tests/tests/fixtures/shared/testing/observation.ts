function __bynkRecordDeps(deps: any, spec: Record<string, string[]>, obs: { log: Record<string, { args: any[]; order: number; typeKey?: string }[]>; n: number }): any {
  for (const cap of Object.keys(spec)) {
    if (!deps || !deps[cap]) continue;
    for (const op of spec[cap]) {
      const orig = deps[cap][op];
      if (typeof orig !== "function") continue;
      const key = cap + "." + op;
      obs.log[key] = obs.log[key] ?? [];
      deps[cap][op] = (...args: any[]) => {
        // #3945: a generic operation's call carries its type arguments as a trailing
        // `{ __bynkTypeKey }` marker (test builds only); it is recorded apart from the
        // arguments, which stay the operation's own.
        const last = args[args.length - 1];
        const keyed = last !== null && typeof last === "object" && "__bynkTypeKey" in last;
        obs.log[key].push(keyed ? { args: args.slice(0, -1), order: obs.n++, typeKey: last.__bynkTypeKey } : { args, order: obs.n++ });
        return orig.apply(deps[cap], args);
      };
    }
  }
  return deps;
}
