import assert from "node:assert/strict";
import { test } from "node:test";
import { akeyChecksum, randomAKey } from "./akey.ts";

test("A-key check digits match the S.S0053 vector", () => {
  assert.equal(akeyChecksum(14142135623730950488n, 0xd75a96ec), "086500");
});

test("manual handset entry matches the calculator vector", () => {
  assert.equal(akeyChecksum(11223344556677889900n, 0xf274799a), "010032");
});

test("random A-keys retain leading zeros and the full unsigned range", () => {
  for (const fill of [0, 255]) {
    const original = crypto.getRandomValues;
    try {
      crypto.getRandomValues = (bytes) => bytes.fill(fill);
      const key = randomAKey();
      assert.equal(key.length, 20);
      assert.equal(BigInt(key), fill === 0 ? 0n : 18446744073709551615n);
    } finally {
      crypto.getRandomValues = original;
    }
  }
});
