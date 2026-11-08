# A peso series on MEXe: parity FIX, zero code

2026-11-08. Explored and proven empirically on devnet the same day the Etherfuse rail was proven:
`elote_pool` runs a series denominated entirely in pesos — debt in MXN, asset in MXN — **with no
program change**. The whole trick is configuration: the asset is a peso token and the FIX is
pinned at parity.

## The arithmetic

Every conversion in the program is `asset = mxn × 10⁸ / fix` with `FIX_SCALE = 10_000` (fx.rs).
With `fix_rate = 10_000` (1.0000) that is exactly the centavos→6-decimals scaling: MXN 7.00 =
7_000_000 base units of a 6-decimal peso token, **no rounding in either direction**, and the FX
realization is identically zero (`cost == received` always).

## The asset

**MEXe** (Etherfuse's principal token): wraps CETES but holds par — 1 MEXe = 1 MXN; the coupon
accrues to the bond, not the holder, so the token itself is a non-yielding peso. The sandbox MEXe
(`MEXeDZ8U…`, devnet) is a classic SPL mint, 6 decimals, with issuer mint and freeze authorities —
the same issuer-risk class as Circle's USDC, recorded in `asset_controls` as usual. (Verify the
mainnet MEXe `MEXeQE5…` mint shape before a mainnet series; CETES itself is Token-2022
interest-bearing and would NOT fit this recipe.)

## The recipe (all existing instructions)

1. `init_pool` with `asset_mint` = the peso token (already on the allow-list: MEXe is in both the
   devnet and mainnet lists).
2. The council posts the first FIX at `1.0000` (`post-fix.ts --fix 1.0000`, or
   `demo-pool.ts --fix 1.0000`, added today).
3. `set_fix_bounds` with `min = max = 10_000`: nobody — feeder or council — can ever move the FIX
   off parity (`FixOutOfBounds`).

Everything else is the standard series: MXN debt, pagarés, provision, dual price, delivery. The
NAV **is pesos**; investors deposit and redeem pesos; there is no FX line in the accounting at all.

## Proven on devnet (pool `9jVpByCWYgfS6PVMJNTHbJiqZ1td6XQf2S9AXidUMCA7`)

Funded by the first end-to-end Etherfuse sandbox onramp (SPEI simulado: 500 MXN → 489.908251 MEXe
in ~70 s, order `62725f56…`): pool on sandbox MEXe, FIX posted at `10000` by the council through
Squads, deposits of 10 + 5 MEXe, a MXN 7.00 line approved and drawn as **exactly 7.000000 MEXe**,
repaid 7.20 (0.20 of interest) with the pagaré delivered in the repay transaction. After the
cycle: `fx_realized = 0`, `interest_collected = 200000` (0.20 MEXe), `outstanding = 0`.

## The two production frictions (both small, neither blocks)

1. **FIX freshness.** `approve_line` and `draw` refuse a FIX older than 5 days, so even a pinned
   FIX must be re-posted with an advancing `fix_date`. On devnet the feeder can re-post 1.0000
   freely (band 0%, once a day). **On mainnet the feeder cannot**: the RedStone MXNe reference
   (~18 MXN/USD) is enforced within 200 bps of the feeder's FIX, so parity fails
   `FixOffReference` — only the council path (which ignores the reference) can re-post. Options:
   (a) a council vote every ≤5 days (operationally heavy), or (b) a one-line program change:
   skip the freshness check — or the reference check — when `min_fix_rate == max_fix_rate` (a
   pinned FIX cannot go stale in any meaningful sense). (b) is the obvious pick when the peso
   series is scheduled; it is NOT built yet.
2. **Caps read at parity.** `max_line_bps` against a peso NAV means peso figures everywhere: a
   50% cap over a 15-peso NAV is a 7.50-peso line. Nothing to change — just remember the cap is
   measured in the series' own currency.

## Product framing (Mario, 2026-11-08)

Two investor doors, both already designed in: the Mexican investor arrives in pesos over SPEI
through the Etherfuse rail (onramp → MEXe → `deposit`); the crypto/web3 investor arrives with
USDC on-chain and deposits directly. Hence the mainnet allow-list of USDC + MEXe: a USDC series
(today's pilot, FIX-converted MXN debt) and, when scheduled, a peso series on MEXe with this
recipe — one `init_pool` per series.
