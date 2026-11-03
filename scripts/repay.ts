// Pays on a line: late interest, then interest, then principal, never more than is due. Anyone may
// pay; the payer here is the borrower unless --payer-keypair names another wallet. A payment that
// would touch more pagaré records than one `repay` can carry is split into several (at most five
// records each), so a large payoff may take more than one transaction.
//
//   npx tsx scripts/repay.ts --cluster local|devnet --pool <address> --line-id <n>
//        (--amount <tMXN> | --all) [--payer-keypair <path>] [--receipt-hash <64 hex>] [--yes]
//
// --amount        what to pay, in tMXN; the program takes no more than is due
// --all           pay everything due now (with late interest on a scheduled line)
// --payer-keypair default ELOTE_BORROWER_KEYPAIR or .demo-keys/borrower.json
// --receipt-hash  hash of the payment's receipt (zero, "receipt to follow", if absent); the holder
//                 may attach the real one later with annotate_payment
// --yes           send without asking when the cluster is not this machine
//
// ELOTE_RPC overrides the cluster's RPC URL. Keys are never printed, only addresses.
//
// Demo on Solana devnet with test assets. Not an offer of securities.
import { address } from "@solana/kit";
import { fetchToken } from "@solana-program/token";
import { LineStatus, fetchCreditLine, lineArrears, lineSchedule } from "../clients/js/src/index.ts";
import {
  DISCLAIMER,
  arg,
  chainNow,
  checkFlags,
  cluster,
  confirmCluster,
  fail,
  flag,
  fmt,
  hash32,
  loadSigner,
  parseAmount,
  required,
} from "./lib/cli.ts";
import { ataOf, loadLine, loadPool, repayInParts } from "./lib/pool.ts";

checkFlags(["cluster", "pool", "line-id", "amount", "all", "payer-keypair", "receipt-hash", "yes"]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
if (flag("all") === (arg("amount") !== undefined)) fail("give either --amount or --all");

await confirmCluster(c, "repay");
const receiptHash = arg("receipt-hash") !== undefined ? hash32("receipt-hash") : new Uint8Array(32);
const payer = await loadSigner(
  "payer",
  arg("payer-keypair") ?? process.env.ELOTE_BORROWER_KEYPAIR ?? ".demo-keys/borrower.json"
);
const pool = await loadPool(c, poolAddress);
const { line, data: l } = await loadLine(c, poolAddress, lineId);
if (l.status !== LineStatus.Active && l.status !== LineStatus.InCollection) fail(`the line is ${LineStatus[l.status]}: nothing to repay`);

const s = lineSchedule(l);
const late = s ? lineArrears(l, s, BigInt(pool.gracePeriodSeconds), await chainNow(c)).lateDue : 0n;
const owed = l.drawn - l.principalRepaid + l.interestDue - l.interestRepaid + late;
// Late interest grows by the second: --all pads it, and the program takes only what is due.
const amount = flag("all") ? owed + late + 1_000n : parseAmount("amount", required("amount"));
const balance = (await fetchToken(c.rpc, await ataOf(payer.address, pool.assetMint), { commitment: "confirmed" })).data.amount;
if (balance < (amount < owed ? amount : owed)) fail(`the payer holds ${fmt(balance)} tMXN`);
console.log(`line ${line} (id ${lineId}): owed ${fmt(owed)} tMXN now${late ? `, ${fmt(late)} of it late interest` : ""}; paying ${fmt(amount < owed ? amount : owed)}`);

const paid = await repayInParts(c, { pool: poolAddress, poolData: pool, line, payer, amount, receiptHash });
const after = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data;
console.log(
  [
    `paid         ${fmt(paid)} tMXN`,
    `line now     principal repaid ${fmt(after.principalRepaid)} of ${fmt(after.drawn)}, interest ${fmt(after.interestRepaid)} of ${fmt(after.interestDue)}`,
    `pagarés      ${after.pagaresPaid} of ${after.draws} marked paid (cancel each with cancel-pagare.ts)`,
    `status       ${LineStatus[after.status]}`,
  ].join("\n")
);
console.log(`\n${DISCLAIMER}`);
