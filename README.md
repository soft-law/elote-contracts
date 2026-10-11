# elote_pool

> Demo on Solana devnet with test assets. Not an offer of securities.

`elote_pool` is a Solana program (Anchor 1.1.2) for a liquidity pool with a share token.
Investors deposit the pool asset and receive shares. The pool admin approves credit lines for
entrepreneurs from the pool. When borrowers repay, the interest stays in the pool and every share
is worth more. A recognised loss lowers the value of every share once.

The demo asset is **tMXN (devnet test token, no value)**: a plain SPL token created by the demo
script. Nothing here holds or moves real money.

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
  the term and the hash of the signed credit agreement. Repayments go to principal first, then to
  interest. Anyone may repay on the borrower's behalf; the payer is recorded in the event.
- **Optional sponsor.** A public support program can be named on a line; it must co-sign the opening and
  may pay the interest. It never touches the pool's money otherwise.
- **Pause.** The admin can pause deposits, redemptions, openings, draws and repayments. Writing off
  an overdue line and unpausing are never blocked.

## What it is not

- Not a product with real money: devnet only, test assets only.
- Not an offer of securities, and no return is promised. Shares earn only as loans repay, and lose
  value when a loan is written off.
- Not audited yet.

Known behaviour of the share accounting, by design for this demo:

- **Interest counts only when it is collected.** A deposit made just before a repayment of
  interest shares in that interest as if it had been in the pool all along.
- **A write-off is recognised only when the admin records it.** Until then, holders can redeem at
  the full value, including the principal that will not come back. The operator procedure is to
  pause the pool first, then write off, then unpause.
- **After a total loss** (NAV 0 with shares still outstanding) deposits fail: the share price is
  undefined and the pool cannot be refilled.

## Known limitations (devnet demo)

1. The pool admin is part of the pool's creation seed and cannot be rotated, so a compromised admin can open a line to its own wallet and draw the liquidity (roadmap: a roles account with a two-step handover and per-line limits).
2. Interest counts only when it is repaid, so a deposit placed right before a known repayment captures that interest and can be redeemed right after.
3. An overdue line stays at full value until the admin writes it off, and there is no deadline for doing so (roadmap: write-off open to anyone after a grace period).
4. The pause blocks deposit, redeem, draw and repay but not write-off, so the operator procedure is to pause and then write off, and a hostile admin could misuse that sequence (roadmap: repay allowed while paused).
5. Any SPL asset mint is accepted, including mints with a freeze authority.
6. The program's upgrade authority is a single key (roadmap: a Squads multisig).

**Roadmap:** installments and arrears, sponsor escrow and interest accounting, SPL collateral, and disputes as a separate program.

## Accounts

| Account | Address | Holds |
|---|---|---|
| `Pool` | PDA `["pool", admin, asset_mint]` | admin, asset, vault, share token, outstanding principal, running totals, pause flag |
| share token | PDA `["shares", pool]` | an SPL token with the asset's decimals; only the pool PDA can issue shares |
| vault | PDA token account `["vault", pool]`, owned by the pool PDA | the pool's liquid assets |
| dead shares | PDA token account `["dead", pool]`, owned by the pool PDA | the 1,000 shares locked by the first deposit |
| `CreditLine` | PDA `["line", pool, line_id (u64 LE)]` | terms, agreement hash, repayments, status |

## Instructions

| Instruction | Signers | Effect |
|---|---|---|
| `init_pool` | admin | creates the pool, its share token, its vault and its dead-shares account |
| `deposit(assets, min_shares_out)` | depositor | assets in, shares out at the current NAV; the first deposit is at least 1.0 and locks 1,000 dead shares |
| `redeem(shares, min_assets_out)` | holder | shares burned, assets out at the current NAV, up to the vault's liquidity |
| `open_line(line_id, principal, rate_bps, term_end, agreement_hash, sponsor)` | admin, borrower, sponsor if named | approves the next line; moves no money |
| `draw` | borrower | the whole principal goes from the vault to the borrower, before the term ends |
| `repay(amount)` | anyone | principal first, then interest; the line closes when nothing remains |
| `write_off` | admin | after the term, the unpaid principal is recognised as a loss |
| `set_paused(paused)` | admin | pause switch |

Every instruction emits an event through `emit_cpi!`. Error codes start at 6000 in the order of
`PoolError` in `programs/elote_pool/src/errors.rs`.

## Build and test

Toolchain: Rust 1.89.0 (`rust-toolchain.toml`), Anchor CLI 1.1.2, Solana CLI 3.1.x, Node 20.18+.

```sh
NO_DNA=1 anchor build            # target/deploy/elote_pool.so and target/idl/elote_pool.json
cargo test -p elote_pool         # unit tests + LiteSVM integration tests against the built .so
npm install
npm run codama                   # regenerates clients/js/src/generated from the IDL
npm run check:client             # fails if the committed client differs from the IDL
npm run typecheck
```

The integration tests run the compiled program in LiteSVM 0.9.0, which loads SPL Token and the
Associated Token Account program by default.

## Demo

```sh
npx tsx scripts/demo-pool.ts --cluster local  [--agreement-hash <64 hex>] [--sponsor]
npx tsx scripts/demo-pool.ts --cluster devnet [--agreement-hash <64 hex>] [--sponsor]
```

All amounts below are in tMXN (devnet test token, no value). The script creates the test token
and a pool, then runs two investors (10,000 and 5,000), a 6,000 line at 12% with the given
agreement hash, the draw, the borrower's repayment of principal and the sponsor's payment of the
720 interest, then a partial redemption. It prints every signature with an explorer link and a final table of NAV, supply and
value per share before and after.

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

## Deployment order

1. `NO_DNA=1 anchor build`; tests green; `npm run check:client` clean.
2. `anchor deploy --provider.cluster devnet` with a dedicated upgrade key (`ANCHOR_WALLET`).
3. `anchor idl init -f target/idl/elote_pool.json --provider.cluster devnet BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`.
4. Demo script on `--cluster local`, then on `--cluster devnet`.
5. The web front end reads `Pool` over RPC.

Program and upgrade keypairs are never committed (`target/` is ignored).

> Demo on Solana devnet with test assets. Not an offer of securities.
