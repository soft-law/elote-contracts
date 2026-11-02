// The pagaré token (decision 24): `draw` mints one Metaplex Core asset per disposición and writes a
// `Pagare` sidecar record, both at program-derived addresses seeded by the line and the draw index
// (`CreditLine.draws` at the time of the draw, as a u16 LE). Codama derives `findPagarePda` from
// `cancel_pagare`, whose `draw_index` is an argument; it derives nothing for the asset, whose
// seed in `draw` is a field of the line account, so that one is written by hand here and checked
// against the Solana CLI in `test/pagare.test.ts`.
import {
  type AccountMeta,
  AccountRole,
  type Address,
  type Instruction,
  type ProgramDerivedAddress,
  type ReadonlyUint8Array,
  getAddressEncoder,
  getProgramDerivedAddress,
  getU16Encoder,
  getUtf8Encoder,
} from "@solana/kit";
import { ELOTE_POOL_PROGRAM_ADDRESS, type PagareSeeds, findPagarePda } from "./generated";

/** The Metaplex Core program the pool mints pagarés through (pinned in the program). */
export const MPL_CORE_PROGRAM_ADDRESS = "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d" as Address<
  "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d"
>;

/** The Metaplex Core asset that is the pagaré of a draw: `["pagare_asset", line, draw_index u16 LE]`. */
export async function findPagareAssetPda(
  seeds: PagareSeeds,
  config: { programAddress?: Address } = {}
): Promise<ProgramDerivedAddress> {
  const line = typeof seeds.line === "string" ? seeds.line : seeds.line.address;
  return await getProgramDerivedAddress({
    programAddress: config.programAddress ?? ELOTE_POOL_PROGRAM_ADDRESS,
    seeds: [
      getUtf8Encoder().encode("pagare_asset"),
      getAddressEncoder().encode(line),
      getU16Encoder().encode(seeds.drawIndex),
    ],
  });
}

/**
 * The pagaré accounts and arguments a `draw` instruction needs, to spread into
 * `getDrawInstructionAsync`'s input: the asset and the sidecar of the next draw (`drawIndex` is
 * `CreditLine.draws` as fetched before the draw) with their bumps (the program verifies each with
 * one `create_program_address`, so no bump search runs on chain), the pool's collection
 * (`Pool.collection`) and the Core program.
 */
export async function drawPagareAccounts(
  input: PagareSeeds & { collection: Address },
  config: { programAddress?: Address } = {}
): Promise<{
  asset: Address;
  assetBump: number;
  pagare: Address;
  pagareBump: number;
  collection: Address;
  mplCoreProgram: Address;
}> {
  const { line, drawIndex, collection } = input;
  const [asset, assetBump] = await findPagareAssetPda({ line, drawIndex }, config);
  const [pagare, pagareBump] = await findPagarePda({ line, drawIndex }, config);
  return { asset, assetBump, pagare, pagareBump, collection, mplCoreProgram: MPL_CORE_PROGRAM_ADDRESS };
}

/**
 * The payment cursor of a line (step 6d), `CreditLine.pagares_paid`: bytes 30..32 of `reserved`,
 * little endian; the index of the oldest pagaré not yet marked paid by a repayment.
 */
export function pagaresPaidOf(reserved: ReadonlyUint8Array | Uint8Array | readonly number[]): number {
  return reserved[30] | (reserved[31] << 8);
}

/**
 * The pagaré records a `repay` must pass as remaining accounts, `[pagaresPaid ..= lastTouched]`,
 * oldest first: from the line's cursor through the first pagaré whose `drawnThrough` exceeds the
 * line's `principalRepaid` after the payment, or the last draw when none does; none once the
 * cursor has passed the last draw or the line never drew. The program refuses any other set
 * (`PagareAccountsMismatch`). `drawnThrough[i]` is `Pagare.drawnThrough` of draw `i` for every
 * draw of the line; `principalAfter` is `principalRepaid` plus the payment's principal part (the
 * program applies late interest, then interest, then principal, never more than is due).
 *
 * The set depends on the line at the time the transaction is built: a payment by anyone that lands
 * first can change it, and the program then refuses this one with `PagareAccountsMismatch` while
 * nothing moves. Re-read the line and the cursor, recompute the set and retry.
 */
export async function repaySidecars(
  input: { line: Address; pagaresPaid: number; drawnThrough: readonly bigint[]; principalAfter: bigint },
  config: { programAddress?: Address } = {}
): Promise<Address[]> {
  const { line, pagaresPaid, drawnThrough, principalAfter } = input;
  const out: Address[] = [];
  for (let drawIndex = pagaresPaid; drawIndex < drawnThrough.length; drawIndex++) {
    out.push((await findPagarePda({ line, drawIndex }, config))[0]);
    if (drawnThrough[drawIndex] > principalAfter) break;
  }
  return out;
}

/** The instruction with `addresses` appended as writable, non-signer remaining accounts. */
export function withRemainingAccounts<T extends Instruction>(instruction: T, addresses: readonly Address[]): T {
  const extra: AccountMeta[] = addresses.map((address) => ({ address, role: AccountRole.WRITABLE }));
  return { ...instruction, accounts: [...(instruction.accounts ?? []), ...extra] } as T;
}
