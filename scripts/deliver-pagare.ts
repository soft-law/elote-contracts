// Delivers a paid pagaré (decision 38, LGTOC 129-130): the pool thaws the Core asset, renounces
// the freeze forever and transfers it to the line's borrower — or to the aval who settled it
// (LGTOC 115). Anyone may send it; the recipient is the debtor's, never the sender's, choice.
// With --burn, the delivered título's owner burns the asset instead (a Terminacion entry first);
// the record is never closed.
//
//   npx tsx scripts/deliver-pagare.ts --cluster local|devnet --pool <address> --line-id <n>
//        --draw-index <n> [--burn --owner-keypair <path>] [--yes]
//
// The admin key (ELOTE_ADMIN_KEYPAIR or .demo-keys/admin.json) pays the fees on the delivery; the
// burn is signed and paid by the owner, who takes the asset's rent back. ELOTE_RPC overrides the
// cluster's RPC URL.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import {
  fetchPagare,
  findPagarePda,
  getBurnPagareInstructionAsync,
  getDeliverPagareInstructionAsync,
} from "../clients/js/src/index.ts";
import {
  DISCLAIMER,
  arg,
  flag,
  checkFlags,
  cluster,
  confirmCluster,
  fail,
  integer,
  loadSigner,
  required,
  sendOrFail,
} from "./lib/cli.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

const PAGARE_PAID = 1; // state::PAGARE_*
const PAGARE_FLAG_DELIVERED = 1 << 2; // state::PAGARE_FLAG_*
const PAGARE_FLAG_PAID_BY_AVAL = 1 << 3;

checkFlags(["cluster", "pool", "line-id", "draw-index", "burn", "owner-keypair", "yes"]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
const drawIndex = integer("draw-index", undefined, 0, 0xffff) ?? fail("--draw-index is required");
const burn = flag("burn");
if (!burn && arg("owner-keypair") !== undefined) fail("--owner-keypair goes with --burn");

await confirmCluster(c, burn ? "burn_pagare" : "deliver_pagare");
const pool = await loadPool(c, poolAddress);
const { line, data: l } = await loadLine(c, poolAddress, lineId);
const [record] = await findPagarePda({ line, drawIndex });
const before = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
if (before.status !== PAGARE_PAID) fail(`no repayment has marked pagaré ${drawIndex} paid yet (repay.ts)`);

if (burn) {
  if ((before.flags & PAGARE_FLAG_DELIVERED) === 0) fail(`pagaré ${drawIndex} is not delivered yet: deliver it first`);
  const owner = await loadSigner("owner", arg("owner-keypair") ?? fail("--burn needs --owner-keypair"));
  const ix = await getBurnPagareInstructionAsync({
    owner,
    pool: poolAddress,
    line,
    pagare: record,
    asset: before.asset,
    collection: pool.collection,
    drawIndex,
    // The burn is the owner's unilateral act: no document backs it, and the program allows zero.
    documentHash: new Uint8Array(32),
  });
  await sendOrFail(c, `burn_pagare ${drawIndex}`, owner, [ix]);
  const after = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
  console.log(`  pagaré ${drawIndex}: asset burnt by its owner; the record stays (${after.entryCount} registry entries)`);
} else {
  if ((before.flags & PAGARE_FLAG_DELIVERED) !== 0) fail(`pagaré ${drawIndex} is already delivered`);
  const byAval = (before.flags & PAGARE_FLAG_PAID_BY_AVAL) !== 0;
  const sponsor = l.sponsor.__option === "Some" ? l.sponsor.value : undefined;
  const recipient = byAval ? (sponsor ?? fail("the aval paid but the line names no sponsor")) : l.borrower;
  const payer = await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
  const ix = await getDeliverPagareInstructionAsync({
    payer,
    pool: poolAddress,
    line,
    pagare: record,
    asset: before.asset,
    collection: pool.collection,
    recipient,
    drawIndex,
  });
  await sendOrFail(c, `deliver_pagare ${drawIndex}`, payer, [ix]);
  const after = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
  const delivered = (after.flags & PAGARE_FLAG_DELIVERED) !== 0;
  console.log(
    `  pagaré ${drawIndex}: ${delivered ? "delivered" : "NOT delivered"} to ${byAval ? "the aval" : "the borrower"} ` +
      `${recipient}; ${after.entryCount} registry entries`
  );
}
console.log(`\n${DISCLAIMER}`);
