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
import { LineStatus, fetchCreditLine, interestAccrued, lineLateDue, lineOwed, usdcCeilForMxn } from "../clients/js/src/index.ts";
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
// A written-off line still owes and takes recoveries (decision 51).
const payable = [LineStatus.Active, LineStatus.InCollection, LineStatus.WrittenOff];
if (!payable.includes(l.status)) fail(`the line is ${LineStatus[l.status]}: nothing to repay`);

const now = await chainNow(c);
const late = lineLateDue(l, now);
// What a payment takes now: late interest, the ordinary interest accrued on the principal
// outstanding and unpaid, and the principal (decision 48).
const owed = lineOwed(l, now) + late;
const owedUsdc = usdcCeilForMxn(owed, pool.fixRate);
// Late interest and the accrual grow by the second: --all quotes them an hour ahead, and the
// program takes only what is payable when the payment lands.
const ceiling = lineOwed(l, now + 3_600n) + lineLateDue(l, now + 3_600n);
const amount = flag("all") ? usdcCeilForMxn(ceiling + 100n, pool.fixRate) : parseAmount("amount", required("amount"));
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
    `line now     principal repaid MXN ${fmtMxn(after.principalRepaid)} of ${fmtMxn(after.drawn)}, interest MXN ${fmtMxn(after.interestRepaid)} of ${fmtMxn(interestAccrued(after, await chainNow(c)))} accrued`,
    `pagarés      ${after.pagaresPaid} of ${after.draws} marked paid; delivered once the line owes nothing (decision 49; deliver-pagare.ts re-delivers a missed one)`,
    `status       ${LineStatus[after.status]}`,
  ].join("\n")
);
console.log(`\n${DISCLAIMER}`);
