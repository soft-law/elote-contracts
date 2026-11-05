# elote-contracts

**Mexican loans on Solana.** The `elote_pool` program behind [elote.baby](https://elote.baby):
signed legal contracts become programmable loan contracts on Solana that carry out the repayment
terms, and depositors share the interest that borrowers repay.

> Demo on Solana devnet with test assets. Not an offer of securities.

This is the production repository for the program, its TypeScript client
(`@softlaw/elote-pool-client`, in [`clients/js`](clients/js)) and the demo scripts. The app lives in
`soft-law/elote-web` (private), the documentation in `soft-law/elote-docs`, and the agreements server in
`soft-law/softlaw-backend` (private).

Hackathon snapshot: https://github.com/soft-law/elote-baby/tree/colosseum-2026 (program, app,
agreements server and the Meteora launch as submitted).

## The `elote_pool` program

`elote_pool` is a Solana program (Anchor 1.2.1) for a liquidity pool with a share token.
Investors deposit the pool asset and receive shares. The pool admin approves credit lines for
entrepreneurs from the pool. When borrowers repay, the interest stays in the pool and every share
is worth more. A recognised loss lowers the value of every share once.

The demo asset is **tUSDC (devnet test token, no value)**: a plain SPL token created by the demo
script; lines and pagarés are in MXN. Nothing here holds or moves real money.

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
- **Credit lines from the pool.** Each line records the borrower, the approved commitment, an
  annual rate, the term, the hash of the signed credit agreement and a commitment to the credit
  file. The borrower draws the commitment in tranches while the draw window is open, each
  tranche against a signed pagaré and with the attestor's co-signature; interest on a tranche is
  fixed at the draw, proportional to the time to the last due date. Repayments go to late
  interest, then interest, then principal, and never take more than is due. Anyone may repay on
  the borrower's behalf; the payer is recorded in the event.
- **Installments, arrears and late interest.** A line can instead be repaid in installments
  (see [Installments and collection](#installments-and-collection)). Missed installments are
  computed from what was repaid, late interest accrues on overdue amounts after the pool's grace
  period, and a line with three missed installments goes to collection.
- **Optional sponsor.** A public support program can be named on a line; it must co-sign the opening and
  may pay the interest. It never touches the pool's money otherwise.
- **Pause.** Any council member or the guardian can pause deposits, redemptions, requests, approvals and draws; only the council resumes. Repayments, moving a line
  to collection, writing off an overdue line and unpausing are never blocked.

## Accounts

| Account | Address | Holds |
|---|---|---|
| `Pool` | PDA `["pool", admin, asset_mint]` | admin, asset, vault, share token, outstanding principal, running totals, the attestor, the series' Metaplex Core collection, the grace, collection and cancel-delay periods, the line cap (`max_line_bps`), the asset's issuer controls (`asset_controls`), `permissioned` and the pause flag |
| share token | a mint the admin creates and hands to `init_pool` | classic SPL Token or Token-2022 with the asset's decimals, mint authority = the pool PDA, supply 0, no extension that takes or hides value; frozen by default (Token-2022 `DefaultAccountState`) for a permissioned series under Token ACL |
| collection | a Metaplex Core collection the admin creates and hands to `init_pool` | the series' collection of pagaré assets; its update authority must be the pool PDA, so only the program mints into it |
| vault | PDA token account `["vault", pool]`, owned by the pool PDA | the pool's liquid assets |
| dead shares | PDA token account `["dead", pool]`, owned by the pool PDA | the 1,000 shares locked by the first deposit |
| `CreditLine` | PDA `["line", pool, line_id (u64 LE)]` | terms, agreement hash and expediente root, what was drawn and repaid, status, the draw window and the first due date, the installment schedule (scheduled lines), late interest collected and settled, the payment cursor `pagares_paid`, when the balance was accelerated |
| `Pagare` | PDA `["pagare", line, draw_index (u16 LE)]` | the program's record of one draw's pagaré: the asset's address, the signed pagaré's hash and how it was signed, the amount and the line's drawn total through it, when it was issued and when it is due, the registry (a hash chain of entries whose content is in the `PagareEntry` events), the procurador of record, any pending cancellation request, and once paid the "PAGADO" mark (status, paid_at, the receipt hashes); never closed; no folio |
| pagaré asset | PDA `["pagare_asset", line, draw_index (u16 LE)]`, owned by the Metaplex Core program | the pagaré token itself (decision 24): a Core asset in the series' collection, owned by the pool PDA and frozen by a permanent freeze delegate whose authority is the collection's (the pool PDA), with immutable metadata, locked attributes (pagaré hash, amount, currency, issue and due dates, line, draw index, certificate fingerprint, signature method) and an `AddBlocker`; no transfer or burn delegate at creation. The first endoso en procuración adds an owner-managed `BurnDelegate` whose authority is the program address `["procurador", pagare, procurador]` |

Every parameter is a typed field (Borsh, no padding), and each account ends in `reserved` bytes,
created zero, for fields added after mainnet: 64 on `Pool` and `CreditLine`, 32 on `Pagare`. With
the 8-byte discriminator the sizes are `Pool` 346, `CreditLine` 375 and `Pagare` 387 bytes
(`tests/layout.rs`); ARCHITECTURE.md §2 gives every offset. A bullet line (single maturity) has
`installments == 0` and the other schedule fields at zero. Clients read the typed fields;
`lineSchedule(line)` in the client returns a line's schedule, or null for a bullet line, and
`clients/js/src/schedule.ts` computes arrears exactly as the program does.

## Instructions

| Instruction | Signers | Effect |
|---|---|---|
| `init_pool(grace_period_seconds, max_line_bps, attestor, collection_period_seconds, cancel_delay_seconds)` | admin | creates the pool, its vault and its dead-shares account around an asset mint and an admin-supplied share mint, each under its own token program; applies the asset-mint and share-mint policies and records `asset_controls` and `permissioned`; records the attestor (the agreements engine's key, never the zero key or the admin) and the series' Metaplex Core collection (a bare one with the pool PDA as update authority: no plugins, nothing minted); the grace period and the concentration cap (largest line as a share of the NAV, in basis points, 1 to 10,000) are fixed for the life of the pool; the collection period (at least 30 days) and the cancel delay of the backstop (at least 7 days) can be changed by the admin; emits `PoolInitialized` with every parameter |
| `deposit(assets, min_shares_out)` | depositor | assets in, shares out at the current NAV; the first deposit is at least 1.0 and locks 1,000 dead shares |
| `redeem(shares, min_assets_out)` | holder | shares burned, assets out at the current NAV, up to the vault's liquidity |
| `open_line(line_id, principal, rate_bps, term_end, agreement_hash, sponsor, sponsor_role, schedule, expediente_root)` | admin, borrower, attestor, sponsor if named | approves the next line as a commitment of `principal` at an annual `rate_bps`; moves no money; refused while paused; the borrower is neither the admin nor the attestor, and `principal <= NAV * max_line_bps / 10_000` (reads the vault); `schedule` (optional, with the contract's `first_due_at`) makes it an installment line; the draw window closes at `first_due_at`, or at `term_end` on a bullet line |
| `draw(amount, document_hash, asset_bump, pagare_bump, cert_fingerprint, signature_method)` | borrower, attestor, payer | one tranche from the vault to the borrower while the window is open (strictly before it closes), at most what is still available and at most 64 tranches a line, against the signed pagaré's hash; refused while paused; interest on the tranche, at the annual rate to the last due date, is fixed here; mints the pagaré token (a Metaplex Core asset in the series' collection, held frozen by the pool PDA) and its `Pagare` record, whose rent the `payer` (the app's relayer) funds; the client derives the two PDA bumps so the program verifies the addresses without a search; `cert_fingerprint` (the PSC certificate that signed the text) and `signature_method` (0 wallet, 1 PSC + wallet, 2 e.firma + wallet) go into the record and the asset's attributes |
| `repay(amount, receipt_hash)` | anyone | late interest (which `amount` must cover), then interest, then principal, never more than is due; what is owed is what was drawn plus the interest fixed at each draw. The line closes when nothing is due and nothing can be drawn any more; while it can still draw, a payment that clears the debt leaves it open and a payment on a zero debt is refused. Takes the pagaré records the payment touches as remaining accounts (from the line's cursor `pagares_paid` through the first pagaré left uncovered, or the last draw; `PagareAccountsMismatch` otherwise) and writes an entry only on a pagaré it applies principal to: `Pagado`, with the PAGADO mark, where the principal repaid reaches the pagaré's `drawn_through`; `Abono` on the one left uncovered. An interest-only payment writes no entry and is recorded in `PagarePayment`. `receipt_hash` is the payer's receipt, zero meaning "to follow". Clients send at most five pagaré records per `repay` and split a larger payment. Never paused |
| `mark_in_collection` | anyone | moves an installment line to `InCollection` once `max_missed` installments are missed and the grace period of the last of them has run. Never paused |
| `write_off` | admin | an `Active` line after the term plus grace, or a line in collection once the pool's collection period has run from the mark and either its oldest missed due date plus grace or the term plus grace has passed: the unpaid principal is recognised as a loss. Never paused |
| `suspend_draws` | admin | closes a line's draw window now (a KYC failure, a default event); the undrawn remainder is forfeited, repayment is untouched, the pool is not paused |
| `cancel_line` | admin | clears an approved commitment nobody drew (status `Cancelled`, terminal); refused once anything was drawn |
| `endorse_pagare(draw_index, kind, to, endoso_hash)` | holder (the admin while the asset is in the pool), attestor, payer | endoso en procuración of a pagaré to `to` (the only kind the pool makes): appends the entry to the pagaré's registry and lends the power to extinguish it through an owner-managed `BurnDelegate` whose authority is the program address `["procurador", pagare, to]` (added the first time, approved again on later procuraciones), so the procurador can only extinguish it inside `cancel_pagare`; the asset stays with the pool. Refused when `to` is the zero key, the pool, the admin or the attestor (`EndorseeInvalid`), on a written-off line, on a pagaré that is not outstanding, and while a procuración is in force |
| `revoke_procuracion(draw_index, entry_hash)` | holder (the admin while the asset is in the pool), payer | revokes the procuración in force over the signed revocation's hash: appends the `Revocacion` entry and returns the burn delegate's authority to the owner (the plugin stays); refused when none is in force |
| `court_annotation(draw_index, kind, document_hash)` | holder (the admin while the asset is in the pool), attestor | records a court's act on a pagaré (embargo, suspension, cancellation; the constancia of art. 28) as an entry of its registry; moves nothing; refused only on a burnt title |
| `annotate_payment(draw_index, entry_count, receipt_hash)` | holder (the admin while the asset is in the pool) or attestor | attaches the real receipt to a payment entry of a pagaré's registry (a `Recibo` entry referencing it) when the payer's hash was zero or wrong; allowed whatever the pagaré's status; the holder's annotation also replaces the payer's provisional `receipt_hash` on the record while the pagaré carries the PAGADO mark and is not yet burnt (`PAGARE_PAID_PENDING_CANCEL`) |
| `request_cancellation(draw_index, receipt_hash)` | holder (the admin while the asset is in the pool) | the backstop on the attestor: on a pagaré carrying the PAGADO mark (`PagareNotCovered` otherwise), whatever the line's status, records the request to cancel it against the receipt's hash as a `SolicitudCancelacion` entry of its registry and fixes the deadline `cancel_after = max(now + cancel delay, the pending deadline)`; a new request replaces a pending one (its receipt, a fresh wait, never an earlier deadline); a later change of the delay never moves it; cleared by any cancellation |
| `cancel_pagare(draw_index, receipt_hash)` | holder (the admin while the asset is in the pool) or the procurador of record, attestor (or the holder alone once a recorded request's deadline has passed, with the requested receipt), payer | extinguishes a pagaré against the FEA-signed receipt's hash once a repayment has written its PAGADO mark (`PAGARE_PAID_PENDING_CANCEL`; `PagareNotCovered` otherwise), whatever the line's status (repayments mark the pagarés oldest first as the principal repaid reaches each `drawn_through`; interest alone covers nothing); appends the `Terminacion` entry to the pagaré's registry, sets the record's status to paid and keeps the cancellation's receipt beside the payment's (the record is never closed), clears any pending request, ends any procuración in force, emits `PagareEntry` and `PagareCancelled`, then thaws the asset (`UpdatePluginV1`, signed by the pool PDA) and burns it (`BurnV1`) as its owner or, for the procurador, through the program address holding the burn delegate; the rent goes to the `payer`. Never paused |
| `pause` / `resume` | a council member or the guardian / the council | pause of deposits, redemptions, requests, approvals and draws; repay stays open |
| `set_attestor` | admin, the new attestor | replaces the pool's attestor with the co-signing key, which proves the engine controls it; never the admin itself; the recovery when the agreements engine's key rotates |
| `set_collection_period(collection_period_seconds)` | admin | changes the seconds a line in collection waits, from its mark, before it can be written off; never below 30 days |
| `set_cancel_delay(cancel_delay_seconds)` | admin | changes the seconds a cancellation request waits before the holder may cancel the pagaré without the attestor; never below 7 days; a pending request keeps the deadline it stored |

The 46 error codes are numbered from 6000 in the order of `PoolError` in
`programs/elote_pool/src/errors.rs`, where they are grouped by domain.

## Installments and collection

`open_line` takes an optional `ScheduleParams { interval_seconds, installments, late_bps, max_missed }`.
Without it the line is a bullet line (single maturity) and nothing below applies.

- **Schedule.** The calendar comes from the signed contract (decision 25): installment `k` falls
  due at `first_due_at + (k - 1) * interval_seconds`, whenever the tranches were drawn. Each
  installment is `(drawn + interest_due) / installments`, rounded down; the last one carries the
  remainder, so they add up exactly, and a later tranche before the first due date raises every
  installment. Limits: 1 to 120 installments, a positive interval, `late_bps <= 10_000`,
  `3 <= max_missed <= installments` (the owner's policy is three months of arrears before
  collection), `first_due_at` after the opening, the last due date no later than `term_end`, and
  at least one unit per installment both for the whole commitment at the opening and for what is
  drawn after each tranche.
- **Missed installments** are computed, never stored: installments fallen due minus installments fully
  covered by the principal and interest repaid. At its due second an installment already counts as due.
  The pool's grace period does not change the count.
- **Late interest** accrues on the **principal share** of each overdue part (decision 11: no
  interest on overdue interest; the share is `part * principal / total`, rounded down) from its due
  date plus the pool's grace period (or from the last repayment, whichever is later), linear per
  second at `late_bps` a year (365 days):
  `Σ principal_share(part) * min(seconds, cap) * late_bps / (10_000 * 31_536_000)`, rounded down, in
  `u128`, where `cap = ceil(10_000 * 31_536_000 / late_bps)` seconds. Two caps apply:
  - **per settlement:** within one repayment, each overdue part accrues at most its own principal share. A
    repayment that collects late interest settles it in full and records `late_paid_until`, so no
    period is charged twice; a repayment that collects none (the accrual rounded down to zero) leaves
    the watermark alone, so the accrual keeps running. A part that stays unpaid accrues again after
    the next settlement, so over several repayments the late interest on one part can exceed that part.
  - **over the life of the line:** late interest collected never exceeds the line's principal
    (`late_due <= principal - late_collected`). A borrower who pays only late interest reaches this cap
    and from then on every payment goes to interest and principal.

  Late interest goes to the vault and raises the NAV like interest; it is counted in the line's
  `late_collected`, not in the pool's `interest_collected`.
- **Repayment order** on an installment line: late interest, then interest, then principal.
- **Collection.** Once `max_missed` installments are missed (at least three, three months of arrears)
  and the grace period of the last of them has run (`now > first_due_at + (covered + max_missed - 1) * interval
  + grace`, with `covered` installments paid), anyone may call `mark_in_collection`, which moves the line
  to `InCollection` and records the acceleration (`accelerated_at`): from the mark every installment is
  due and late interest runs on all remaining principal (vencimiento anticipado, decision 28), while the
  ordinary interest of the installments not yet due is forgiven. Repayments are still accepted there;
  there is no cure, and only a full repayment moves the line to `Repaid`.
- **Write-off policy.** After the third month in arrears the line goes to out-of-court collection and the
  admin records the write-off. The program allows `write_off` only when the pool's collection period has run from the mark and the line is in collection and its
  oldest missed installment fell due more than the grace period ago, or when the term ended more than the
  grace period ago. Only the admin may call it.

## Events

Every instruction emits its events through `emit_cpi!` (`programs/elote_pool/src/events.rs`), in
the order listed for each instruction.

| Event | Emitted by | Fields |
|---|---|---|
| `PoolInitialized` | `init_pool`, its only event | pool, admin, attestor, asset_mint, share_mint, collection, grace_period_seconds, collection_period_seconds, cancel_delay_seconds, max_line_bps, asset_controls (bits: 0 freeze authority, 1 `Pausable`), permissioned |
| `Deposited` | `deposit`, then `NavUpdated` | pool, depositor, assets, shares, nav_after, supply_after |
| `Redeemed` | `redeem`, then `NavUpdated` | pool, holder, shares, assets, nav_after, supply_after |
| `LineOpened` | `open_line` | pool, line_id, borrower, sponsor, sponsor_role (0 promoter, 1 guarantor; meaningful only with a sponsor), agreement_hash, expediente_root, principal, rate_bps, term_end, availability_end, first_due_at, interval_seconds, installments, late_bps, max_missed (the last five zero on a bullet line) |
| `LineDrawn` | `draw`, first | pool, line_id, draw_index, amount (the tranche), drawn, available, interest_due, availability_end (the line after the tranche), outstanding_after (the pool's) |
| `PagareIssued` | `draw`, after `LineDrawn` | pool, line_id, draw_index, asset (the Core asset), pagare_hash, amount, due_at, holder (the pool PDA) |
| `PagareEntry` | every instruction that appends to a pagaré's registry: `draw` (the issuance entry, after `PagareIssued`), `repay`, `endorse_pagare`, `revoke_procuracion`, `court_annotation`, `annotate_payment`, `request_cancellation`, `cancel_pagare` | pool, line_id, draw_index, asset, kind, from, to, entry_hash, entry_count, recorded_at, stated_at |
| `PagarePayment` | `repay` that took pagaré records, after its `PagareEntry` events | pool, line_id, payer, receipt_hash, late_part, interest_part, principal_part, entries (draw_index, kind, entry_count, principal_applied; empty for an interest-only payment) |
| `LateInterestCollected` | `repay` with late interest, before `LineRepaid` | pool, line_id, amount |
| `LineRepaid` | `repay`, then `NavUpdated` | pool, line_id, payer, principal_part, interest_part, remaining (without late interest), nav_after |
| `LineInCollection` | `mark_in_collection` | pool, line_id, missed, remaining (after the forgiveness), interest_forgiven (the ordinary interest of the installments not yet due at the mark) |
| `LineWrittenOff` | `write_off`, then `NavUpdated` | pool, line_id, loss, nav_after |
| `NavUpdated` | `deposit`, `redeem`, `repay`, `write_off`, last in each | pool, total_assets, share_supply |
| `LineDrawsSuspended` | `suspend_draws` | pool, line_id, available_forfeited |
| `LineCancelled` | `cancel_line` | pool, line_id |
| `PagareReceiptAnnotated` | `annotate_payment`, after its `PagareEntry` | pool, line_id, draw_index, entry_count (the `Recibo` entry's), payment_entry_count (the entry it annotates), receipt_hash |
| `CancellationRequested` | `request_cancellation`, after its `PagareEntry` | pool, line_id, draw_index, asset, receipt_hash, cancel_after (the deadline after which the holder may cancel alone with that receipt) |
| `PagareCancelled` | `cancel_pagare`, after its `PagareEntry` and before the burn | pool, line_id, draw_index, asset, receipt_hash, holder (the key that extinguished it), backstop (true when the holder cancelled alone after a request's deadline) |
| `PoolPaused` | `set_paused` | pool, paused |
| `AttestorSet` | `set_attestor` | pool, attestor |
| `CollectionPeriodSet` | `set_collection_period` | pool, collection_period_seconds |
| `CancelDelaySet` | `set_cancel_delay` | pool, cancel_delay_seconds |

## Build and test

Toolchain: Rust 1.89.0 (`rust-toolchain.toml`), Anchor CLI 1.2.1 (from [otter-sec/anchor](https://github.com/otter-sec/anchor), where Anchor now lives), Solana CLI 4.1.2 (the pairing Anchor 1.2 is tested against), Node 22.12+. `avm install 1.2.1 && avm use 1.2.1` and `agave-install init 4.1.2` set both; CI installs the same versions and checks the Anchor binary's sha256.

```sh
NO_DNA=1 anchor build --ignore-keys --arch v0 -- --features localnet   # target/deploy/elote_pool.so and target/idl/elote_pool.json
cargo test -p elote_pool         # unit tests + LiteSVM integration tests against the built .so
npm install
npm run codama                   # copies the IDL to idl/ and regenerates clients/js/src/generated
npm run check:client             # fails if the committed client differs from idl/elote_pool.json
npm run typecheck
npm run test:client              # replays the program's arrears vectors against clients/js/src/schedule.ts
```

The `localnet` feature lets `init_pool` accept any asset mint, which the tests and the local e2e
need. Without a feature the program accepts only mainnet USDC and MEXe; `--features devnet`
accepts the Etherfuse sandbox MEXe and USDC and Circle's devnet USDC (`state::ASSET_ALLOWLIST`). A
deployed build is verified byte for byte against a build with its cluster's feature.

`anchor build` on a fresh clone writes a new random `target/deploy/elote_pool-keypair.json`. It is
not the program's key: never run `anchor keys sync` or `anchor deploy` with it, or the program id
in `declare_id!` and `Anchor.toml` would change. Anchor 1.2 refuses to build on that mismatch, hence
`--ignore-keys`. Tests and the local validator use the explicit id.

`--arch v0` keeps the SBPF v0 format of the deployed program: Anchor 1.2 defaults to SBPF v3, which
the Solana 3.x runtime inside LiteSVM 0.11 cannot load. Moving to v3 goes together with
`anchor-lang`'s `solana-v4` feature and a LiteSVM on the Agave 4.x line.

The integration tests run the compiled program in LiteSVM 0.11.0, which loads SPL Token, Token-2022
and the Associated Token Account program by default; the harness adds Metaplex Core from
`programs/elote_pool/tests/fixtures/` (pinned by sha256, see the README there).
There are 163 Rust tests in 14 binaries (29 unit tests in `src/` and 134 integration tests) and
11 JS tests. `programs/elote_pool/tests/` holds:

| File | Tests | What it checks |
|---|---|---|
| `pool.rs`, `lines.rs`, `schedule.rs` | 23, 11, 20 | Behaviour of every instruction, happy paths and each error code |
| `known_limits.rs` | 12 | Documentation tests that pin the known design limits listed in SECURITY.md; they pass by design and must be inverted when a limit is removed |
| `tranches.rs` | 12 | Tranched draws: the draw window, the payability floor, per-tranche interest, the cap on tranches, the attestor on every draw, `suspend_draws`, `cancel_line` |
| `pagare.rs` | 22 | The pagaré token: one frozen Core asset and one `Pagare` record per draw, every plugin read back through the `mpl-core` crate against the pinned Core binary, wrong accounts refused; the registry entries; `cancel_pagare`: the PAGADO mark, the burn, coverage oldest first, the backstop, the procuración, the refusals |
| `core_capabilities.rs` | 8 | What Metaplex Core allows after an asset's creation: handing off or extinguishing the permanent delegates, thawing, moving the update authority off the collection, adding plugins and adapters, who writes and removes an AppData adapter and whether it follows the asset (the program uses none), the cost of `AddBlocker`, owner-managed delegates for the procuración and a burn delegate held by a program address; pinned against the Core binary so an upgrade that changes an answer fails here |
| `layout.rs` | 1 | The account sizes (`Pool` 346, `CreditLine` 375, `Pagare` 387 bytes) and the zeroed `reserved` tail of each account |
| `audit.rs` | 5 | Regression tests of the findings of the 2026-11-01 audit |
| `reaudit.rs` | 5 | Regression tests of the findings of the 2026-11-02 re-audit |
| `compute.rs` | 7 | Compute units of `init_pool`, `draw` (with the Core CPI), `repay` (one covering five pagaré records included), `cancel_pagare` (by the holder, alone and by the procurador), `request_cancellation`, `endorse_pagare`, `revoke_procuracion`, `court_annotation`, `mark_in_collection` and `write_off`, at 18 and 120 installments, stay under 100,000 |
| `math_props.rs` | 7 | Property tests (proptest) over the share accounting and the arrears arithmetic |
| `vectors.rs` | 1 | Evaluates the arrears arithmetic on 421 cases (44 accelerated) and compares with `clients/js/test/arrears-vectors.json`, which `npm run test:client` replays against the TypeScript mirror. After a deliberate change to `math.rs`, regenerate with `UPDATE_VECTORS=1 cargo test --test vectors` in the same commit |

## Deployments and upgrade authority

| Cluster | Program id | Upgrade authority |
|---|---|---|
| devnet | `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM` | `GU7CfoGZYsfkx23aJoqFjfwyAmLCExqBBoBFrn2EvZKS` (single key) |
| mainnet-beta | not deployed | |

The devnet program was built with Anchor 1.1.2 and Solana CLI 3.1.10, before `security.txt` was added; the last commit on that toolchain is tag `pre-audit-2026-10-19`, and `main` (Anchor 1.2.1, Solana 4.1.2) produces a different binary. No deployment has been verified against any commit yet; the next deployment should be made from `main` so the on-chain program and the source line up. The devnet authority is a single hot wallet, acceptable for a test deployment only. Before any
mainnet deployment the authority moves to a [Squads](https://squads.so) multisig, the IDL is
published on chain (`anchor idl init`), and the build is verified. The program embeds a
`security.txt` (see `SECURITY.md`).

Reproduce and compare the on-chain build with [solana-verify](https://github.com/Ellipsis-Labs/solana-verifiable-build):

```sh
solana-verify build --library-name elote_pool
solana-verify get-executable-hash target/deploy/elote_pool.so
solana-verify get-program-hash -u devnet BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM
```

The two hashes match when the deployed program was built from this commit in the verify
container. No deployment has been verified yet; the first one that is will be recorded here.

### Releasing the client

Bump `version` in `clients/js/package.json`, commit on `main`, tag `client-v<version>` and push the
tag. `release-client.yml` checks the tag is on `main`, regenerates the client against the IDL,
builds and publishes through npm trusted publishing (OIDC): no token in the repo, provenance
attached by npm. One-time setup by the package owner: publish the first version by hand from a
logged-in machine, then on npmjs.com add the trusted publisher (GitHub, `soft-law`,
`elote-contracts`, `release-client.yml`, environment `npm-publish`) and add required reviewers
to the `npm-publish` environment in this repository's settings.

## Demo

```sh
npx tsx scripts/demo-pool.ts --cluster local  --multisig <address> [--agreement-hash <64 hex>] [--sponsor] [--schedule]
npx tsx scripts/demo-pool.ts --cluster devnet --multisig <address> [--agreement-hash <64 hex>] [--sponsor]
```

Units: lines and pagarés are in MXN (centavos on chain; the scripts take pesos with up to 2
decimals), the vault holds the pool asset, here **tUSDC (devnet test token, no value)**, 6 decimals;
deposits, redemptions and repayments are in tUSDC. Draws deliver tUSDC at the FIX in force and
repayments are credited in MXN at it. The test asset has no value: nothing here is ever tested with
an asset of real value.

The council is vault 0 of a Squads v4 multisig (`--multisig`); its members' keys come from
`--council-keypairs <a.json,b.json>`, `ELOTE_COUNCIL_KEYPAIRS` or `.demo-keys/curator-<n>.json`, and every
council step runs as a Squads vault transaction (created, proposed, approved by the threshold,
executed with a 100,000 CU limit) through `scripts/lib/council.ts`, whose Squads instructions are
hand-built in `scripts/lib/squads.ts` (Squads v4 af94153, the pinned binary's sha256 in its header).
A Squads error and an elote_pool error can share a number (both count from 6000): the scripts name
the program that raised it from the logs.

The demo creates the test token, the share token and the pagarés' Metaplex Core collection (bare,
with the pool PDA as update authority, as `init_pool` requires) and a pool on the given multisig. The
council publishes a FIX (18.3740); two investors deposit (10,000 and 5,000 tUSDC); a MXN 60,000 line
at 12% is requested and approved by the council; the borrower draws it (pagaré 0, delivered as tUSDC
at the FIX) and repays what it holds; the sponsor pays the rest; then a partial redemption. It prints
every signature with an explorer link and a final table of NAV, supply and value per share.

With `--schedule` it then requests a MXN 6,000 line at 12% in six installments with 100% a year late
interest and collection after three missed installments, has the council approve it, draws it, lets
the first installment go unpaid past its due date and the grace period, and repays it with the next
one: late interest first, then interest, then principal. A validator's clock cannot be moved forward,
so on `--cluster local` an installment falls due every 20 seconds (`--interval-seconds`) and the
pool's grace period is 5 seconds (`--grace-seconds`).

Wallets are generated into `.demo-keys/` (ignored by git) unless `ELOTE_ADMIN_KEYPAIR`,
`ELOTE_INVESTOR_A_KEYPAIR`, `ELOTE_INVESTOR_B_KEYPAIR`, `ELOTE_BORROWER_KEYPAIR` or
`ELOTE_SPONSOR_KEYPAIR` point to keypair files. Every script prints the cluster and, unless the RPC is
this machine or `--yes` is given, asks before sending. `ELOTE_RPC` overrides the RPC URL.

`scripts/local-e2e.sh` runs every script below end to end on a validator of its own: it preloads the
program, the pinned Metaplex Core and Squads v4 binaries (checking their sha256) and a 2-of-3 Squads
multisig of three keys it makes (`scripts/squads-local-multisig.ts` writes the account file, so no
Squads ProgramConfig is needed), with the ledger and all keys in a temporary directory removed on
exit. It refuses any RPC that is not this machine and refuses to start when something already
answers on its port (`ELOTE_E2E_PORT`, default 18899). Build first
(`NO_DNA=1 anchor build --ignore-keys --arch v0 -- --features localnet`).

`scripts/request-line.ts --pool <address> --borrower-keypair <path> --principal <MXN>` requests one
line, with `--rate-bps` (1200), `--term-days` (90), `--late-rate-bps` (10000), `--sponsor-keypair` with
`--sponsor-role promoter|guarantor`, and `--agreement-hash` (or `--agreement-file` with
`--agreement-salt`: the hash is then `sha256(salt || file)`, the convention for agreement and receipt
hashes), `--expediente-root` and `--pagare-template-hash`. `--installments <n> --interval-days <n>`
request a scheduled line, with `--max-missed` (3, the program's minimum). The borrower and a payer
(the demo admin, standing in for Elote's backend) sign; the script prints the terms hash the council
approves.

`scripts/approve-line.ts --pool <address> --line-id <n>` has the council approve a requested line: it
recomputes the terms hash from the stored terms with the client's `termsHash`, refuses on a mismatch,
and runs `approve_line` through Squads with `valid_until` a day ahead.

`scripts/post-fix.ts --pool <address> --fix <18.3740> [--fix-date <yyyymmdd>]` publishes Banxico's FIX
(the date defaults to today in Mexico City by the chain clock): through the council, or by the pool's
feeder with `--feeder-keypair`.

`scripts/council-propose.ts --pool <address> --op <op>` runs one council decision on the pool's
settings through Squads: `set-fx-feeder --address`, `set-guardian --address`, `set-max-line-bps
--bps`, `set-outstanding-cap --usdc`, `reject-line --line-id`, `resume`.

`scripts/draw.ts --pool <address> --line-id <n> --borrower-keypair <path> --amount <MXN>` draws one
tranche and mints its pagaré, with `--signature-method wallet|psc|efirma`; psc and efirma need
`--cert-fingerprint` and `--cert-salt`, and the draw records `sha256(salt || fingerprint)`, never the
fingerprint (the SAT publishes e.firma certificates by RFC). The script prints the FIX and the tUSDC it
delivers before asking; the borrower signs alone (a FIX published in between fails the draw).

`scripts/repay.ts --pool <address> --line-id <n> --amount <tUSDC> | --all` pays on a line (the
borrower by default, `--payer-keypair` for anyone else; `--receipt-hash` or `--receipt-file` with
`--receipt-salt`, zero meaning "to follow"), credited in MXN at the FIX in force. A payment that would
touch more than three pagaré records is split into several `repay` calls (`MAX_SIDECARS_PER_REPAY`; the
program refuses more with `TooManySidecars`).

`scripts/endorse-pagare.ts --pool <address> --line-id <n> --draw-index <n>` endorses an outstanding
pagaré en procuración through the council. By default it makes a procurador key for the case,
`.demo-keys/procuradores/<pool>-<line>-<draw>.json`, to hand to the collection agency: one key per
case, never reused, or anyone could list the lines in collection from it. `--procurador <address>`
names an existing key and warns when it already holds a pagaré under procuración
(`pagaresUnderProcuracion` in the client). `--revoke` revokes the procuración in force. The procurador
can never be the pool, the council, a curator, the guardian or the borrower.

`scripts/cancel-pagare.ts --pool <address> --line-id <n> --draw-index <n> --receipt-hash <64 hex>`
extinguishes a pagaré a repayment marked paid: by default the council as holder (through Squads);
`--borrower-keypair` cancels as the line's borrower alone once the cancel grace the payment fixed
on the pagaré has run (refused before it); `--procurador-keypair` cancels as the procurador of record
alone through its program address.

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
