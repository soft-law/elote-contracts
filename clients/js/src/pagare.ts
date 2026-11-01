// The pagaré token (decision 24): `draw` mints one Metaplex Core asset per disposición and writes a
// `Pagare` sidecar record, both at program-derived addresses seeded by the line and the draw index
// (`CreditLine.draws` at the time of the draw, as a u16 LE). Codama derives no helper for seeds
// that come from another account's field, so these are written by hand and checked against the
// Solana CLI in `test/pagare.test.ts`.
import {
  type Address,
  type ProgramDerivedAddress,
  getAddressEncoder,
  getProgramDerivedAddress,
  getU16Encoder,
  getUtf8Encoder,
} from "@solana/kit";
import { ELOTE_POOL_PROGRAM_ADDRESS } from "./generated";

/** The Metaplex Core program the pool mints pagarés through (pinned in the program). */
export const MPL_CORE_PROGRAM_ADDRESS = "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d" as Address<
  "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d"
>;

export type PagareSeeds = {
  /** The `CreditLine` address. */
  line: Address;
  /** The draw's index: `CreditLine.draws` before the draw. */
  drawIndex: number;
};

async function pagareAddress(prefix: string, seeds: PagareSeeds, programAddress?: Address) {
  return await getProgramDerivedAddress({
    programAddress: programAddress ?? ELOTE_POOL_PROGRAM_ADDRESS,
    seeds: [
      getUtf8Encoder().encode(prefix),
      getAddressEncoder().encode(seeds.line),
      getU16Encoder().encode(seeds.drawIndex),
    ],
  });
}

/** The `Pagare` sidecar record of a draw: `["pagare", line, draw_index u16 LE]`. */
export async function findPagarePda(
  seeds: PagareSeeds,
  config: { programAddress?: Address } = {}
): Promise<ProgramDerivedAddress> {
  return await pagareAddress("pagare", seeds, config.programAddress);
}

/** The Metaplex Core asset that is the pagaré of a draw: `["pagare_asset", line, draw_index u16 LE]`. */
export async function findPagareAssetPda(
  seeds: PagareSeeds,
  config: { programAddress?: Address } = {}
): Promise<ProgramDerivedAddress> {
  return await pagareAddress("pagare_asset", seeds, config.programAddress);
}

/**
 * The pagaré accounts a `draw` instruction needs, for `getDrawInstructionAsync`: the asset and
 * the sidecar of the next draw (`drawIndex` is `CreditLine.draws` as fetched before the draw),
 * the pool's collection (`Pool.collection`) and the Core program.
 */
export async function drawPagareAccounts(
  input: PagareSeeds & { collection: Address },
  config: { programAddress?: Address } = {}
): Promise<{ asset: Address; pagare: Address; collection: Address; mplCoreProgram: Address }> {
  const { line, drawIndex, collection } = input;
  return {
    asset: (await findPagareAssetPda({ line, drawIndex }, config))[0],
    pagare: (await findPagarePda({ line, drawIndex }, config))[0],
    collection,
    mplCoreProgram: MPL_CORE_PROGRAM_ADDRESS,
  };
}
