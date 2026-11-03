// The FIX conversions the program makes (programs/elote_pool/src/fx.rs): lines and pagarés are in
// centavos of MXN, the vault in USDC base units, `fixRate` is MXN per USD times 10^4.

const MXN_USDC = 100_000_000n;

/** USDC base units a draw of `mxn` centavos delivers at `fixRate`, rounded down. */
export const usdcForMxn = (mxn: bigint, fixRate: bigint): bigint => (mxn * MXN_USDC) / fixRate;

/** Centavos a payment of `usdc` base units is credited at `fixRate`, rounded down. */
export const mxnForUsdc = (usdc: bigint, fixRate: bigint): bigint => (usdc * fixRate) / MXN_USDC;

/** USDC base units a repayment takes to pay `mxn` centavos at `fixRate`, rounded up. */
export const usdcCeilForMxn = (mxn: bigint, fixRate: bigint): bigint => (mxn * MXN_USDC + fixRate - 1n) / fixRate;
