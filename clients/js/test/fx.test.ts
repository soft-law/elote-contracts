// The same cases as programs/elote_pool/src/fx.rs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mxnForUsdc, usdcCeilForMxn, usdcForMxn } from "../src/fx";

test("conversions round as the program does", () => {
  assert.equal(usdcForMxn(5_000_000n, 183_740n), 2_721_236_529n);
  assert.equal(mxnForUsdc(100_000_000n, 183_740n), 183_740n);
  assert.equal(mxnForUsdc(1n, 183_740n), 0n);
  assert.equal(usdcCeilForMxn(183_740n, 183_740n), 100_000_000n);
  assert.equal(usdcCeilForMxn(1n, 183_740n), 545n);
});
