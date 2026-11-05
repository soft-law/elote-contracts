// The same cases as programs/elote_pool/src/fx.rs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { MAX_FIX_REFERENCE_GAP_BPS, mxnForUsdc, referenceRate, usdcCeilForMxn, usdcForMxn, withinBps } from "../src/fx";

test("conversions round as the program does", () => {
  assert.equal(usdcForMxn(5_000_000n, 183_740n), 2_721_236_529n);
  assert.equal(mxnForUsdc(100_000_000n, 183_740n), 183_740n);
  assert.equal(mxnForUsdc(1n, 183_740n), 0n);
  assert.equal(usdcCeilForMxn(183_740n, 183_740n), 100_000_000n);
  assert.equal(usdcCeilForMxn(1n, 183_740n), 545n);
});

// RedStone's MXNe account on mainnet as read on 2026-11-05: the first 98 of its 176 bytes.
const MAINNET_REFERENCE =
  "e871c1e785d1ce9a4d584e65000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000521a857090ababa1010000014098ababa10100005d16c21b0000000008";

const mainnetReference = () => {
  const data = new Uint8Array(176);
  data.set(Uint8Array.from(MAINNET_REFERENCE.match(/../g)!.map((h) => parseInt(h, 16))));
  return data;
};

test("the reference reads as the program reads it", () => {
  assert.deepEqual(referenceRate(mainnetReference()), { rate: 185_848n, at: 1_793_881_510n });
  for (const [at, byte] of [
    [0, 0xe9],
    [11, 0x78],
    [12, 1],
    [40, 1],
    [80, 2],
    [97, 6],
  ]) {
    const bad = mainnetReference();
    bad[at] = byte;
    assert.throws(() => referenceRate(bad), `byte ${at}`);
  }
  assert.throws(() => referenceRate(mainnetReference().subarray(0, 97)));
  assert.ok(withinBps(200_000n, 204_000n, MAX_FIX_REFERENCE_GAP_BPS));
  assert.ok(!withinBps(200_000n, 195_999n, MAX_FIX_REFERENCE_GAP_BPS));
});
