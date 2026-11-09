// The pagaré PDAs the program derives in `draw` (`["pagare" | "pagare_asset", line, draw_index as
// u16 LE]`). Codama derives the sidecar from `cancel_pagare` (`findPagarePda` in `generated/pdas`)
// and nothing for the asset, whose seed in `draw` is a field of the line account, so `src/pagare.ts`
// derives that one by hand; the expected addresses come from the Solana
// CLI (`solana find-program-derived-address <program> string:pagare pubkey:<line> u16le:<n>`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { AccountRole, address } from "@solana/kit";
import {
  MAX_SIDECARS_PER_REPAY,
  MPL_CORE_PROGRAM_ADDRESS,
  drawPagareAccounts,
  findPagareAssetPda,
  repayPrincipalCap,
  repaySidecars,
  withRemainingAccounts,
} from "../src/pagare";
import { findPagarePda } from "../src/generated";

const line = address("11111111111111111111111111111112");

test("repayPrincipalCap keeps a repay within maxRecords pagaré records", () => {
  const drawnThrough = [100n, 200n, 300n, 400n, 500n, 600n, 700n];
  // From cursor 0 with five records: pagarés 0..=3 covered, 4 the uncovered last record.
  assert.equal(repayPrincipalCap({ pagaresPaid: 0, drawnThrough, principalRepaid: 0n, maxRecords: 5 }), 400n);
  assert.equal(repayPrincipalCap({ pagaresPaid: 1, drawnThrough, principalRepaid: 150n, maxRecords: 5 }), 350n);
  // As many pagarés left as the limit, or fewer: the cap is all that is left.
  assert.equal(repayPrincipalCap({ pagaresPaid: 2, drawnThrough, principalRepaid: 250n, maxRecords: 5 }), 450n);
  assert.equal(repayPrincipalCap({ pagaresPaid: 4, drawnThrough, principalRepaid: 400n, maxRecords: 5 }), 300n);
});

test("MAX_SIDECARS_PER_REPAY is the program's state::MAX_SIDECARS_PER_REPAY", () => {
  const state = readFileSync(new URL("../../../programs/elote_pool/src/state.rs", import.meta.url), "utf8");
  const onChain = /pub const MAX_SIDECARS_PER_REPAY: usize = (\d+);/.exec(state);
  assert.ok(onChain, "state.rs declares MAX_SIDECARS_PER_REPAY");
  assert.equal(MAX_SIDECARS_PER_REPAY, Number(onChain[1]));
  assert.equal(MAX_SIDECARS_PER_REPAY, 3);
});

test("findPagarePda derives the sidecar address of a draw", async () => {
  assert.equal((await findPagarePda({ line, drawIndex: 0 }))[0], "5ckRzBZo2pcuZuHzfwLNBdiYWwJbU3GM8J78eYGo9n5H");
  assert.equal((await findPagarePda({ line, drawIndex: 300 }))[0], "C7NLJU5z4GzPiCzJfLGqQA5tRq16TxnvYhCsqT9GzPyB");
});

test("findPagareAssetPda derives the Core asset address of a draw", async () => {
  assert.equal((await findPagareAssetPda({ line, drawIndex: 0 }))[0], "4vzxVAuc1nczBTEPMBpGXiB7W3PyuRikEMbdd4m7zUcq");
  assert.equal((await findPagareAssetPda({ line, drawIndex: 300 }))[0], "AVo549EDYCCJ1UEW5WN56yikpEG17krYvATsoQ69DSGs");
});

test("drawPagareAccounts assembles the draw's pagaré accounts and bumps for the next index", async () => {
  const collection = address("11111111111111111111111111111113");
  const [, assetBump] = await findPagareAssetPda({ line, drawIndex: 300 });
  const [, pagareBump] = await findPagarePda({ line, drawIndex: 300 });
  assert.deepEqual(await drawPagareAccounts({ line, drawIndex: 300, collection }), {
    asset: "AVo549EDYCCJ1UEW5WN56yikpEG17krYvATsoQ69DSGs",
    assetBump,
    pagare: "C7NLJU5z4GzPiCzJfLGqQA5tRq16TxnvYhCsqT9GzPyB",
    pagareBump,
    collection,
    mplCoreProgram: "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d",
  });
});

test("the Metaplex Core program address is the one the program pins", () => {
  assert.equal(MPL_CORE_PROGRAM_ADDRESS, "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d");
});

test("repaySidecars selects the records from the cursor through the first pagaré left uncovered", async () => {
  const drawnThrough = [100n, 200n, 400n];
  const s = (...i: number[]) => Promise.all(i.map(async (drawIndex) => (await findPagarePda({ line, drawIndex }))[0]));
  // Interest only: the cursor's pagaré alone, as an Abono.
  assert.deepEqual(await repaySidecars({ line, pagaresPaid: 0, drawnThrough, principalAfter: 0n }), await s(0));
  // The first pagaré paid and half of the next: both.
  assert.deepEqual(await repaySidecars({ line, pagaresPaid: 0, drawnThrough, principalAfter: 150n }), await s(0, 1));
  // From the cursor through the last draw when the payment covers everything.
  assert.deepEqual(await repaySidecars({ line, pagaresPaid: 1, drawnThrough, principalAfter: 400n }), await s(1, 2));
  // Nothing left to mark: none.
  assert.deepEqual(await repaySidecars({ line, pagaresPaid: 3, drawnThrough, principalAfter: 400n }), []);
});

test("withRemainingAccounts appends writable, non-signer accounts to an instruction", () => {
  const other = address("11111111111111111111111111111113");
  const ix = { programAddress: line, accounts: [{ address: line, role: AccountRole.READONLY }], data: new Uint8Array() };
  const out = withRemainingAccounts(ix, [other]);
  assert.equal(out.accounts.length, 2);
  assert.deepEqual(out.accounts[1], { address: other, role: AccountRole.WRITABLE });
  assert.equal(ix.accounts.length, 1, "the input is not changed");
});
