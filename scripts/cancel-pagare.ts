// Extinguishes a paid pagaré. A repayment that reached the pagaré's `drawnThrough` marked it paid
// (PAGADO); cancellation burns the asset against the final receipt's hash, and the record stays.
//
//   npx tsx scripts/cancel-pagare.ts --cluster local|devnet --pool <address> --line-id <n>
//        --draw-index <n> [--receipt-hash <64 hex> | --receipt-file <path> --receipt-salt <64 hex>]
//        [--borrower-keypair <path> | --procurador-keypair <path> | --council-keypairs <a.json,b.json>] [--yes]
//
// One branch per run:
//   (none)                the council cancels as the holder, through Squads (the members' keys from
//                         --council-keypairs, ELOTE_COUNCIL_KEYPAIRS or .demo-keys/curator-<n>.json)
//   --borrower-keypair    the line's borrower cancels alone, once the pool's cancel grace from the
//                         payment has run; before that the program refuses (CancelGraceNotElapsed)
//   --procurador-keypair  the procurador of record cancels alone through its program address
//
// --receipt-hash      hash of the FEA-signed receipt (a placeholder if absent); --receipt-file with
//                     --receipt-salt hashes the receipt as sha256(salt || file) instead
// --yes               send without asking when the cluster is not this machine
//
// On the borrower's and the procurador's branches the admin key (ELOTE_ADMIN_KEYPAIR or
// .demo-keys/admin.json) pays the fees and gets the asset's rent back; on the council's, the
// executing member does. ELOTE_RPC overrides the cluster's RPC URL.
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
  documentHash,
  integer,
  loadSigner,
  required,
  sendOrFail,
} from "./lib/cli.ts";
import { councilExecute, loadCouncil } from "./lib/council.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

const PAGARE_OUTSTANDING = 0; // state::PAGARE_*
const PAGARE_PAID = 1;
const PAGARE_FLAG_PROCURACION = 1 << 1; // state::PAGARE_FLAG_PROCURACION

checkFlags([
  "cluster", "pool", "line-id", "draw-index", "receipt-hash", "receipt-file", "receipt-salt", "borrower-keypair",
  "procurador-keypair", "council-keypairs", "yes",
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
const receiptHash = documentHash("receipt", "elote.fun demo receipt");
const pool = await loadPool(c, poolAddress);
const { line, data: l } = await loadLine(c, poolAddress, lineId);
const [record] = await findPagarePda({ line, drawIndex });
const before = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
if (before.status === PAGARE_PAID) fail(`pagaré ${drawIndex} is already extinguished`);
if (before.status === PAGARE_OUTSTANDING) fail(`no repayment has marked pagaré ${drawIndex} paid yet (repay.ts)`);

{
  const council = branch === "council" ? await loadCouncil(c, pool.multisig) : null;
  const admin = council ? council.members[0] : await loadSigner("admin", process.env.ELOTE_ADMIN_KEYPAIR ?? ".demo-keys/admin.json");
  let holder = council ? council.signer : admin;
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
  const label = `cancel_pagare ${drawIndex} (${branch})`;
  const ix = await getCancelPagareInstructionAsync({
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
    });
  if (council) await councilExecute(c, council, label, [ix], [admin]).catch((e) => fail((e as Error).message));
  else await sendOrFail(c, label, admin, [ix]);
  const after = (await fetchPagare(c.rpc, record, { commitment: "confirmed" })).data;
  const asset = await c.rpc.getAccountInfo(before.asset, { encoding: "base64", commitment: "confirmed" }).send();
  console.log(
    `  pagaré ${drawIndex}: status ${after.status === PAGARE_PAID ? "PAID (extinguished)" : after.status}, ` +
      `asset ${before.asset} ${asset.value && asset.value.data[0].length > 4 ? "still exists" : "burnt"}`
  );
}
console.log(`\n${DISCLAIMER}`);
