import assert from "node:assert/strict";
import test from "node:test";
import { orderProviderRows } from "../src/providerRowOrder.ts";

const official = { id: "official", isCurrent: false };
const detected = { id: "detected", isCurrent: true };

test("a copied provider is last while the selected saved ID stays current", () => {
  const rows = orderProviderRows(official, [detected], [
    { id: "older", isCurrent: false },
    { id: "selected", isCurrent: true },
    { id: "copy", isCurrent: false },
  ]);

  assert.deepEqual(rows.map((row) => row.id), ["official", "older", "selected", "copy"]);
  assert.deepEqual(rows.filter((row) => row.isCurrent).map((row) => row.id), ["selected"]);
});

test("an unresolved detected current remains visible before saved rows", () => {
  const rows = orderProviderRows(official, [detected], [
    { id: "original", isCurrent: false },
    { id: "copy", isCurrent: false },
  ]);

  assert.deepEqual(rows.map((row) => row.id), ["official", "detected", "original", "copy"]);
  assert.deepEqual(rows.filter((row) => row.isCurrent).map((row) => row.id), ["detected"]);
});
