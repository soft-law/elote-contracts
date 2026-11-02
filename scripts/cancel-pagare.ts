// Extinguishes a paid pagaré, or files the request that lets the holder do it alone later. A
// repayment that reached the pagaré's `drawnThrough` marked it paid (PAGADO); cancellation burns
// the asset against the final receipt's hash, and the record stays.
//
//   npx tsx scripts/cancel-pagare.ts --cluster local|devnet --pool <address> --line-id <n>
//        --draw-index <n> [--receipt-hash <64 hex>] [--request | --backstop | --procurador-keypair <path>]
//        [--attestor-keypair <path>] [--yes]
//
// One branch per run:
//   (none)                the holder (the pool's admin) cancels, the attestor co-signs
//   --request             the holder files a cancellation request; the holder may then cancel
//                         alone once the pool's cancel delay has run (a new request replaces it)
//   --backstop            the holder cancels alone, after a request's deadline, with the requested
//                         receipt; before the deadline the program refuses (AttestorRequired)
//   --procurador-keypair  the procurador of record cancels through its program address, the
//                         attestor co-signs
//
// --receipt-hash      hash of the FEA-signed receipt (a placeholder if absent)
// --attestor-keypair  default ELOTE_ATTESTOR_KEYPAIR or .demo-keys/attestor.json
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
  getRequestCancellationInstructionAsync,
} from "../clients/js/src/index.ts";
import {
  DISCLAIMER,
  arg,
  chainNow,
  checkFlags,
  cluster,
  confirmCluster,
  fail,
  flag,
  hash32,
  integer,
  loadSigner,
  required,
  roleSigner,
  sendOrFail,
} from "./lib/cli.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

const PAGARE_OUTSTANDING = 0; // state::PAGARE_*
const PAGARE_PAID = 1;
const PAGARE_FLAG_PROCURACION = 1 << 1; // state::PAGARE_FLAG_PROCURACION

checkFlags([
  "cluster", "pool", "line-id", "draw-index", "receipt-hash", "request", "backstop", "procurador-keypair",
  "attestor-keypair", "yes",
]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
const drawIndex = integer("draw-index", undefined, 0, 0xffff) ?? fail("--draw-index is required");
const branches = [flag("request"), flag("backstop"), arg("procurador-keypair") !== undefined].filter(Boolean).length;
if (branches > 1) fail("give at most one of --request, --backstop and --procurador-keypair");
const branch = flag("request") ? "request" : flag("backstop") ? "backstop" : arg("procurador-keypair") ? "procurador" : "attested";

await confirmCluster(c, branch === "request" ? "request_cancellation" : "cancel_pagare");
const receiptHash = hash32("receipt-hash", "elote.fun demo receipt");
const admin = await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
const pool = await loadPool(c, poolAddress);
if (pool.admin !== admin.address) fail(`the admin key (${admin.address}) is not this pool's admin (${pool.admin})`);
const { line } = await loadLine(c, poolAddress, lineId);
const [record] = await findPagarePda({ line, drawIndex });
const before = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
if (before.status === PAGARE_PAID) fail(`pagaré ${drawIndex} is already extinguished`);
if (before.status === PAGARE_OUTSTANDING) fail(`no repayment has marked pagaré ${drawIndex} paid yet (repay.ts)`);

if (branch === "request") {
  await sendOrFail(c, `request_cancellation of pagaré ${drawIndex}`, admin, [
    await getRequestCancellationInstructionAsync({ holder: admin, pool: poolAddress, line, drawIndex, receiptHash }),
  ]);
  const after = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
  console.log(`  the holder may cancel alone (--backstop) after ${new Date(Number(after.cancelAfter) * 1000).toISOString()}, with this receipt hash`);
} else {
  if (branch === "backstop") {
    const now = await chainNow(c);
    if (before.cancelAfter === 0n) console.log("  no cancellation request is pending: the program will refuse (AttestorRequired)");
    else if (now <= before.cancelAfter) {
      console.log(`  the request's deadline is ${new Date(Number(before.cancelAfter) * 1000).toISOString()}, ${before.cancelAfter - now} s away: the program will refuse (AttestorRequired)`);
    }
  }
  const attestor = branch === "backstop" ? null : await roleSigner("attestor");
  if (attestor && pool.attestor !== attestor.address) fail(`the attestor key (${attestor.address}) is not this pool's attestor (${pool.attestor})`);
  let holder = admin;
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
      ...(attestor ? { attestor } : {}),
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
