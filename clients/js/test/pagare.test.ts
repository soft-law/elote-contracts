// The pagaré PDAs the program derives in `draw` (`["pagare" | "pagare_asset", line, draw_index as
// u16 LE]`). Codama generates no helper for them because their seed is a field of the line
// account, so `src/pagare.ts` derives them by hand; the expected addresses come from the Solana
// CLI (`solana find-program-derived-address <program> string:pagare pubkey:<line> u16le:<n>`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { address } from "@solana/kit";
import { MPL_CORE_PROGRAM_ADDRESS, drawPagareAccounts, findPagareAssetPda, findPagarePda } from "../src/pagare";

const line = address("11111111111111111111111111111112");

test("findPagarePda derives the sidecar address of a draw", async () => {
  assert.equal((await findPagarePda({ line, drawIndex: 0 }))[0], "5ckRzBZo2pcuZuHzfwLNBdiYWwJbU3GM8J78eYGo9n5H");
  assert.equal((await findPagarePda({ line, drawIndex: 300 }))[0], "C7NLJU5z4GzPiCzJfLGqQA5tRq16TxnvYhCsqT9GzPyB");
});

test("findPagareAssetPda derives the Core asset address of a draw", async () => {
  assert.equal((await findPagareAssetPda({ line, drawIndex: 0 }))[0], "4vzxVAuc1nczBTEPMBpGXiB7W3PyuRikEMbdd4m7zUcq");
  assert.equal((await findPagareAssetPda({ line, drawIndex: 300 }))[0], "AVo549EDYCCJ1UEW5WN56yikpEG17krYvATsoQ69DSGs");
});

test("drawPagareAccounts assembles the draw's pagaré accounts for the next index", async () => {
  const collection = address("11111111111111111111111111111113");
  assert.deepEqual(await drawPagareAccounts({ line, drawIndex: 300, collection }), {
    asset: "AVo549EDYCCJ1UEW5WN56yikpEG17krYvATsoQ69DSGs",
    pagare: "C7NLJU5z4GzPiCzJfLGqQA5tRq16TxnvYhCsqT9GzPyB",
    collection,
    mplCoreProgram: "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d",
  });
});

test("the Metaplex Core program address is the one the program pins", () => {
  assert.equal(MPL_CORE_PROGRAM_ADDRESS, "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d");
});
