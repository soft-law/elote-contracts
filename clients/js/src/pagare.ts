// The pagaré token (decision 24): `draw` mints one Metaplex Core asset per disposición and writes a
// `Pagare` sidecar record, both at program-derived addresses seeded by the line and the draw index
// (`CreditLine.draws` at the time of the draw, as a u16 LE). Codama derives `findPagarePda` from
// `deliver_pagare`, whose `draw_index` is an argument; it derives nothing for the asset, whose
// seed in `draw` is a field of the line account, so that one is written by hand here and checked
// against the Solana CLI in `test/pagare.test.ts`.
import {
  type AccountMeta,
  AccountRole,
  type Address,
  type Instruction,
  type ProgramDerivedAddress,
  type ReadonlyUint8Array,
  type Base58EncodedBytes,
  type GetProgramAccountsApi,
  type Rpc,
  getAddressEncoder,
  getBase58Decoder,
  getProgramDerivedAddress,
  getI64Encoder,
  getPublicKeyFromAddress,
  getU16Encoder,
  getU32Encoder,
  getUtf8Encoder,
  type SignatureBytes,
  verifySignature,
} from "@solana/kit";
import { ELOTE_POOL_PROGRAM_ADDRESS, type PagareSeeds, findPagarePda, getPagareDiscriminatorBytes } from "./generated";

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
 * The most pagaré records one `repay` takes: the program's `state::MAX_SIDECARS_PER_REPAY`, and
 * it refuses more with `TooManySidecars` (`test/pagare.test.ts` reads the constant from
 * `programs/elote_pool/src/state.rs`). Three stay at least 5,000 CU under the 100,000 CU ceiling.
 * Split a payment whose `repaySidecars` returns more into several `repay` calls
 * (`repayPrincipalCap`); the amount is free.
 */
export const MAX_SIDECARS_PER_REPAY = 3;

/**
 * The pagaré records a `repay` — or a `condone` with principal (decision 51; none without) — must
 * pass as remaining accounts, `[pagaresPaid ..= lastTouched]`,
 * oldest first: from the line's cursor through the first pagaré whose `drawnThrough` exceeds the
 * line's `principalRepaid` after the payment, or the last draw when none does; none once the
 * cursor has passed the last draw or the line never drew. The program refuses any other set
 * (`PagareAccountsMismatch`). `drawnThrough[i]` is `Pagare.drawnThrough` of draw `i` for every
 * draw of the line; `principalAfter` is `principalRepaid` plus the payment's principal part (the
 * program applies late interest, then interest — `interestUnpaid` — then principal, never more
 * than is due), or plus the principal a quita condones.
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

/**
 * The most principal one `repay` may apply so that it touches at most `maxRecords` pagaré
 * records: it may cover pagarés `pagaresPaid ..= pagaresPaid + maxRecords - 2`, the next one
 * being the last record it passes — or every pagaré left, when no more than `maxRecords` are
 * (the program lets a covered record end the set when it is the last draw). A payment above the
 * cap is split into several `repay` calls. A `condone` runs through Squads, which costs more: on a
 * long schedule a quita condones one pagaré per call, up to `drawnThrough[pagaresPaid] -
 * principalRepaid` (ARCHITECTURE §10).
 */
export function repayPrincipalCap(input: {
  pagaresPaid: number;
  drawnThrough: readonly bigint[];
  principalRepaid: bigint;
  maxRecords: number;
}): bigint {
  const { pagaresPaid, drawnThrough, principalRepaid, maxRecords } = input;
  const lastCovered = pagaresPaid + maxRecords - 2;
  const ceiling =
    pagaresPaid + maxRecords < drawnThrough.length ? drawnThrough[lastCovered] : drawnThrough[drawnThrough.length - 1];
  return ceiling > principalRepaid ? ceiling - principalRepaid : 0n;
}

/** Byte offset of `procurador` in a `Pagare` account (pinned against the generated codec in the tests). */
export const PAGARE_PROCURADOR_OFFSET = 212;

/**
 * The pagaré records whose procuración in force names `procurador`. A procurador key must serve
 * one case only: a key with several pagarés lets anyone list the lines in collection from it.
 */
export async function pagaresUnderProcuracion(
  rpc: Rpc<GetProgramAccountsApi>,
  procurador: Address,
  config: { programAddress?: Address } = {}
): Promise<Address[]> {
  const discriminator = getBase58Decoder().decode(getPagareDiscriminatorBytes());
  const accounts = await rpc
    .getProgramAccounts(config.programAddress ?? ELOTE_POOL_PROGRAM_ADDRESS, {
      commitment: "confirmed",
      dataSlice: { offset: 0, length: 0 },
      filters: [
        { memcmp: { offset: 0n, bytes: discriminator as Base58EncodedBytes, encoding: "base58" } },
        { memcmp: { offset: BigInt(PAGARE_PROCURADOR_OFFSET), bytes: procurador as unknown as Base58EncodedBytes, encoding: "base58" } },
      ],
    })
    .send();
  return accounts.map((a) => a.pubkey);
}

async function sha256(...parts: ReadonlyUint8Array[]): Promise<Uint8Array> {
  const total = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    total.set(p, at);
    at += p.length;
  }
  return new Uint8Array(await crypto.subtle.digest("SHA-256", total));
}

const asBytes = (message: string | ReadonlyUint8Array) => (typeof message === "string" ? getUtf8Encoder().encode(message) : message);

/**
 * The `signed_pagare_hash` a draw records (decision 55): SHA-256 over the bytes of the message the
 * borrower signed, the full filled pagaré text (UTF-8 when given as a string).
 */
export const signedPagareHash = (message: string | ReadonlyUint8Array): Promise<Uint8Array> => sha256(asBytes(message));

/**
 * Whether `signature` (a record's `pagare_signature`) is `borrower`'s ed25519 signature over
 * `message`, and, when `signedPagareHash` (the record's) is given, whether `message` is the text
 * it commits to. The program stores both without verifying them: it never sees the text.
 */
export async function verifyPagareSignature(input: {
  message: string | ReadonlyUint8Array;
  signature: ReadonlyUint8Array;
  borrower: Address;
  signedPagareHash?: ReadonlyUint8Array;
}): Promise<boolean> {
  const bytes = asBytes(input.message);
  if (input.signature.length !== 64) return false;
  if (input.signedPagareHash) {
    const hash = await sha256(bytes);
    if (hash.length !== input.signedPagareHash.length || hash.some((b, i) => b !== input.signedPagareHash![i])) return false;
  }
  const key = await getPublicKeyFromAddress(input.borrower);
  return await verifySignature(key, input.signature as SignatureBytes, bytes);
}

/**
 * The hash the issuance (`Emision`) entry states: `sha256(pagare_hash || signed_pagare_hash)`, so the
 * registry's first entry binds both the on-chain derivation and the message the borrower signed
 * (`registry::issuance_hash`). Both preimages travel in the `PagareIssued` event.
 */
export const issuanceDocumentHash = (pagareHash: ReadonlyUint8Array, signedHash: ReadonlyUint8Array): Promise<Uint8Array> =>
  sha256(pagareHash, signedHash);

/**
 * A registry entry's hash, `sha256(asset || kind || from || to || hash || stated_at i64 LE ||
 * entry_count u32 LE)` (`registry::entry_hash`); the record's `log_root` is
 * `sha256(log_root || entry_hash)` over every entry from 32 zero bytes, the issuance entry first.
 */
export function registryEntryHash(input: {
  asset: Address;
  kind: number;
  from: Address;
  to: Address;
  hash: ReadonlyUint8Array;
  statedAt: bigint;
  entryCount: number;
}): Promise<Uint8Array> {
  const enc = getAddressEncoder();
  return sha256(
    enc.encode(input.asset),
    Uint8Array.of(input.kind),
    enc.encode(input.from),
    enc.encode(input.to),
    input.hash,
    getI64Encoder().encode(input.statedAt),
    getU32Encoder().encode(input.entryCount)
  );
}
