// The FIX conversions the program makes (programs/elote_pool/src/fx.rs): lines and pagarés are in
// centavos of MXN, the vault in USDC base units, `fixRate` is MXN per USD times 10^4.
import { address } from "@solana/kit";

const MXN_USDC = 100_000_000n;

/** USDC base units a draw of `mxn` centavos delivers at `fixRate`, rounded down. */
export const usdcForMxn = (mxn: bigint, fixRate: bigint): bigint => (mxn * MXN_USDC) / fixRate;

/** Centavos a payment of `usdc` base units is credited at `fixRate`, rounded down. */
export const mxnForUsdc = (usdc: bigint, fixRate: bigint): bigint => (usdc * fixRate) / MXN_USDC;

/** USDC base units a repayment takes to pay `mxn` centavos at `fixRate`, rounded up. */
export const usdcCeilForMxn = (mxn: bigint, fixRate: bigint): bigint => (mxn * MXN_USDC + fixRate - 1n) / fixRate;

/** The FIX scale: `fixRate` is MXN per USD times 10^4 (18.3740 is 183,740). */
export const FIX_SCALE = 10_000n;

/** `yyyymmdd` of the day in Mexico City (UTC-6 all year since 2022) at unix time `seconds`. */
export function mexicoCityDate(seconds: bigint): number {
  const days = Number((seconds - 6n * 3_600n - ((((seconds - 6n * 3_600n) % 86_400n) + 86_400n) % 86_400n)) / 86_400n);
  const d = new Date(days * 86_400_000);
  return d.getUTCFullYear() * 10_000 + (d.getUTCMonth() + 1) * 100 + d.getUTCDate();
}

/**
 * 00:00:00 in Mexico City of the day of unix time `seconds` (`fx::mexico_city_midnight`). Of
 * `firstDueAt` it is where a scheduled line's draw window ends, `availabilityEnd` (decision 59).
 */
export function mexicoCityMidnight(seconds: bigint): bigint {
  const local = seconds - 6n * 3_600n;
  return local - (((local % 86_400n) + 86_400n) % 86_400n) + 6n * 3_600n;
}

/** "18.3740" to 183,740; refuses more than four decimals. */
export function parseFix(raw: string): bigint {
  const m = /^(\d+)(?:\.(\d{1,4}))?$/.exec(raw);
  if (!m) throw new Error(`a FIX looks like 18.3740 (at most four decimals), not ${raw}`);
  const rate = BigInt(m[1]) * FIX_SCALE + BigInt((m[2] ?? "").padEnd(4, "0"));
  if (rate === 0n) throw new Error("the FIX must be positive");
  return rate;
}

/** RedStone's Solana price adapter, the owner of the FX reference account (`fx::REFERENCE_PROGRAM`). */
export const FX_REFERENCE_PROGRAM = address("REDSTBDUecGjwXd6YGPzHSvEUBHQqVRfCcjUVgPiHsr");
/** RedStone's MXNe price account, which `post_fix` takes as `fx_reference` (`fx::REFERENCE_ACCOUNT`). Mainnet only. */
export const FX_REFERENCE_ACCOUNT = address("8QuwResPgdQdja4gdTbvNxeUznc58XX8baUM3cCwfVW2");
/** The oldest reference the feeder publishes against, in seconds (`fx::MAX_REFERENCE_AGE_SECONDS`). */
export const MAX_REFERENCE_AGE_SECONDS = 26n * 3_600n;
/** The largest gap between the feeder's FIX and the reference, in basis points (`fx::MAX_FIX_REFERENCE_GAP_BPS`). */
export const MAX_FIX_REFERENCE_GAP_BPS = 200n;

const REFERENCE_DISCRIMINATOR = [0xe8, 0x71, 0xc1, 0xe7, 0x85, 0xd1, 0xce, 0x9a];

/**
 * The reference as the program reads it (`fx::reference_rate`): MXN per USD times 10^4, rounded
 * down, and its timestamp in seconds, from the bytes of RedStone's `PriceData` account. Throws on
 * any other feed, layout or decimals.
 */
export function referenceRate(data: Uint8Array): { rate: bigint; at: bigint } {
  const invalid = () => new Error("not RedStone's MXNe PriceData account");
  if (data.length < 98 || REFERENCE_DISCRIMINATOR.some((b, i) => data[i] !== b)) throw invalid();
  const feed = new TextDecoder().decode(data.subarray(8, 12));
  if (feed !== "MXNe" || data.subarray(12, 40).some((b) => b !== 0)) throw invalid();
  const decimalsAt = data[80] === 0 ? 89 : data[80] === 1 ? 97 : -1;
  if (decimalsAt < 0 || data[decimalsAt] !== 8) throw invalid();
  if (data.subarray(40, 64).some((b) => b !== 0)) throw invalid();
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const value = view.getBigUint64(64, false);
  if (value === 0n) throw invalid();
  const rate = 1_000_000_000_000n / value;
  if (rate === 0n) throw invalid();
  return { rate, at: view.getBigUint64(72, true) / 1_000n };
}

/** Whether `value` is within `bps` basis points of `reference` (`fx::within_bps`). */
export const withinBps = (reference: bigint, value: bigint, bps: bigint): boolean =>
  (reference > value ? reference - value : value - reference) * 10_000n <= reference * bps;
