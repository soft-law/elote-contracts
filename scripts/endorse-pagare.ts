// Endorses a pagaré en procuración, or revokes the procuración in force. The pool stays the owner;
// the procurador of record collects through repay, like any payer, and litigates with the
// registry's entry; no power over the asset is lent (decision 38).
//
//   npx tsx scripts/endorse-pagare.ts --cluster local|devnet --pool <address> --line-id <n>
//        --draw-index <n> ([--procurador <address>] [--endoso-hash <64 hex>] | --revoke [--revocation-hash <64 hex>])
//        [--council-keypairs <a.json,b.json>] [--yes]
//
// --procurador       the endorsee: never the pool, the council, a curator, the creator,
//                    the FIX feeder, or the line's borrower, payer or sponsor
//                    (the program refuses them). Without it the script creates a key for this case,
//                    .demo-keys/procuradores/<pool>-<line>-<draw>.json, to hand to the collection
//                    agency: one key per case, never reused, or anyone could list the lines in
//                    collection from it. With it, the script warns when the key already holds a
//                    pagaré under procuración.
// --endoso-hash      hash of the signed endoso (a placeholder if absent)
// --revoke           revoke the procuración in force
// --revocation-hash  hash of the signed revocation (a placeholder if absent)
// --yes              send without asking when the cluster is not this machine
//
// The council (the holder while the asset is in the pool) decides through Squads: the members' keys
// from --council-keypairs, ELOTE_COUNCIL_KEYPAIRS or .demo-keys/curator-<n>.json; the executing member
// pays. ELOTE_RPC overrides the cluster's RPC URL.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import path from "node:path";
import { address } from "@solana/kit";
import {
  fetchPagare,
  findPagarePda,
  getEndorsePagareInstructionAsync,
  getRevokeProcuracionInstructionAsync,
  pagaresUnderProcuracion,
} from "../clients/js/src/index.ts";
import {
  DISCLAIMER,
  KEY_DIR,
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
  writeNewKey,
} from "./lib/cli.ts";
import { councilExecute, loadCouncil } from "./lib/council.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

const KIND_PROCURACION = 2; // state::KIND_PROCURACION
const PAGARE_OUTSTANDING = 0; // state::PAGARE_OUTSTANDING
const PAGARE_FLAG_PROCURACION = 1 << 1; // state::PAGARE_FLAG_PROCURACION

checkFlags([
  "cluster", "pool", "line-id", "draw-index", "procurador", "endoso-hash", "revoke", "revocation-hash", "council-keypairs",
  "yes",
]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
const drawIndex = integer("draw-index", undefined, 0, 0xffff) ?? fail("--draw-index is required");
const revoke = flag("revoke");
if (revoke && arg("procurador") !== undefined) fail("--procurador goes with an endorsement, not --revoke");
if (revoke && arg("endoso-hash") !== undefined) fail("--endoso-hash goes with --procurador");
if (!revoke && arg("revocation-hash") !== undefined) fail("--revocation-hash goes with --revoke");

await confirmCluster(c, revoke ? "revoke_procuracion" : "endorse_pagare");
const pool = await loadPool(c, poolAddress);
const council = await loadCouncil(c, pool.multisig);
const payer = council.members[0];
const validUntil = (await chainNow(c)) + 86_400n;
const { line, data: lineData } = await loadLine(c, poolAddress, lineId);
const [record] = await findPagarePda({ line, drawIndex });
const before = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
const inForce = (before.flags & PAGARE_FLAG_PROCURACION) !== 0;

if (revoke) {
  if (!inForce) fail(`pagaré ${drawIndex} has no procuración in force`);
  const entryHash = hash32("revocation-hash", "elote.fun demo revocation");
  await councilExecute(c, council, `revoke_procuracion of pagaré ${drawIndex}`, [
    await getRevokeProcuracionInstructionAsync({
      holder: council.signer,
      multisig: pool.multisig,
      validUntil,
      pool: poolAddress,
      line,
      pagare: record,
      drawIndex,
      entryHash,
    }),
  ], [payer]).catch((e) => fail((e as Error).message));
} else {
  if (before.status !== PAGARE_OUTSTANDING) fail(`pagaré ${drawIndex} is already marked paid: endorse before the payment`);
  if (inForce) fail(`pagaré ${drawIndex} already has a procuración in force (${before.procurador}); revoke it first`);
  let procurador;
  if (arg("procurador") !== undefined) {
    procurador = address(arg("procurador")!);
    const others = await pagaresUnderProcuracion(c.rpc, procurador);
    if (others.length > 0) {
      console.log(`  warning: ${procurador} already holds ${others.length} pagaré(s) under procuración; one key per case keeps the lines in collection from being listed`);
    }
  } else {
    const file = path.join(KEY_DIR, "procuradores", `${poolAddress}-${lineId}-${drawIndex}.json`);
    writeNewKey(file);
    procurador = (await loadSigner("procurador", file)).address;
    console.log(`  a new procurador key for this case, ${procurador}, is in ${file}: hand it to the collection agency`);
  }
  const sponsor = lineData.sponsor.__option === "Some" ? lineData.sponsor.value : undefined;
  const refused = [poolAddress, pool.council, pool.creator, pool.fxFeeder, lineData.borrower, lineData.payer, sponsor];
  if (refused.includes(procurador) || council.members.some((m) => m.address === procurador)) {
    fail("the procurador cannot be the pool, the council, a curator, the creator, the FIX feeder, or the line's borrower, payer or sponsor");
  }
  const endosoHash = hash32("endoso-hash", "elote.fun demo endoso en procuración");
  await councilExecute(c, council, `endorse_pagare ${drawIndex} en procuración`, [
    await getEndorsePagareInstructionAsync({
      holder: council.signer,
      multisig: pool.multisig,
      validUntil,
      pool: poolAddress,
      line,
      pagare: record,
      drawIndex,
      kind: KIND_PROCURACION,
      to: procurador,
      endosoHash,
    }),
  ], [payer]).catch((e) => fail((e as Error).message));
  console.log(`  the procurador of record collects through repay.ts; the entry is the endorsement (no power over the asset is lent)`);
}

const after = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
const nowInForce = (after.flags & PAGARE_FLAG_PROCURACION) !== 0;
console.log(`  pagaré ${drawIndex}: procuración ${nowInForce ? `in force, procurador ${after.procurador}` : "none"}; ${after.entryCount} registry entries`);
console.log(`\n${DISCLAIMER}`);
