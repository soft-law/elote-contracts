// The pagaré token (decision 24): `draw` mints one Metaplex Core asset per disposición and writes a
// `Pagare` sidecar record, both at program-derived addresses seeded by the line and the draw index
// (`CreditLine.draws` at the time of the draw, as a u16 LE). Codama derives `findPagarePda` from
// `cancel_pagare`, whose `draw_index` is an argument; it derives nothing for the asset, whose
// seed in `draw` is a field of the line account, so that one is written by hand here and checked
// against the Solana CLI in `test/pagare.test.ts`.
import {
  type Address,
  type ProgramDerivedAddress,
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

