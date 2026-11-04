// Extinguishes a paid pagaré. A repayment that reached the pagaré's `drawnThrough` marked it paid
// (PAGADO); cancellation burns the asset against the final receipt's hash, and the record stays.
//
//   npx tsx scripts/cancel-pagare.ts --cluster local|devnet --pool <address> --line-id <n>
//        --draw-index <n> [--receipt-hash <64 hex>] [--borrower-keypair <path> | --procurador-keypair <path>]
//        [--yes]
//
// One branch per run:
//   (none)                the council cancels as the holder; it signs through Squads, so this branch
//                         runs only with the council tooling (PR 4, commit 10)
//   --borrower-keypair    the line's borrower cancels alone, once the pool's cancel grace from the
//                         payment has run; before that the program refuses (CancelGraceNotElapsed)
//   --procurador-keypair  the procurador of record cancels alone through its program address
//
// --receipt-hash      hash of the FEA-signed receipt (a placeholder if absent)
// --yes               send without asking when the cluster is not this machine
//
// The admin key comes from ELOTE_ADMIN_KEYPAIR or .demo-keys/admin.json and pays the fees (the
// asset's rent comes back to it); ELOTE_RPC overrides the cluster's RPC URL.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import {
  fetchPagare,
  findPagarePda,
  findProcuradorAuthorityPda,
  getCancelPagareInstructionAsync,
} from "../clients/js/src/index.ts";
import {
  DISCLAIMER,
  arg,
  chainNow,
  checkFlags,
  cluster,
  confirmCluster,
  fail,
  hash32,
  integer,
  loadSigner,
  required,
  sendOrFail,
} from "./lib/cli.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

const PAGARE_OUTSTANDING = 0; // state::PAGARE_*
const PAGARE_PAID = 1;
const PAGARE_FLAG_PROCURACION = 1 << 1; // state::PAGARE_FLAG_PROCURACION

checkFlags([
  "cluster", "pool", "line-id", "draw-index", "receipt-hash", "borrower-keypair", "procurador-keypair", "yes",
]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
const drawIndex = integer("draw-index", undefined, 0, 0xffff) ?? fail("--draw-index is required");
if (arg("borrower-keypair") !== undefined && arg("procurador-keypair") !== undefined) fail("give at most one of --borrower-keypair and --procurador-keypair");
const branch = arg("borrower-keypair") ? "borrower" : arg("procurador-keypair") ? "procurador" : "council";

await confirmCluster(c, "cancel_pagare");
const receiptHash = hash32("receipt-hash", "elote.fun demo receipt");
const admin = await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
const pool = await loadPool(c, poolAddress);
if (branch === "council" && pool.council !== admin.address) {
  fail(`the key (${admin.address}) is not this pool's council (${pool.council}); a Squads council signs through Squads`);
}
const { line, data: l } = await loadLine(c, poolAddress, lineId);
const [record] = await findPagarePda({ line, drawIndex });
const before = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
if (before.status === PAGARE_PAID) fail(`pagaré ${drawIndex} is already extinguished`);
if (before.status === PAGARE_OUTSTANDING) fail(`no repayment has marked pagaré ${drawIndex} paid yet (repay.ts)`);

{
  let holder = admin;
  if (branch === "borrower") {
    holder = await loadSigner("borrower", arg("borrower-keypair")!);
    if (l.borrower !== holder.address) fail(`${holder.address} is not the borrower of line ${lineId}`);
    const opens = before.paidAt + BigInt(pool.cancelGraceSeconds);
    const now = await chainNow(c);
    if (now <= opens) {
      console.log(`  the cancel grace runs until ${new Date(Number(opens) * 1000).toISOString()}, ${opens - now} s away: the program will refuse (CancelGraceNotElapsed)`);
    }
  }
  let procuradorAuthority = undefined;
  if (branch === "procurador") {
    holder = await loadSigner("procurador", arg("procurador-keypair")!);
    if ((before.flags & PAGARE_FLAG_PROCURACION) === 0 || before.procurador !== holder.address) {
      fail(`${holder.address} is not the procurador of record of pagaré ${drawIndex}`);
    }
    [procuradorAuthority] = await findProcuradorAuthorityPda({ pagare: record, procurador: holder.address });
  }
  await sendOrFail(c, `cancel_pagare ${drawIndex} (${branch})`, admin, [
    await getCancelPagareInstructionAsync({
      holder,
      multisig: pool.multisig,
      // A council decision expires a day after it is built (Squads proposals never do).
      validUntil: (await chainNow(c)) + 86_400n,
      payer: admin,
      pool: poolAddress,
      line,
      pagare: record,
      asset: before.asset,
      collection: pool.collection,
      ...(procuradorAuthority ? { procuradorAuthority } : {}),
      drawIndex,
      receiptHash,
    }),
  ]);
  const after = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
  const asset = await c.rpc.getAccountInfo(before.asset, { encoding: "base64", commitment: "confirmed" }).send();
  console.log(
    `  pagaré ${drawIndex}: status ${after.status === PAGARE_PAID ? "PAID (extinguished)" : after.status}, ` +
      `asset ${before.asset} ${asset.value && asset.value.data[0].length > 4 ? "still exists" : "burnt"}`
  );
}
console.log(`\n${DISCLAIMER}`);
