// The FIX conversions the program makes (programs/elote_pool/src/fx.rs): lines and pagarés are in
// centavos of MXN, the vault in USDC base units, `fixRate` is MXN per USD times 10^4.

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

/** "18.3740" to 183,740; refuses more than four decimals. */
export function parseFix(raw: string): bigint {
  const m = /^(\d+)(?:\.(\d{1,4}))?$/.exec(raw);
  if (!m) throw new Error(`a FIX looks like 18.3740 (at most four decimals), not ${raw}`);
  const rate = BigInt(m[1]) * FIX_SCALE + BigInt((m[2] ?? "").padEnd(4, "0"));
  if (rate === 0n) throw new Error("the FIX must be positive");
  return rate;
}
