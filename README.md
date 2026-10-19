# elote-contracts

**Mexican loans on Solana.** The `elote_pool` program behind [elote.baby](https://elote.baby):
signed legal contracts become programmable loan contracts on Solana that carry out the repayment
terms, and depositors share the interest that borrowers repay.

> Demo on Solana devnet with test assets. Not an offer of securities.

This is the production repository for the program, its TypeScript client
(`@softlaw/elote-pool-client`, in [`clients/js`](clients/js)) and the demo scripts. The app lives in
`soft-law/elote-web`, the documentation in `soft-law/elote-docs`, and the agreements server in
`soft-law/softlaw-backend`.

Hackathon snapshot: https://github.com/soft-law/elote-baby/tree/colosseum-2026 (program, app,
agreements server and the Meteora launch as submitted).

## The `elote_pool` program

`elote_pool` is a Solana program (Anchor 1.1.2) for a liquidity pool with a share token.
Investors deposit the pool asset and receive shares. The pool admin approves credit lines for
entrepreneurs from the pool. When borrowers repay, the interest stays in the pool and every share
is worth more. A recognised loss lowers the value of every share once.

The demo asset is **tMXN (devnet test token, no value)**: a plain SPL token created by the demo
script. Nothing here holds or moves real money.

Documentation for investors, borrowers and developers (how the pool works, credit lines, the app,
the agreements server, program reference and addresses) lives in `soft-law/elote-docs`.

## What it is

- **Shares that appreciate.** Accounting follows ERC-4626:
  `shares = assets * supply / NAV` on deposit, `assets = shares * NAV / supply` on redemption,
  with `NAV = vault balance + outstanding principal`. Both legs round down, in favour of the pool.
  Interest counts when it is collected; nothing accrues and there is no oracle.
- **Dead shares.** The first deposit (at least 1.0) locks 1,000 of its shares in a share token
  account owned by the pool, which nothing can move. The supply never falls below that floor, so a
  donation to the vault cannot round later deposits down to nothing. `deposit` and `redeem` also
  take a minimum the caller accepts (`min_shares_out`, `min_assets_out`).
- **Credit lines from the pool.** Each line records the borrower, the principal, a fixed rate,
  the term and the hash of the signed credit agreement. A single-maturity line takes repayments
  to principal first, then to interest. Anyone may repay on the borrower's behalf; the payer is
  recorded in the event.
- **Installments, arrears and late interest.** A line can instead be repaid in installments
  (see [Installments and collection](#installments-and-collection)). Missed installments are
  computed from what was repaid, late interest accrues on overdue amounts after the pool's grace
  period, and a line with three missed installments goes to collection.
- **Optional sponsor.** A public support program can be named on a line; it must co-sign the opening and
  may pay the interest. It never touches the pool's money otherwise.
- **Pause.** The admin can pause deposits, redemptions, openings and draws. Repayments, moving a line
  to collection, writing off an overdue line and unpausing are never blocked.

## Accounts

| Account | Address | Holds |
|---|---|---|
| `Pool` | PDA `["pool", admin, asset_mint]` | admin, asset, vault, share token, outstanding principal, running totals, pause flag |
| share token | PDA `["shares", pool]` | an SPL token with the asset's decimals; only the pool PDA can issue shares |
| vault | PDA token account `["vault", pool]`, owned by the pool PDA | the pool's liquid assets |
| dead shares | PDA token account `["dead", pool]`, owned by the pool PDA | the 1,000 shares locked by the first deposit |
| `CreditLine` | PDA `["line", pool, line_id (u64 LE)]` | terms, agreement hash, repayments, status, installment schedule (version 2) |

Both accounts keep 32 `reserved` bytes; new parameters live there so the layouts do not change.
`clients/js/src/schedule.ts` reads them and computes arrears exactly as the program does.

| Account | Bytes of `reserved` | Field |
|---|---|---|
| `Pool` | 0..4 | `grace_period_seconds`, u32 LE (zero: no grace); 4..32 zero |
| `CreditLine` v2 | 0..8 | `interval_seconds`, i64 LE |
| | 8..10 | `installments`, u16 LE |
| | 10..12 | `late_bps`, u16 LE, annual |
| | 12 | `max_missed`, u8 |
| | 13 | `flags`, u8, zero |
| | 14..22 | `late_collected`, u64 LE |
| | 22..30 | `late_paid_until`, i64 LE: late interest is settled up to this time |
| | 30..32 | zero |

A version-1 line (single maturity) has all 32 bytes at zero.

## Instructions

| Instruction | Signers | Effect |
|---|---|---|
| `init_pool(grace_period_seconds)` | admin | creates the pool, its share token, its vault and its dead-shares account; the grace period is fixed for the life of the pool |
| `deposit(assets, min_shares_out)` | depositor | assets in, shares out at the current NAV; the first deposit is at least 1.0 and locks 1,000 dead shares |
| `redeem(shares, min_assets_out)` | holder | shares burned, assets out at the current NAV, up to the vault's liquidity |
| `open_line(line_id, principal, rate_bps, term_end, agreement_hash, sponsor, schedule)` | admin, borrower, sponsor if named | approves the next line; moves no money; `schedule` (optional) makes it an installment line |
| `draw` | borrower | the whole principal goes from the vault to the borrower, before the term ends (and, for an installment line, early enough for the last installment to fall due within the term) |
| `repay(amount)` | anyone | single maturity: principal first, then interest. Installments: late interest, then interest, then principal, never more than is due. The line closes when nothing remains. Never paused |
| `mark_in_collection` | anyone | moves an installment line to `InCollection` once `max_missed` installments are missed and the grace period of the last of them has run. Never paused |
| `write_off` | admin | after the term plus grace, or for a line in collection after its oldest missed due date plus grace, the unpaid principal is recognised as a loss |
| `set_paused(paused)` | admin | pause switch for deposits, redemptions, openings and draws |

Error codes start at 6000 in the order of `PoolError` in `programs/elote_pool/src/errors.rs`.

## Installments and collection

`open_line` takes an optional `ScheduleParams { interval_seconds, installments, late_bps, max_missed }`.
Without it the line is a single-maturity line (version 1) and nothing below applies.

- **Schedule.** Installment `k` falls due at `drawn_at + k * interval_seconds`. Each installment is
  `(principal + interest_due) / installments`, rounded down; the last one carries the remainder, so they
  add up exactly. Limits: 1 to 120 installments, a positive interval, `late_bps <= 10_000`,
  `3 <= max_missed <= installments` (the owner's policy is three months of arrears before collection), at least one unit per installment, and the last due date no later
  than `term_end` (checked at opening and again at the draw).
- **Missed installments** are computed, never stored: installments fallen due minus installments fully
  covered by the principal and interest repaid. At its due second an installment already counts as due.
  The pool's grace period does not change the count.
- **Late interest** accrues on each overdue part from its due date plus the pool's grace period (or
  from the last repayment, whichever is later), linear per second at `late_bps` a year (365 days):
  `Σ part * min(seconds, cap) * late_bps / (10_000 * 31_536_000)`, rounded down, in `u128`, where
  `cap = ceil(10_000 * 31_536_000 / late_bps)` seconds. Two caps apply:
  - **per settlement:** within one repayment, each overdue part accrues at most its own amount. Each
    repayment settles the late interest in full and records `late_paid_until`, so no period is charged
    twice; but a part that stays unpaid accrues again after the next repayment, so over several
    repayments the late interest on one part can exceed that part.
  - **over the life of the line:** late interest collected never exceeds the line's principal
    (`late_due <= principal - late_collected`). A borrower who pays only late interest reaches this cap
    and from then on every payment goes to interest and principal.

  Late interest goes to the vault and raises the NAV like interest; it is counted in the line's
  `late_collected`, not in the pool's `interest_collected`.
- **Repayment order** on an installment line: late interest, then interest, then principal.
- **Collection.** Once `max_missed` installments are missed (at least three, three months of arrears)
  and the grace period of the last of them has run (`now > drawn_at + (covered + max_missed) * interval
  + grace`, with `covered` installments paid), anyone may call `mark_in_collection`, which moves the line
  to `InCollection`. Collection does not accelerate the balance: the remaining installments keep their
  due dates. Repayments are
  still accepted there; a full repayment moves it to `Repaid`. A payment that cures the arrears leaves the
  status as it is, but then nothing is missed and the write-off below is not allowed.
- **Write-off policy.** After the third month in arrears the line goes to out-of-court collection and the
  admin records the write-off. The program allows `write_off` only when the line is in collection and its
  oldest missed installment fell due more than the grace period ago, or when the term ended more than the
  grace period ago. Only the admin may call it.

## Events

Every instruction emits its events through `emit_cpi!`. Events are append-only: existing events never
change fields; new information comes as new events.

| Event | Emitted by | Fields |
|---|---|---|
| `PoolInitialized` | `init_pool` | pool, asset_mint, share_mint, admin |
| `PoolConfigured` | `init_pool`, after `PoolInitialized` | pool, grace_period_seconds |
| `Deposited` | `deposit` | pool, depositor, assets, shares, nav_after, supply_after |
| `Redeemed` | `redeem` | pool, holder, shares, assets, nav_after, supply_after |
| `LineOpened` | `open_line` | pool, line_id, borrower, sponsor, agreement_hash, principal, rate_bps, interest_due, term_end |
| `LineScheduled` | `open_line` with a schedule, after `LineOpened` | pool, line_id, interval_seconds, installments, late_bps, max_missed |
| `LineDrawn` | `draw` | pool, line_id, amount, outstanding_after |
| `LateInterestCollected` | `repay` with late interest, before `LineRepaid` | pool, line_id, amount |
| `LineRepaid` | `repay` | pool, line_id, payer, principal_part, interest_part, remaining (without late interest), nav_after |
| `LineInCollection` | `mark_in_collection` | pool, line_id, missed, remaining |
| `LineWrittenOff` | `write_off` | pool, line_id, loss, nav_after |
| `PoolPaused` | `set_paused` | pool, paused |

## Build and test

Toolchain: Rust 1.89.0 (`rust-toolchain.toml`), Anchor CLI 1.1.2, Solana CLI 3.1.x, Node 20.18+.

```sh
NO_DNA=1 anchor build            # target/deploy/elote_pool.so and target/idl/elote_pool.json
cargo test -p elote_pool         # unit tests + LiteSVM integration tests against the built .so
npm install
npm run codama                   # copies the IDL to idl/ and regenerates clients/js/src/generated
npm run check:client             # fails if the committed client differs from idl/elote_pool.json
npm run typecheck
```

The integration tests run the compiled program in LiteSVM 0.9.0, which loads SPL Token and the
Associated Token Account program by default.

## Demo

```sh
npx tsx scripts/demo-pool.ts --cluster local  [--agreement-hash <64 hex>] [--sponsor] [--schedule]
npx tsx scripts/demo-pool.ts --cluster devnet [--agreement-hash <64 hex>] [--sponsor]
```

All amounts below are in tMXN (devnet test token, no value). The script creates the test token
and a pool, then runs two investors (10,000 and 5,000), a 6,000 line at 12% with the given
agreement hash, the draw, the borrower's repayment of principal and the sponsor's payment of the
720 interest, then a partial redemption. It prints every signature with an explorer link and a final table of NAV, supply and
value per share before and after.

With `--schedule` it then opens a 600 line at 12% in six installments with 100% a year late interest
and collection after three missed installments, draws it, lets the first installment go unpaid past its
due date and the grace period, and repays it with the next one: the repayment collects the late interest
first, then interest, then principal, and no installment remains missed. A validator's clock cannot be
moved forward, so on `--cluster local` an installment falls due every 20 seconds (a compressed demo
month, `--interval-seconds` to change it) and the pool's grace period is 5 seconds (`--grace-seconds`).
Over a few seconds the late interest is a few millionths of a tMXN; the integration tests move the clock
by months instead.

Wallets are generated into `.demo-keys/` (ignored by git) unless `ELOTE_ADMIN_KEYPAIR`,
`ELOTE_INVESTOR_A_KEYPAIR`, `ELOTE_INVESTOR_B_KEYPAIR`, `ELOTE_BORROWER_KEYPAIR` or
`ELOTE_SPONSOR_KEYPAIR` point to keypair files. `ELOTE_RPC` overrides the RPC URL. On devnet the
admin is funded by airdrop (with retries) and pays the other wallets' fees.

For a local run, start a validator with the program loaded and the demo admin funded (the script
creates `.demo-keys/admin.json` on its first run; create it beforehand for the validator):

```sh
mkdir -p .demo-keys && test -f .demo-keys/admin.json || solana-keygen new --no-bip39-passphrase -s -o .demo-keys/admin.json
solana-test-validator --reset \
  --bpf-program BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM target/deploy/elote_pool.so \
  --mint "$(solana-keygen pubkey .demo-keys/admin.json)"
```

`scripts/open-line.ts` opens one approved line on an existing pool for a borrower whose keypair file
is held locally, so the borrower's draw and repayments can be rehearsed in the app:
`npx tsx scripts/open-line.ts --cluster local|devnet --pool <address> --borrower-keypair <path> --principal <tMXN>`,
with `--rate-bps` (1200), `--term-days` (90), `--sponsor-keypair` and `--agreement-hash` (the demo's
placeholder if absent). `--installments <n> --interval-days <n>` open a scheduled line (version 2),
with `--late-rate-bps` (10000) and `--max-missed` (3, the program's minimum, so at least three
installments); due dates count from the draw, so `--term-days` must exceed installments times
interval. The admin (`ELOTE_ADMIN_KEYPAIR` or `.demo-keys/admin.json`) and the borrower sign the
same transaction. The script checks the pool's admin and pause state, then prints the line id and
address, the terms, the signature and an explorer link; program errors are shown by code and name.

`scripts/key-to-base58.ts <keypair.json>` prints the base58 form of a keypair file's secret bytes,
the format a wallet's private key import accepts. The output is a secret: use it for devnet demo keys only.

`scripts/token-metadata.ts` gives an existing test token a name, symbol and icon in wallets by creating
its Metaplex Token Metadata account (`createMetadataAccountV3`, mutable, no creators, no collection):
`npx tsx scripts/token-metadata.ts --cluster devnet|local --mint <address> --authority-keypair <path>`,
with `--update-authority` (the demo admin of `.demo-keys/admin.json` if present, else the signer),
`--uri` (https://elote.baby/token/tmxn.json, served by the `elote-web` deploy), `--name` ("tMXN test token")
and `--symbol` (tMXN). The token's mint authority signs and pays; `ELOTE_TOKEN_AUTHORITY_KEYPAIR` can
hold the keypair path instead of the flag. Only addresses, the signature and explorer links are printed.

The script refuses if the metadata account already exists (change it with `updateMetadataAccountV2`,
signed by the update authority), if the key is not the token's mint authority, or if the URI does not
answer 200 with JSON whose name and symbol match (`--skip-uri-check` before the web deploy). `--dry-run`
prints the instruction's accounts and sends nothing; without a keypair it uses a placeholder signer. On a
local validator, load the Token Metadata program first with
`--clone-upgradeable-program metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s --url https://api.devnet.solana.com`.
