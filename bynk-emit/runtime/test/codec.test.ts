// #2715: the codec combinators behind `Codable` (constrained-generics slice 3).
import { test } from "node:test";
import assert from "node:assert/strict";
import { Ok, Err, None, Some } from "../src/result.ts";
import type { Result } from "../src/result.ts";
import type { BoundaryError, JsonValue } from "../src/boundary.ts";
import { __BynkKeySet, __BynkKeyMap } from "../src/keys.ts";
import {
  __bynkCodecList,
  __bynkCodecMap,
  __bynkCodecOption,
  __bynkCodecResult,
  __bynkCodecSet,
} from "../src/codec.ts";

type P = { readonly x: number };

const int = {
  encode: (v: number): JsonValue => v,
  decode: (j: JsonValue, path: string): Result<number, BoundaryError> =>
    typeof j === "number"
      ? Ok(j)
      : Err({ kind: "StructuralMismatch", path, expected: "number", actual: typeof j }),
};
const str = {
  encode: (v: string): JsonValue => v,
  decode: (j: JsonValue, path: string): Result<string, BoundaryError> =>
    typeof j === "string"
      ? Ok(j)
      : Err({ kind: "StructuralMismatch", path, expected: "string", actual: typeof j }),
};
const point = {
  encode: (v: P): JsonValue => ({ x: v.x }),
  decode: (j: JsonValue, path: string): Result<P, BoundaryError> => {
    if (typeof j !== "object" || j === null || Array.isArray(j)) {
      return Err({ kind: "StructuralMismatch", path, expected: "object", actual: typeof j });
    }
    const r = int.decode((j as { [k: string]: JsonValue })["x"], `${path}.x`);
    return r.tag === "Err" ? r : Ok({ x: r.value });
  },
};

test("list: encodes elements and reports an element error at its index", () => {
  const c = __bynkCodecList(int);
  assert.deepEqual(c.encode([1, 2]), [1, 2]);
  assert.deepEqual(c.decode([1, 2], "$"), Ok([1, 2]));
  assert.deepEqual(c.decode([1, "a"], "$"), {
    tag: "Err",
    error: { kind: "StructuralMismatch", path: "$[1]", expected: "number", actual: "string" },
  });
  assert.deepEqual(c.decode({}, "$"), {
    tag: "Err",
    error: { kind: "StructuralMismatch", path: "$", expected: "array", actual: "object" },
  });
});

test("option: the tagged forms win, a bare value is Some, null is None", () => {
  const c = __bynkCodecOption(int);
  assert.deepEqual(c.encode(Some(3)), { kind: "Some", value: 3 });
  assert.deepEqual(c.encode(None), { kind: "None" });
  assert.deepEqual(c.decode(null, "$"), Ok(None));
  assert.deepEqual(c.decode({ kind: "None" }, "$"), Ok(None));
  assert.deepEqual(c.decode({ kind: "Some", value: 3 }, "$"), Ok(Some(3)));
  assert.deepEqual(c.decode(3, "$"), Ok(Some(3)));
  const bad = c.decode("x", "$");
  assert.equal(bad.tag, "Err");
  if (bad.tag === "Err" && bad.error.kind === "StructuralMismatch") {
    assert.match(bad.error.expected, /^number \| null \| /);
  }
  const nested = __bynkCodecOption(__bynkCodecOption(int));
  assert.deepEqual(nested.decode({ kind: "Some", value: { kind: "None" } }, "$"), Ok(Some(None)));
  const twice = nested.decode("x", "$");
  if (twice.tag === "Err" && twice.error.kind === "StructuralMismatch") {
    assert.equal(twice.error.expected.split(" | null | ").length, 2, "the suffix is appended once");
  }
});

test("result: Ok and Err arms, and an unknown kind", () => {
  const c = __bynkCodecResult(int, str);
  assert.deepEqual(c.encode(Ok(1)), { kind: "Ok", value: 1 });
  assert.deepEqual(c.encode(Err("e")), { kind: "Err", error: "e" });
  assert.deepEqual(c.decode({ kind: "Ok", value: 1 }, "$"), Ok(Ok(1)));
  assert.deepEqual(c.decode({ kind: "Err", error: "e" }, "$"), Ok(Err("e")));
  assert.deepEqual(c.decode({ kind: "Err", error: 4 }, "$"), {
    tag: "Err",
    error: { kind: "StructuralMismatch", path: "$.error", expected: "string", actual: "number" },
  });
  assert.deepEqual(c.decode({ kind: "Maybe" }, "$"), {
    tag: "Err",
    error: { kind: "StructuralMismatch", path: "$", expected: "Ok | Err", actual: "Maybe" },
  });
});

test("set: a composite element decodes into a keyed set that rejects a repeat", () => {
  const c = __bynkCodecSet(point);
  const ok = c.decode([{ x: 1 }, { x: 2 }], "$");
  assert.equal(ok.tag, "Ok");
  if (ok.tag === "Ok") {
    assert.ok(ok.value instanceof __BynkKeySet);
    assert.ok(ok.value.has({ x: 2 }));
  }
  assert.deepEqual(c.decode([{ x: 1 }, { x: 1 }], "$"), {
    tag: "Err",
    error: { kind: "StructuralMismatch", path: "$[1]", expected: "a distinct set element", actual: "a duplicate" },
  });
  const plain = __bynkCodecSet(int).decode([1, 2, 2], "$");
  assert.equal(plain.tag, "Err");
  const fine = __bynkCodecSet(int).decode([1, 2], "$");
  assert.ok(fine.tag === "Ok" && !(fine.value instanceof __BynkKeySet));
  assert.deepEqual(c.encode(new __BynkKeySet([{ x: 1 }])), [{ x: 1 }]);
});

test("map: entries, a composite key held in a keyed map, and entry errors", () => {
  const c = __bynkCodecMap(point, int);
  const ok = c.decode([[{ x: 1 }, 10]], "$");
  assert.equal(ok.tag, "Ok");
  if (ok.tag === "Ok") {
    assert.ok(ok.value instanceof __BynkKeyMap);
    assert.equal(ok.value.get({ x: 1 }), 10);
  }
  assert.deepEqual(c.encode(new __BynkKeyMap([[{ x: 1 }, 10]])), [[{ x: 1 }, 10]]);
  assert.deepEqual(c.decode([[1, 2, 3]], "$"), {
    tag: "Err",
    error: { kind: "StructuralMismatch", path: "$[0]", expected: "[key, value] entry", actual: "object" },
  });
  assert.deepEqual(c.decode([[{ x: 1 }, "no"]], "$"), {
    tag: "Err",
    error: { kind: "StructuralMismatch", path: "$[0][1]", expected: "number", actual: "string" },
  });
  const plain = __bynkCodecMap(str, int).decode([["a", 1]], "$");
  assert.ok(plain.tag === "Ok" && !(plain.value instanceof __BynkKeyMap) && plain.value.get("a") === 1);
});
