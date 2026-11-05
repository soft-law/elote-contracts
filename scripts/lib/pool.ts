// Reads of the pool, its lines and their pagaré records, and the repayment split the program's
// compute ceiling asks of clients (at most `MAX_SIDECARS_PER_REPAY` records per `repay`).
import { type Address, type KeyPairSigner } from "@solana/kit";
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
  getRepayInstructionAsync,
  fetchPool,
  lineLateDue,
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

/**
 * Pays up to `amount` on the line, in as many `repay` calls as it takes for each to touch at most
 * `MAX_SIDECARS_PER_REPAY` pagaré records. Each part re-reads the line, so another payment landing
 * in between only changes the next part; a part refused because the records it was built for
 * changed (`PagareAccountsMismatch`) is rebuilt and sent again, once. Returns what the payer's
 * account was debited.
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
  for (let part = 1; left > 0n; part++) {
    let sent = false;
    for (let attempt = 1; !sent; attempt++) {
      const l = (await fetchCreditLine(c.rpc, line, { commitment: "confirmed" })).data;
      const fix = (await fetchPool(c.rpc, pool, { commitment: "confirmed" })).data;
      const leftMxn = mxnForUsdc(left, fix.fixRate);
      if (l.status !== LineStatus.Active && l.status !== LineStatus.InCollection) return start - (await balance());
      // Late interest: a scheduled line's, or a bullet line's from term_end + grace (or the mark).
      // Late interest grows by the second until the part lands: quote it 30 s ahead. The program
      // takes only what is due; the excess falls on principal, and if that moves the records the
      // part touches, the retry below rebuilds it.
      const late = lineLateDue(l, BigInt(poolData.gracePeriodSeconds), (await chainNow(c)) + 30n);
      const interestLeft = l.interestDue - l.interestRepaid;
      const principalLeft = l.drawn - l.principalRepaid;
      const drawnThrough = await drawnThroughOf(c, line, l.draws);
      const cap = drawnThrough.length
        ? repayPrincipalCap({ pagaresPaid: l.pagaresPaid, drawnThrough, principalRepaid: l.principalRepaid, maxRecords: MAX_SIDECARS_PER_REPAY })
        : principalLeft;
      const afterInterest = leftMxn > late + interestLeft ? leftMxn - late - interestLeft : 0n;
      const principal = [afterInterest, principalLeft, cap].reduce((a, b) => (a < b ? a : b));
      const interest = leftMxn > late ? (leftMxn - late < interestLeft ? leftMxn - late : interestLeft) : 0n;
      const piece = late + interest + principal;
      if (piece === 0n) return start - (await balance());
      const records = await repaySidecars({ line, pagaresPaid: l.pagaresPaid, drawnThrough, principalAfter: l.principalRepaid + principal });
      const label = `repay part ${part}: MXN ${fmtMxn(piece)} (${records.length} pagaré record${records.length === 1 ? "" : "s"})`;
      try {
        await send(c, label, payer, [
          withRemainingAccounts(
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
          ),
        ]);
        sent = true;
      } catch (e) {
        if (!(e instanceof SendError) || attempt >= 2 || !/not the ones this payment touches/.test(e.message)) throw e;
        console.log(`  ${e.message}\n  the line changed meanwhile: rebuilding the part`);
      }
    }
    const leftAfter = args.amount - (start - (await balance()));
    if (leftAfter >= left) fail("a repay part debited nothing; stopping");
    left = leftAfter;
  }
  return start - (await balance());
}
