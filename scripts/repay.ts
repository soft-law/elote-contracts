// Pays on a line: late interest, then interest (on an active line, the interest accrued to the
// payment), then principal, never more than is due. Anyone may
// pay; the payer here is the borrower unless --payer-keypair names another wallet. A payment that
// would touch more pagaré records than one `repay` can carry is split into several (at most three
// records each), so a large payoff may take more than one transaction.
//
//   npx tsx scripts/repay.ts --cluster local|devnet --pool <address> --line-id <n>
//        (--amount <USDC> | --all) [--payer-keypair <path>]
//        [--receipt-hash <64 hex> | --receipt-file <path> --receipt-salt <64 hex>] [--yes]
//
// --amount        what to pay, in USDC; the program credits it in MXN at the FIX in force and takes
//                 no more than is due
// --all           pay everything payable now (with late interest on a scheduled line)
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
import { LineStatus, fetchCreditLine, lineLateDue, repayInterestLeft, usdcCeilForMxn } from "../clients/js/src/index.ts";
import {
  DISCLAIMER,
  arg,
  chainNow,
  checkFlags,
  cluster,
  confirmCluster,
  fail,
  flag,
  documentHash,
  fmt,
  fmtMxn,
  loadSigner,
  parseAmount,
  required,
} from "./lib/cli.ts";
import { ataOf, loadLine, loadPool, repayInParts } from "./lib/pool.ts";

checkFlags(["cluster", "pool", "line-id", "amount", "all", "payer-keypair", "receipt-hash", "receipt-file", "receipt-salt", "yes"]);
const c = cluster();
const poolAddress = address(required("pool"));
const lineIdRaw = required("line-id");
if (!/^\d+$/.test(lineIdRaw)) fail("--line-id must be a whole number");
const lineId = BigInt(lineIdRaw);
if (flag("all") === (arg("amount") !== undefined)) fail("give either --amount or --all");

await confirmCluster(c, "repay");
const given = ["receipt-hash", "receipt-file", "receipt-salt"].some((n) => arg(n) !== undefined);
const receiptHash = given ? documentHash("receipt", "") : new Uint8Array(32);
const payer = await loadSigner(
  "payer",
  arg("payer-keypair") ?? process.env.ELOTE_BORROWER_KEYPAIR ?? ".demo-keys/borrower.json"
);
const pool = await loadPool(c, poolAddress);
const { line, data: l } = await loadLine(c, poolAddress, lineId);
if (l.status !== LineStatus.Active && l.status !== LineStatus.InCollection) fail(`the line is ${LineStatus[l.status]}: nothing to repay`);

const late = lineLateDue(l, await chainNow(c));
// What a payment can take now: on an active line, the interest accrued (decisions 46 and 47); the
// rest stays owed, or is forgiven when this payment settles a line that can draw nothing more.
const owed = l.drawn - l.principalRepaid + repayInterestLeft(l, await chainNow(c)) + late;
const owedUsdc = usdcCeilForMxn(owed, pool.fixRate);
// Late interest and the accrual grow by the second: --all offers all the interest fixed and pads
// the late interest, and the program takes only what is payable when the payment lands.
const ceiling = l.drawn - l.principalRepaid + l.interestDue - l.interestRepaid + late;
const amount = flag("all") ? usdcCeilForMxn(ceiling + late + 100n, pool.fixRate) : parseAmount("amount", required("amount"));
const balance = (await fetchToken(c.rpc, await ataOf(payer.address, pool.assetMint), { commitment: "confirmed" })).data.amount;
if (balance < (amount < owedUsdc ? amount : owedUsdc)) fail(`the payer holds ${fmt(balance)} USDC`);
console.log(
  `line ${line} (id ${lineId}): owed MXN ${fmtMxn(owed)} now${late ? `, MXN ${fmtMxn(late)} of it late interest` : ""}, ` +
    `${fmt(owedUsdc)} USDC at the FIX ${pool.fixRate}; paying up to ${fmt(amount < owedUsdc ? amount : owedUsdc)} USDC`
);

const paid = await repayInParts(c, { pool: poolAddress, poolData: pool, line, payer, amount, receiptHash });
const after = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data;
console.log(
  [
    `paid         ${fmt(paid)} USDC`,
    `line now     principal repaid MXN ${fmtMxn(after.principalRepaid)} of ${fmtMxn(after.drawn)}, interest MXN ${fmtMxn(after.interestRepaid)} of ${fmtMxn(after.interestDue)}`,
    `pagarés      ${after.pagaresPaid} of ${after.draws} marked paid and delivered (deliver-pagare.ts re-delivers a missed one)`,
    `status       ${LineStatus[after.status]}`,
  ].join("\n")
);
console.log(`\n${DISCLAIMER}`);
