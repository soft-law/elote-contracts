// Endorses a pagaré en procuración, or revokes the procuración in force. The pool stays the owner;
// the procurador gets the power to collect and to extinguish the title through the program, which
// approves the asset's burn delegate to the program address ["procurador", pagare, procurador].
//
//   npx tsx scripts/endorse-pagare.ts --cluster local|devnet --pool <address> --line-id <n>
//        --draw-index <n> (--procurador <address> [--endoso-hash <64 hex>] | --revoke [--revocation-hash <64 hex>])
//        [--yes]
//
// --procurador       the endorsee: never the pool, the council, a curator, the guardian or the borrower
//                    (the program refuses them)
// --endoso-hash      hash of the signed endoso (a placeholder if absent)
// --revoke           revoke the procuración in force
// --revocation-hash  hash of the signed revocation (a placeholder if absent)
// --yes              send without asking when the cluster is not this machine
//
// The council (the holder while the asset is in the pool) signs through Squads, so the script runs
// only with the council tooling (PR 4, commit 10); the admin key (ELOTE_ADMIN_KEYPAIR or
// .demo-keys/admin.json) stands in for it and pays the fees; ELOTE_RPC overrides the cluster's RPC URL.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import {
  fetchPagare,
  findPagarePda,
  findProcuradorAuthorityPda,
  getEndorsePagareInstructionAsync,
  getRevokeProcuracionInstructionAsync,
} from "../clients/js/src/index.ts";
import {
  DISCLAIMER,
  arg,
  checkFlags,
  chainNow,
  cluster,
  confirmCluster,
  fail,
  flag,
  hash32,
  integer,
  loadSigner,
  required,
  sendOrFail,
} from "./lib/cli.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

const KIND_PROCURACION = 2; // state::KIND_PROCURACION
const PAGARE_OUTSTANDING = 0; // state::PAGARE_OUTSTANDING
const PAGARE_FLAG_PROCURACION = 1 << 1; // state::PAGARE_FLAG_PROCURACION

checkFlags([
  "cluster", "pool", "line-id", "draw-index", "procurador", "endoso-hash", "revoke", "revocation-hash", "yes",
]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
const drawIndex = integer("draw-index", undefined, 0, 0xffff) ?? fail("--draw-index is required");
const revoke = flag("revoke");
if (revoke === (arg("procurador") !== undefined)) fail("give either --procurador or --revoke");
if (revoke && arg("endoso-hash") !== undefined) fail("--endoso-hash goes with --procurador");
if (!revoke && arg("revocation-hash") !== undefined) fail("--revocation-hash goes with --revoke");

await confirmCluster(c, revoke ? "revoke_procuracion" : "endorse_pagare");
const admin = await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
const pool = await loadPool(c, poolAddress);
if (pool.council !== admin.address) fail(`the key (${admin.address}) is not this pool's council (${pool.council}); a Squads council signs through Squads`);
const { line } = await loadLine(c, poolAddress, lineId);
const [record] = await findPagarePda({ line, drawIndex });
const before = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
const inForce = (before.flags & PAGARE_FLAG_PROCURACION) !== 0;

if (revoke) {
  if (!inForce) fail(`pagaré ${drawIndex} has no procuración in force`);
  const entryHash = hash32("revocation-hash", "elote.fun demo revocation");
  await sendOrFail(c, `revoke_procuracion of pagaré ${drawIndex} (procurador ${before.procurador})`, admin, [
    await getRevokeProcuracionInstructionAsync({
      holder: admin,
      multisig: pool.multisig,
      validUntil: (await chainNow(c)) + 86_400n,
      payer: admin,
      pool: poolAddress,
      line,
      pagare: record,
      asset: before.asset,
      collection: pool.collection,
      drawIndex,
      entryHash,
    }),
  ]);
} else {
  if (before.status !== PAGARE_OUTSTANDING) fail(`pagaré ${drawIndex} is already marked paid: endorse before the payment`);
  if (inForce) fail(`pagaré ${drawIndex} already has a procuración in force (${before.procurador}); revoke it first`);
  const procurador = address(required("procurador"));
  if (procurador === poolAddress || procurador === pool.council || procurador === pool.guardian) {
    fail("the procurador cannot be the pool, the council or the guardian");
  }
  const endosoHash = hash32("endoso-hash", "elote.fun demo endoso en procuración");
  await sendOrFail(c, `endorse_pagare ${drawIndex} en procuración to ${procurador}`, admin, [
    await getEndorsePagareInstructionAsync({
      holder: admin,
      multisig: pool.multisig,
      validUntil: (await chainNow(c)) + 86_400n,
      payer: admin,
      pool: poolAddress,
      line,
      pagare: record,
      asset: before.asset,
      collection: pool.collection,
      drawIndex,
      kind: KIND_PROCURACION,
      to: procurador,
      endosoHash,
    }),
  ]);
  const [authority] = await findProcuradorAuthorityPda({ pagare: record, procurador });
  console.log(`  the burn delegate is approved to ${authority}; the procurador cancels with cancel-pagare.ts --procurador-keypair`);
}

const after = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
const nowInForce = (after.flags & PAGARE_FLAG_PROCURACION) !== 0;
console.log(`  pagaré ${drawIndex}: procuración ${nowInForce ? `in force, procurador ${after.procurador}` : "none"}; ${after.entryCount} registry entries`);
console.log(`\n${DISCLAIMER}`);
