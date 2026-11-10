// Reads of the pool, its lines and their pagaré records, the repayment split the program's
// compute ceiling asks of clients (at most `MAX_SIDECARS_PER_REPAY` records per `repay`), and the
// delivery of the títulos once the line owes nothing (decision 49).
import {
  type Address,
  type Blockhash,
  type Instruction,
  type KeyPairSigner,
  appendTransactionMessageInstructions,
  createTransactionMessage,
  isTransactionMessageWithinSizeLimit,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
} from "@solana/kit";
import { TOKEN_PROGRAM_ADDRESS, fetchToken, findAssociatedTokenPda } from "@solana-program/token";
import {
  type CreditLine,
  type Pool,
  LineStatus,
  MAX_SIDECARS_PER_REPAY,
  fetchCreditLine,
  fetchMaybePool,
  fetchPagare,
  findLinePda,
  findPagarePda,
  findPoolPda,
  getDeliverPagareInstructionAsync,
  getRepayInstructionAsync,
  fetchPool,
  interestUnpaid,
  lineLateDue,
  lineOwed,
  lineSponsor,
  mxnForUsdc,
  usdcCeilForMxn,
  repayPrincipalCap,
  repaySidecars,
  withRemainingAccounts,
} from "../../clients/js/src/index.ts";
import { type Cluster, SendError, chainNow, fail, fmtMxn, send } from "./cli.ts";

/** The pool at `address`, checked to be an elote_pool pool at its own PDA. */
export async function loadPool(c: Cluster, address: Address): Promise<Pool> {
  const maybe = await fetchMaybePool(c.rpc, address, { commitment: "confirmed" });
  if (!maybe.exists) fail(`no elote_pool pool at ${address} on ${c.name}`);
  const [derived] = await findPoolPda({ creator: maybe.data.creator, assetMint: maybe.data.assetMint });
  if (derived !== address) fail("the account is not a pool at its expected address");
  return maybe.data;
}

export async function loadLine(c: Cluster, pool: Address, lineId: bigint): Promise<{ line: Address; data: CreditLine }> {
  const [line] = await findLinePda({ pool, lineId });
  const data = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data;
  return { line, data };
}

/** `Pagare.drawnThrough` of every draw of the line, oldest first. */
export async function drawnThroughOf(c: Cluster, line: Address, draws: number): Promise<bigint[]> {
  const out: bigint[] = [];
  for (let drawIndex = 0; drawIndex < draws; drawIndex++) {
    const [pagare] = await findPagarePda({ line, drawIndex });
    out.push((await fetchPagare(c.rpc, pagare, { commitment: "confirmed" })).data.drawnThrough);
  }
  return out;
}

export const ataOf = async (owner: Address, mint: Address) =>
  (await findAssociatedTokenPda({ owner, mint, tokenProgram: TOKEN_PROGRAM_ADDRESS }))[0];

const PAGARE_PAID = 1; // state::PAGARE_*
const PAGARE_FLAG_DELIVERED = 1 << 2; // state::PAGARE_FLAG_*
const PAGARE_FLAG_PAID_BY_AVAL = 1 << 3;
const SPONSOR_GUARANTOR = 1; // state::SPONSOR_*

/** Whether `instructions` fit one transaction of the shape `send` builds, paid by `feePayer`. */
export function fitsOneTransaction(feePayer: KeyPairSigner, instructions: Instruction[]): boolean {
  const message = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(feePayer, m),
    // The size does not depend on which blockhash: any 32 bytes stand in for the real one.
    (m) => setTransactionMessageLifetimeUsingBlockhash({ blockhash: "11111111111111111111111111111111" as Blockhash, lastValidBlockHeight: 0n }, m),
    (m) => appendTransactionMessageInstructions(instructions, m)
  );
  return isTransactionMessageWithinSizeLimit(message);
}

/**
 * What the line owes at `now` in principal and the ordinary interest accrued on it (decision 48):
 * no título leaves until it is zero (decision 49).
 */
export const lineOwes = (l: CreditLine, now: bigint) => lineOwed(l, now);

/**
 * A `deliver_pagare` for every pagaré of the line not yet delivered, oldest first: those already
 * marked paid go to whoever their mark names (the aval when `PAID_BY_AVAL`, else the borrower);
 * the others, which the settling payment is about to mark, to its payer's side (`byAval`).
 */
async function pendingDeliveries(
  c: Cluster,
  args: { pool: Address; poolData: Pool; line: Address; l: CreditLine; payer: KeyPairSigner; byAval: boolean }
): Promise<{ drawIndex: number; ix: Instruction }[]> {
  const { pool, poolData, line, l, payer, byAval } = args;
  const sponsor = lineSponsor(l) ?? undefined;
  const out = [];
  for (let drawIndex = 0; drawIndex < l.draws; drawIndex++) {
    const [pagare] = await findPagarePda({ line, drawIndex });
    const record = (await fetchPagare(c.rpc, pagare, { commitment: "confirmed" })).data;
    if ((record.flags & PAGARE_FLAG_DELIVERED) !== 0) continue;
    const toAval = record.status === PAGARE_PAID ? (record.flags & PAGARE_FLAG_PAID_BY_AVAL) !== 0 : byAval;
    const recipient = toAval ? (sponsor ?? fail("the aval paid but the line names no sponsor")) : l.borrower;
    const ix = await getDeliverPagareInstructionAsync({
      payer,
      pool,
      line,
      pagare,
      asset: record.asset,
      collection: poolData.collection,
      recipient,
      drawIndex,
    });
    out.push({ drawIndex, ix });
  }
  return out;
}

/**
 * Once the line owes nothing, delivers every paid título not yet delivered (decision 49), in as
 * many transactions as their size takes; does nothing while the line owes anything. Returns how
 * many were delivered.
 */
export async function deliverPending(c: Cluster, args: { pool: Address; poolData: Pool; line: Address; payer: KeyPairSigner }): Promise<number> {
  const l = (await fetchCreditLine(c.rpc, args.line, { commitment: "confirmed" })).data;
  if (lineOwes(l, await chainNow(c)) > 0n) return 0;
  const pending = await pendingDeliveries(c, { ...args, l, byAval: false });
  let delivered = 0;
  while (pending.length) {
    let n = 1;
    while (n < pending.length && fitsOneTransaction(args.payer, pending.slice(0, n + 1).map((d) => d.ix))) n++;
    const batch = pending.splice(0, n);
    await send(c, `deliver_pagare: pagaré${n === 1 ? "" : "s"} ${batch.map((d) => d.drawIndex).join(", ")} delivered`, args.payer, batch.map((d) => d.ix));
    delivered += n;
  }
  return delivered;
}

/**
 * Pays up to `amount` on the line, in as many `repay` calls as it takes for each to touch at most
 * `MAX_SIDECARS_PER_REPAY` pagaré records. Each part re-reads the line, so another payment landing
 * in between only changes the next part; a part refused because the records it was built for
 * changed (`PagareAccountsMismatch`) is rebuilt and sent again, once. A título leaves only once the
 * line owes nothing (decision 49): the part expected to settle the line carries the deliveries of
 * every título not yet delivered, as many as fit its transaction (if the line would still owe, the
 * program refuses them and the part is sent again without them), and the rest follow in
 * transactions of their own. Returns what the payer's account was debited.
 */
export async function repayInParts(
  c: Cluster,
  args: { pool: Address; poolData: Pool; line: Address; payer: KeyPairSigner; amount: bigint; receiptHash: Uint8Array }
): Promise<bigint> {
  const { pool, poolData, line, payer, receiptHash } = args;
  const payerAsset = await ataOf(payer.address, poolData.assetMint);
  const balance = async () => (await fetchToken(c.rpc, payerAsset, { commitment: "confirmed" })).data.amount;
  const start = await balance();
  // `amount` and `left` are USDC; the line counts MXN at the FIX in force, read for each part.
  let left = args.amount;
  parts: for (let part = 1; left > 0n; part++) {
    let sent = false;
    let withDeliveries = true;
    for (let attempt = 1; !sent; attempt++) {
      const l = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data;
      const fix = (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data;
      const leftMxn = mxnForUsdc(left, fix.fixRate);
      if (l.status !== LineStatus.Active && l.status !== LineStatus.InCollection) break parts;
      // Late interest: a scheduled line's, or a bullet line's from term_end (or the mark).
      // Late interest grows by the second until the part lands: quote it 30 s ahead. The program
      // takes only what is due; the excess falls on principal, and if that moves the records the
      // part touches, the retry below rebuilds it.
      const now = await chainNow(c);
      const quotedAt = now + 30n;
      const late = lineLateDue(l, quotedAt);
      // The interest the program takes before principal: the interest accrued and unpaid, quoted
      // ahead like the late interest (decision 48).
      const interestLeft = interestUnpaid(l, quotedAt);
      const principalLeft = l.drawn - l.principalRepaid;
      // A part that left no principal left no interest either: nothing accrues on principal
      // repaid, so there is nothing more to pay (decision 48).
      if (part > 1 && l.status === LineStatus.Active && principalLeft === 0n) break parts;
      const drawnThrough = await drawnThroughOf(c, line, l.draws);
      const cap = drawnThrough.length
        ? repayPrincipalCap({ pagaresPaid: l.pagaresPaid, drawnThrough, principalRepaid: l.principalRepaid, maxRecords: MAX_SIDECARS_PER_REPAY })
        : principalLeft;
      const afterInterest = leftMxn > late + interestLeft ? leftMxn - late - interestLeft : 0n;
      const principal = [afterInterest, principalLeft, cap].reduce((a, b) => (a < b ? a : b));
      const interest = leftMxn > late ? (leftMxn - late < interestLeft ? leftMxn - late : interestLeft) : 0n;
      const piece = late + interest + principal;
      if (piece === 0n) break parts;
      const records = await repaySidecars({ line, pagaresPaid: l.pagaresPaid, drawnThrough, principalAfter: l.principalRepaid + principal });
      const repayIx = withRemainingAccounts(
        await getRepayInstructionAsync({
          payer,
          pool,
          line,
          assetMint: poolData.assetMint,
          shareMint: poolData.shareMint,
          vault: poolData.vault,
          payerAssetAccount: payerAsset,
          tokenProgram: TOKEN_PROGRAM_ADDRESS,
          maxUsdc: usdcCeilForMxn(piece, fix.fixRate),
          fixRate: fix.fixRate,
          fixDate: fix.fixDate,
          receiptHash,
        }),
        records
      );
      // Does this part leave the line owing nothing? All the principal and the interest accrued
      // (decision 48); the program takes no more than it accrued by the time the part lands.
      const settles = principal === principalLeft && interest === interestLeft;
      // The título goes to the borrower, or to the aval when the aval's payment covers it.
      const byAval = lineSponsor(l) !== null && payer.address === l.sponsor && l.sponsorRole === SPONSOR_GUARANTOR;
      const deliveries: { drawIndex: number; ix: Instruction }[] = [];
      if (settles && withDeliveries) {
        for (const d of await pendingDeliveries(c, { pool, poolData, line, l, payer, byAval })) {
          if (!fitsOneTransaction(payer, [repayIx, ...deliveries.map((x) => x.ix), d.ix])) break;
          deliveries.push(d);
        }
      }
      const label = `repay part ${part}: MXN ${fmtMxn(piece)} (${records.length} pagaré record${records.length === 1 ? "" : "s"}${deliveries.length ? `, ${deliveries.length} delivered` : ""})`;
      try {
        await send(c, label, payer, [repayIx, ...deliveries.map((d) => d.ix)]);
        sent = true;
      } catch (e) {
        const mismatch = e instanceof SendError && /not the ones this payment touches/.test(e.message);
        const owes = e instanceof SendError && deliveries.length > 0 && /until it owes nothing/.test(e.message);
        if (attempt >= 2 || !(mismatch || owes)) throw e;
        if (owes) withDeliveries = false;
        console.log(`  ${(e as Error).message}\n  ${owes ? "the line would still owe after this part: sending it without the deliveries" : "the line changed meanwhile: rebuilding the part"}`);
      }
    }
    const leftAfter = args.amount - (start - (await balance()));
    if (leftAfter >= left) fail("a repay part debited nothing; stopping");
    left = leftAfter;
  }
  // The títulos the settling part had no room for, or all of them if it was sent without them.
  await deliverPending(c, { pool, poolData, line, payer });
  return start - (await balance());
}
