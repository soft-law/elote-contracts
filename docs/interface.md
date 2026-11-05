# `elote_pool` interface reference

Generated from `idl/elote_pool.json` by the PR 4 docs commit; the instruction and field docs are the program's own (`///` comments). Regenerate this file whenever the IDL changes. Program id `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`.

Conventions:

- **Council instructions** take the Squads multisig account (`multisig`, which must be `Pool.multisig`) and run the multisig check before anything else is written: owner = Squads v4, the council is vault 0 at the stored canonical bump, no config authority, at most ten members, threshold greater than half the voting members (`CouncilInvalid`, `CouncilTooLarge`). The council signs only through Squads `vault_transaction_execute`.
- **`valid_until: i64`** is the last argument of every council instruction except `pause` (a late pause only protects) and `init_pool` (the council does not sign it). `now > valid_until` is refused with `ProposalExpired`, and `valid_until > now + MAX_PROPOSAL_HORIZON_SECONDS` (15 days) with `ProposalTooFar`; both bounds are inclusive. The check runs first, before the multisig check. The scripts pass `now + 1 day`.
- Amounts on a line and a pagaré are **centavos of MXN** (`CreditLine.currency = 484`); the vault, deposits, redemptions and repayments are **USDC base units** (6 decimals). Conversions use the FIX (`Pool.fix_rate`, MXN per USD × 10^4).
- **Late payment has no grace period** (decision 36): late interest runs from the first second past a due date (or `term_end` on a bullet line), `mark_in_collection` is open a second after the due date of the `max_missed`-th missed installment (`max_missed` at least 3), and `write_off` counts from the due dates and the term themselves. `Pool.grace_period_seconds` is reserved and always zero.
- Every instruction that emits events takes `event_authority` and `program` (Anchor `#[event_cpi]`); they are not listed below.
- Error codes start at 6000 in the order of `errors.rs`. **Squads also numbers its errors from 6000** (its `NotAMember` is 6005, the same number as `CouncilInvalid`): decode an error by the program that raised it (the invoke stack in the logs), never by number alone. `scripts/lib/cli.ts` does this.

## Pool and council

### `init_pool(params: InitPoolParams)`

Creates the pool, its vault and its dead-shares account around `asset_mint`, the creator's share mint and the series collection. The creator signs and pays; the council is vault 0 of `multisig`, an autonomous Squads v4 multisig of at most ten members with a majority threshold, and does not sign. Sets the line cap (`1..=10_000` bps of the NAV), the collection period, the outstanding cap, the acceleration notice and the cancel grace. There is no grace period on late payment: late interest runs from the first day past a due date.

Signers: `creator`.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `creator` | yes | yes | Pays for the pool and seeds its address; no other power. |
| `asset_mint` |  |  | Classic SPL Token or Token-2022; `token_program` must be the mint's own program. |
| `pool` | yes |  |  |
| `share_mint` |  |  | The share mint, created by the creator beforehand and validated here: mint authority = this pool's PDA, supply 0, the asset's decimals, and no extension that takes or hides value. `DefaultAccountState = Frozen` makes the shares permissioned. |
| `vault` | yes |  | A PDA token account, so nobody can pre-create it and block the pool. |
| `dead_shares` | yes |  | Holds the dead shares of the first deposit; nothing ever moves them. On a permissioned share mint it is born frozen and must be thawed by the mint's freeze authority before the first deposit (in this transaction or a later one). |
| `collection` |  |  | The series' Metaplex Core collection, created by the creator beforehand with the pool PDA as update authority. |
| `multisig` |  |  | The Squads v4 multisig whose vault 0 becomes the council; it does not sign here. |
| `token_program` |  |  | The token program of `asset_mint` (classic SPL Token or Token-2022). |
| `share_token_program` |  |  | The token program of `share_mint` (Token-2022 in production, under Token ACL). |
| `system_program` |  |  | Fixed address `11111111111111111111111111111111`. |

`InitPoolParams`:

| Field | Type | Notes |
|---|---|---|
| `max_line_bps` | `u16` | `1..=BPS`; fixed for the pool's life. |
| `collection_period_seconds` | `u32` | At least `MIN_COLLECTION_PERIOD_SECONDS`. |
| `guardian` | `pubkey` | The zero key for none. |
| `max_outstanding_principal` | `u64` | At least 1. |
| `acceleration_notice_seconds` | `u32` | At least `MIN_ACCELERATION_NOTICE_SECONDS`. |
| `cancel_grace_seconds` | `u32` | `MIN_CANCEL_GRACE_SECONDS..=MAX_CANCEL_GRACE_SECONDS`. |
| `min_fix_rate` | `u64` | The lowest FIX `post_fix` accepts; zero with `max_fix_rate` zero for no bounds. |
| `max_fix_rate` | `u64` | The highest FIX `post_fix` accepts; at least `min_fix_rate` unless both are zero (`WrongAmount`). |

### `propose_council(new_council: pubkey, kind: u8, valid_until: i64)`

Names the next council: `kind` 0 (a Squads vault), the only kind taken. Council only, after the multisig check; the zero key withdraws the proposal.

Signers: `council`. Council instruction: runs the multisig check on the current council's multisig before writing the proposal; `accept_council` runs it again on the new one.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

Errors: `ProposalExpired` (`valid_until` passed), `ProposalTooFar` (`valid_until` more than 15 days ahead), `CouncilKindInvalid` (`kind` other than `COUNCIL_SQUADS`), `CouncilInvalid` (`multisig` is not `Pool.multisig`, or fails the multisig check), `CouncilTooLarge`, `Unauthorized` (the signer is not `Pool.council`).

### `accept_council(valid_until: i64)`

The proposed council signs and takes over, after the same multisig check as `init_pool`.

Signers: `new_council`. Signed by the proposed council (vault 0 of the new multisig, through Squads); runs the multisig check on the new multisig.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `new_council` |  | yes | The proposed authority: vault 0 of `multisig`, signing through Squads. |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `set_guardian(guardian: pubkey, valid_until: i64)`

Sets the guardian, a key that may only pause (the zero key for none). Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `set_max_line_bps(max_line_bps: u16, valid_until: i64)`

Sets the line cap, `1..=10_000` bps of the NAV, measured at every approval and draw. Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `set_outstanding_cap(max_outstanding_principal: u64, valid_until: i64)`

Sets the pool-wide cap on the USDC cost lent out. Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `set_fix_bounds(min_fix_rate: u64, max_fix_rate: u64, valid_until: i64)`

Sets the lowest and highest FIX `post_fix` accepts (zero and zero for none). Council only. `min_fix_rate <= max_fix_rate` with `max_fix_rate` positive, or both zero (`WrongAmount`); the FIX in force is not re-checked. Emits `FixBoundsSet`.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `set_acceleration_notice(acceleration_notice_seconds: u32, valid_until: i64)`

Sets the acceleration notice, at least seven days. Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `set_collection_period(collection_period_seconds: u32, valid_until: i64)`

Sets the seconds a line in collection waits, from its mark, before it can be written off. Council only; refused below the 30-day floor.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `set_cancel_grace(cancel_grace_seconds: u32, valid_until: i64)`

Sets the wait from a pagaré's payment before its borrower may cancel it alone; never below 30 days nor above 365. Applies to pagarés paid from then on: each keeps the grace in force at its payment. Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `set_fx_feeder(fx_feeder: pubkey, valid_until: i64)`

Names the key that publishes the FIX (the zero key for none). Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `post_fix(fix_rate: u64, fix_date: u32, valid_until: i64)`

Publishes Banxico's FIX: `fix_rate` is MXN per USD times 10^4, `fix_date` the date it was determined (`yyyymmdd`), within the pool's FIX bounds. The feeder moves forward within 5% a day since the FIX in force, counting one to three days; the council publishes the rest, dated at most five days back.

Signers: `poster`. Signed by `Pool.fx_feeder` or by the council; the council path runs the multisig check.

Rules, in order:

- `fix_date` is a real date, not after today in Mexico City (`FixDateInvalid`).
- The feeder: a date later than `Pool.fix_date` (`FixDateInvalid`), and `|fix_rate − Pool.fix_rate| ≤ Pool.fix_rate × 5% × days`, where `days = fix_date − Pool.fix_date` in calendar days, at least 1 and at most 3 (`fx::MAX_FIX_JUMP_BPS_PER_DAY`, `fx::MAX_FIX_JUMP_DAYS`): a jump of more than 15% always needs the council (`FixJump`). The first FIX is the council's (`FixJump`).
- The council: a date not before `Pool.fix_date` (same-day corrections allowed) and not more than `fx::MAX_FIX_AGE_DAYS` (5) days before today in Mexico City (`FixDateInvalid`); no band.
- The feeder, against the market reference `fx_reference` (RedStone's MXNe price, below): the reference at most `fx::MAX_REFERENCE_AGE_SECONDS` (26 h) old (`FxReferenceStale`) and `|fix_rate − reference| ≤ reference × 2%` (`fx::MAX_FIX_REFERENCE_GAP_BPS`, `FixOffReference`). The council publishes without this check.
- Both: `Pool.min_fix_rate ≤ fix_rate ≤ Pool.max_fix_rate` unless both bounds are zero (`FixOutOfBounds`).

The market reference is RedStone's MXNe price on Solana: account `8QuwResPgdQdja4gdTbvNxeUznc58XX8baUM3cCwfVW2` (`fx::REFERENCE_ACCOUNT`, the PDA `["price", "MXNe"]`, each seed zero-padded to 32 bytes) owned by the adapter `REDSTBDUecGjwXd6YGPzHSvEUBHQqVRfCcjUVgPiHsr` (`fx::REFERENCE_PROGRAM`), which verifies RedStone's signed payload when it writes. The program reads its `PriceData` at fixed offsets (`fx::reference_rate`): discriminator `sha256("account:PriceData")[..8]`, feed id `"MXNe"` at 8..40, the value at 40..72 as a big-endian u256 of USD per MXNe × 10^8, the timestamp in milliseconds at 72..80 (little-endian), and decimals 8; the FIX-scale reference is `10^12 / value`, rounded down. Any other address is refused by the account constraint, any other owner, feed or layout with `FxReferenceInvalid`. `FixPosted` records `reference_rate` and `reference_at` for the feeder and the council (zero when the reference was missing or, for the council, unreadable).

**On mainnet the check is mandatory**: a build without a cluster feature requires the account, owned by the adapter, for every feeder publication. RedStone's account exists on mainnet only, so a `localnet` or `devnet` build skips the check when the account is missing (empty and owned by the system program); an account that exists is checked as on mainnet.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `poster` |  | yes | The feeder or the council. |
| `multisig` |  |  |  |
| `pool` | yes |  |  |
| `fx_reference` |  |  | Fixed address `8QuwResPgdQdja4gdTbvNxeUznc58XX8baUM3cCwfVW2`. RedStone's MXNe price account, the market reference the feeder's FIX is checked against. |

### `pause()`

Pauses deposits, redemptions, requests, approvals and draws. Any member of the council'"'"'s multisig alone, the guardian or the council; repayments and everything else go on.

Signers: `signer`. Signed by any member of the council multisig (voter or not), the guardian, or the council; no `valid_until`. A member is read from `multisig` only while Squads v4 owns the account (`CouncilInvalid` otherwise); the guardian and the council do not need it.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `signer` |  | yes | A member of the council's multisig, the stored guardian, or the council itself. |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

### `resume(valid_until: i64)`

Ends a pause. Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |

## Investors

### `deposit(assets: u64, min_shares_out: u64)`

The depositor pays `assets` for shares at the current NAV, at least `min_shares_out`; the first deposit must meet the minimum and locks `DEAD_SHARES`. Refused while paused.

Signers: `depositor`.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `depositor` |  | yes |  |
| `pool` |  |  |  |
| `asset_mint` |  |  | Read for its decimals (`transfer_checked`). |
| `share_mint` | yes |  |  |
| `vault` | yes |  |  |
| `dead_shares` | yes |  |  |
| `depositor_asset_account` | yes |  |  |
| `depositor_share_account` | yes |  |  |
| `token_program` |  |  | The asset's token program. |
| `share_token_program` |  |  | The share mint's token program. |

### `redeem(shares: u64, min_assets_out: u64)`

The holder burns `shares` for assets at the current NAV, at least `min_assets_out`, up to the vault's balance. Refused while paused.

Signers: `holder`.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `holder` |  | yes |  |
| `pool` |  |  |  |
| `asset_mint` |  |  | Read for its decimals (`transfer_checked`). |
| `share_mint` | yes |  |  |
| `vault` | yes |  |  |
| `holder_share_account` | yes |  |  |
| `holder_asset_account` | yes |  |  |
| `token_program` |  |  | The asset's token program. |
| `share_token_program` |  |  | The share mint's token program. |

## Lines

### `request_line(line_id: u64, terms: LineTerms)`

The borrower requests line `line_id` (the pool's next) on terms `terms`, in centavos of MXN; a separate payer funds the account and the named sponsor co-signs. Stored as `Requested` with the terms hash; refused while paused. Moves no money.

Signers: `borrower`, `payer`, `sponsor (optional)`.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `borrower` |  | yes |  |
| `payer` | yes | yes | Funds the request and receives the rent back on withdraw or reject; never the borrower's concern. |
| `sponsor` |  | yes | Optional. The sponsor named in the terms; required (and signing) when one is named. |
| `pool` | yes |  |  |
| `line` | yes |  |  |
| `system_program` |  |  | Fixed address `11111111111111111111111111111111`. |

`LineTerms`:

| Field | Type | Notes |
|---|---|---|
| `principal` | `u64` | In centavos of `CURRENCY_MXN`. |
| `rate_bps` | `u16` |  |
| `late_bps` | `u16` | Annual late interest on the overdue principal, in basis points, scheduled or bullet. |
| `term_end` | `i64` |  |
| `agreement_hash` | `[u8; 32]` |  |
| `sponsor` | `Option<pubkey>` |  |
| `sponsor_role` | `u8` |  |
| `schedule` | `Option<ScheduleParams>` |  |
| `expediente_root` | `[u8; 32]` |  |
| `pagare_template_hash` | `[u8; 32]` |  |

### `approve_line(expediente_root: [u8; 32], terms_hash: [u8; 32], valid_until: i64)`

The council approves a requested line, naming its expediente root and terms hash, within 15 days of the request and within the line cap at a FIX at most five days old; a curator cannot be the borrower.

The principal must be at most `max_line_bps` of the NAV now (`vault + outstanding_principal`), converted to MXN at the FIX in force (`LineTooLarge`); that FIX must be published and dated at most `fx::MAX_FIX_AGE_DAYS` (5) days before today in Mexico City, the same freshness as a draw (`FixStale`).

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` | yes |  |  |
| `vault` |  |  | Read for the NAV the concentration cap is measured against. |

### `reject_line(valid_until: i64)`

The council rejects a requested line; the rent returns to the payer.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` | yes |  |  |
| `payer` | yes |  |  |

### `withdraw_request()`

The borrower withdraws a requested line, expired or not; the rent returns to the payer.

Signers: `borrower`.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `borrower` |  | yes |  |
| `line` | yes |  |  |
| `payer` | yes |  |  |

### `draw(amount_mxn: u64, fix_rate: u64, fix_date: u32, asset_bump: u8, pagare_bump: u8, cert_fingerprint: [u8; 32], signature_method: u8)`

Pays a tranche of `amount_mxn` centavos to the borrower as USDC at the FIX in force, which the borrower signs as `fix_rate` and `fix_date`, and mints the pagaré (its hash derived on chain from the approved template and terms) as a frozen Core asset owned by the pool, with its `Pagare` record. The borrower and the rent payer sign; refused while paused, once the draw window has closed, on a changed or stale FIX, or above what the line, the line cap (the line's USDC drawn within `max_line_bps` of the NAV now), the vault or the pool cap allows.

The line cap is measured again in USDC at every draw: `line.usdc_drawn + usdc ≤ max_line_bps × (vault + outstanding_principal) / BPS`, with the NAV read before the tranche moves (the NAV a deposit prices at), after the vault check (`LineTooLarge`). A line approved within the cap in MXN can therefore draw less than its principal when the peso strengthens or the NAV falls.

Signers: `borrower`, `payer`.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `borrower` |  | yes | Signs alone: the signature covers the pagaré hash the program derives and the FIX. |
| `pool` | yes |  |  |
| `line` | yes |  |  |
| `asset_mint` |  |  | Read for its decimals (`transfer_checked`). |
| `vault` | yes |  |  |
| `borrower_asset_account` | yes |  |  |
| `token_program` |  |  |  |
| `payer` | yes | yes | Pays the rent of the pagaré asset and its sidecar. Any signer. |
| `asset` | yes |  | The pagaré's Core asset, created here by the Core program. The client passes the bump so the constraint does not search for it. |
| `pagare` | yes |  | The pagaré's record, created by the handler (Anchor's `init` would search for the bump). |
| `collection` | yes |  | The series' collection the pool controls (validated at `init_pool`). |
| `mpl_core_program` |  |  | Fixed address `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d`. |
| `system_program` |  |  | Fixed address `11111111111111111111111111111111`. |

### `repay(max_usdc: u64, fix_rate: u64, fix_date: u32, receipt_hash: [u8; 32])`

Anyone pays up to `max_usdc` on an `Active` or `InCollection` line, credited in MXN at the FIX in force (`fix_rate`, `fix_date`, whatever its age): late interest (which the credit must cover), then interest, then principal, never more than is due; the USDC taken is rounded up. The remaining accounts are exactly the pagaré records `[pagares_paid ..= last_touched]`, oldest first, at most `MAX_SIDECARS_PER_REPAY` of them; each one covered is marked paid with the payer's provisional `receipt_hash`. The line becomes `Repaid` only when nothing is due and nothing can be drawn; while it can draw, a payment on a zero debt is refused. Never paused.

Signers: `payer`.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `payer` |  | yes | Anyone may repay: the borrower, a sponsor paying the interest, or a third party. |
| `pool` | yes |  |  |
| `line` | yes |  |  |
| `asset_mint` |  |  | Read for its decimals (`transfer_checked`). |
| `share_mint` |  |  | Read for the share supply (`NavUpdated` and the invariant check). |
| `vault` | yes |  |  |
| `payer_asset_account` | yes |  |  |
| `token_program` |  |  |  |

### `mark_in_collection()`

Moves a scheduled line to collection once `max_missed` installments are missed (past the due date of the last of them), accelerating the balance. Anyone may call it, also while paused.

Signers: none.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `pool` |  |  |  |
| `line` | yes |  |  |

### `suspend_draws(valid_until: i64)`

Closes the draw window of an `Approved` or `Active` line now, forfeiting the undrawn remainder. Council only. Repayment is untouched.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` | yes |  |  |

### `resume_draws(valid_until: i64)`

Reopens the draw window of a suspended line to its original end. Council only; refused while an acceleration is pending.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` | yes |  |  |

### `cancel_line(valid_until: i64)`

Cancels an `Approved` line nobody drew. Council only; refused once anything was drawn.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` | yes |  |  |

### `propose_acceleration(valid_until: i64)`

Proposes to collect a drawn line early: draws are suspended at once and `accelerate_line` may fire after the pool'"'"'s notice. Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` | yes |  |  |

### `withdraw_acceleration(valid_until: i64)`

Withdraws a pending acceleration; draws stay suspended. Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` | yes |  |  |

### `accelerate_line(valid_until: i64)`

Fires a pending acceleration once its notice has run: the line moves to collection, as with `mark_in_collection`, without missed installments. Council only.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` | yes |  |  |

### `write_off(valid_until: i64)`

Books a line's unpaid principal as a loss. Council only; an `Active` line after `term_end`, an `InCollection` line after its collection period from the mark and after its oldest missed due date or its term. Never paused.

Signers: `council`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `council` |  | yes |  |
| `multisig` |  |  |  |
| `pool` | yes |  |  |
| `line` | yes |  |  |
| `share_mint` |  |  | Read for the share supply (`NavUpdated` and the invariant check). |
| `vault` |  |  | Read for the NAV in the event. |

## Pagarés

### `cancel_pagare(draw_index: u16, receipt_hash: [u8; 32], valid_until: i64)`

Extinguishes pagaré `draw_index` once a repayment has marked it paid (`PAGARE_PAID_PENDING_CANCEL`, whatever the line's status): writes the `Terminacion` entry, then thaws and burns the asset; the record stays. The council (the owner), the procurador of record alone, or the line's borrower alone once the cancel grace fixed at the payment has run. Never paused.

Signers: `holder`, `payer`. The signer picks the branch: the council (multisig check), the procurador of record alone, or the line's borrower alone after the pagaré's `cancel_grace_seconds` (fixed by the payment) from the PAGADO mark.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `holder` |  | yes | The council (the owner while the asset is in the pool), the procurador of record with a procuración in force, or the line's borrower once the cancel grace has run; checked in the handler (`NotHolder`, `ProcuracionState`, `CancelGraceNotElapsed`). |
| `multisig` |  |  |  |
| `payer` | yes | yes | Receives the asset's rent from the Core program. Any signer. |
| `pool` |  |  |  |
| `line` |  |  |  |
| `pagare` | yes |  | The record of pagaré `draw_index` of the line; written, never closed. |
| `asset` | yes |  | The Core asset the record names; burnt by the Core program. |
| `collection` | yes |  | The series' collection the asset belongs to. |
| `procurador_authority` |  |  | Optional. The burn delegate's authority, `["procurador", pagare, holder]`: required on the procurador's branch, where the program signs for it; left out on the owner's. |
| `mpl_core_program` |  |  | Fixed address `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d`. |
| `system_program` |  |  | Fixed address `11111111111111111111111111111111`. |

### `endorse_pagare(draw_index: u16, kind: u8, to: pubkey, endoso_hash: [u8; 32], valid_until: i64)`

Endorses outstanding pagaré `draw_index` en procuración to `to`: appends the entry and lets the program address `["procurador", pagare, to]` burn the asset through an owner-managed `BurnDelegate`. The council (the holder) signs; `kind` must be `KIND_PROCURACION`; refused with `to` the zero key, the pool, the council, a curator, the guardian, the pool's creator, the FIX feeder, or the line's borrower, rent payer or sponsor, or a procuración in force.

Signers: `holder`, `payer`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `holder` |  | yes | The pagaré's holder: the council while the asset is in the pool. |
| `multisig` |  |  |  |
| `payer` | yes | yes | Pays the rent the Core plugin may need. Any signer. |
| `pool` |  |  |  |
| `line` |  |  |  |
| `pagare` | yes |  |  |
| `asset` | yes |  |  |
| `collection` | yes |  |  |
| `mpl_core_program` |  |  | Fixed address `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d`. |
| `system_program` |  |  | Fixed address `11111111111111111111111111111111`. |

### `revoke_procuracion(draw_index: u16, entry_hash: [u8; 32], valid_until: i64)`

Revokes the procuración in force on pagaré `draw_index`: appends the revocation entry over the signed revocation's hash and returns the burn delegate to the owner. The holder (the council) signs.

Signers: `holder`, `payer`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `holder` |  | yes | The pagaré's holder: the council while the asset is in the pool. The owner alone revokes. |
| `multisig` |  |  |  |
| `payer` | yes | yes |  |
| `pool` |  |  |  |
| `line` |  |  |  |
| `pagare` | yes |  |  |
| `asset` | yes |  |  |
| `collection` | yes |  |  |
| `mpl_core_program` |  |  | Fixed address `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d`. |
| `system_program` |  |  | Fixed address `11111111111111111111111111111111`. |

### `court_annotation(draw_index: u16, kind: u8, document_hash: [u8; 32], valid_until: i64)`

Records a court's act on pagaré `draw_index`: `kind` is `KIND_ANOTACION_JUDICIAL` (embargo, suspension, cancellation) or `KIND_CONSTANCIA_JUDICIAL`, over the court document's hash. The council signs; refused on a cancelled pagaré; moves nothing.

Signers: `holder`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `holder` |  | yes | The pagaré's holder: the council while the asset is in the pool. |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` |  |  |  |
| `pagare` | yes |  |  |

### `annotate_payment(draw_index: u16, entry_count: u32, receipt_hash: [u8; 32], valid_until: i64)`

The council (the holder) attaches a receipt to payment entry `entry_count` of pagaré `draw_index`, as a `Recibo` entry over `receipt_hash`. The annotation also replaces the pagaré's provisional `receipt_hash` until the burn (`PAGARE_PAID_PENDING_CANCEL`).

Signers: `annotator`. Council instruction.

| Account | Writable | Signer | Notes |
|---|---|---|---|
| `annotator` |  | yes | The holder: the council while the asset is in the pool. |
| `multisig` |  |  |  |
| `pool` |  |  |  |
| `line` |  |  |  |
| `pagare` | yes |  |  |

## Accounts

### `CreditLine`

| Field | Type | Notes |
|---|---|---|
| `pool` | `pubkey` |  |
| `line_id` | `u64` |  |
| `borrower` | `pubkey` |  |
| `sponsor` | `Option<pubkey>` | Co-signed `request_line`; `None` when the line has no sponsor. |
| `sponsor_role` | `u8` | `SPONSOR_PROMOTER` or `SPONSOR_GUARANTOR`; meaningful only when `sponsor` is named. |
| `agreement_hash` | `[u8; 32]` | Hash of the signed credit agreement. |
| `expediente_root` | `[u8; 32]` | Salted merkle root of the credit file in the agreements engine. A commitment: no document, name or folio is derivable from it. |
| `status` | `LineStatus` |  |
| `principal` | `u64` | The approved commitment; `available = principal - drawn`. |
| `rate_bps` | `u16` | Annual ordinary interest rate, in basis points, charged on each tranche from its draw to the last due date. |
| `term_end` | `i64` | The maturity of a bullet line. |
| `availability_end` | `i64` | Last instant a draw is accepted (strict `<`): `first_due_at` on a scheduled line, `term_end` on a bullet line; `suspend_draws` brings it to now. |
| `drawn` | `u64` | Principal transferred to the borrower so far; `drawn <= principal`, never decreases. |
| `draws` | `u16` | Tranches drawn; at most `MAX_DRAWS`. |
| `interest_due` | `u64` | Ordinary interest owed: each draw adds `amount * rate_bps * (last_due - now) / (BPS * SECONDS_PER_YEAR)`; `mark_in_collection` cuts it to the share of the installments then due. |
| `principal_repaid` | `u64` |  |
| `interest_repaid` | `u64` |  |
| `pagares_paid` | `u16` | The index of the oldest pagaré no repayment has marked paid. `repay` requires the records `[pagares_paid ..= last_touched]` and moves it. |
| `first_due_at` | `i64` | The first due date; installment `k` is due at `first_due_at + (k - 1) * interval_seconds`. Zero on a bullet line. |
| `interval_seconds` | `i64` |  |
| `installments` | `u16` | Installments of the schedule, `1..=MAX_INSTALLMENTS`; zero on a bullet line. |
| `late_bps` | `u16` | Annual late interest rate on the overdue principal, in basis points. |
| `max_missed` | `u8` | Missed installments that allow `mark_in_collection`. |
| `late_collected` | `u64` |  |
| `late_paid_until` | `i64` | Late interest is settled up to this instant; zero until a repayment collects some. |
| `drawn_at` | `i64` | Time of the first draw; informational (the calendar is `first_due_at`). |
| `closed_at` | `i64` |  |
| `accelerated_at` | `i64` | When `mark_in_collection` accelerated the balance; zero before. From then on every installment is due, and `write_off` waits `Pool::collection_period_seconds` from it. |
| `bump` | `u8` |  |
| `currency` | `u16` | ISO 4217 code of every amount of the line: `CURRENCY_MXN`. |
| `requested_at` | `i64` |  |
| `approved_at` | `i64` |  |
| `terms_hash` | `[u8; 32]` | `sha256` of the stored terms in the fixed encoding; the council's approval names it. |
| `pagare_template_hash` | `[u8; 32]` | Hash of the approved pagaré text every draw's pagaré hash is derived from. |
| `payer` | `pubkey` | Funded the request; receives the rent back on withdraw, reject or expiry. |
| `draw_time_weight` | `u128` | `Σ amount × drawn_at` over the draws: a bullet line's interest accrued at a mark is `rate × (drawn × mark − draw_time_weight)`. |
| `availability_end_saved` | `i64` | The `availability_end` before `suspend_draws` or `propose_acceleration` moved it, which `resume_draws` restores; zero when draws are not suspended. Suspended ⇔ non-zero. |
| `acceleration_due_at` | `i64` | When `accelerate_line` may fire, set by `propose_acceleration`; zero when none is pending. `withdraw_acceleration` and `mark_in_collection` clear it, `accelerate_line` keeps it: with `accelerated_at` it tells the route, the council's (non-zero) or the objective one (zero). |
| `usdc_drawn` | `u64` | USDC delivered over the draws: the pool's cost of the line. |
| `usdc_cost_repaid` | `u64` | The cost of the principal repaid, at each pagaré's own FIX; `usdc_drawn - usdc_cost_repaid` is the cost still outstanding. |
| `reserved` | `[u8; 64]` | Zero; for fields added after mainnet. |

### `Pagare`

The program's record of one pagaré, beside the Core asset `draw` mints. It holds no folio and nothing personal, and is never closed: after `cancel_pagare` it stays as the PAGADO mark.

| Field | Type | Notes |
|---|---|---|
| `line` | `pubkey` |  |
| `draw_index` | `u16` | Which tranche of the line, 0-based. |
| `asset` | `pubkey` | The Core asset at `[PAGARE_ASSET, line, draw_index]`. |
| `pagare_hash` | `[u8; 32]` | Hash of the signed pagaré data message, as `draw` received it. |
| `amount_mxn` | `u64` | The tranche this pagaré documents, in centavos of MXN: the sum the pagaré promises. |
| `issued_at` | `i64` |  |
| `due_at` | `i64` | The line's last due date at issuance. |
| `drawn_through` | `u64` | The line's `drawn` after this tranche. Pagarés are covered oldest first, so pagaré k is paid once the line's `principal_repaid` reaches it. |
| `cert_fingerprint` | `[u8; 32]` | `sha256(salt \|\| fingerprint)` of the PSC or e.firma certificate that signed the pagaré text, with the salt kept in the expediente: a bare fingerprint can be looked up to the signer's RFC. Zero only when `signature_method` is `SIGNATURE_WALLET`. |
| `signature_method` | `u8` | `SIGNATURE_*`. |
| `status` | `u8` | `PAGARE_OUTSTANDING`, `PAGARE_PAID_PENDING_CANCEL` or `PAGARE_PAID`; never reverts. |
| `paid_at` | `i64` | When the repayment that covered it landed; zero while outstanding. |
| `receipt_hash` | `[u8; 32]` | The receipt hash of that repayment: the payer's, until the holder annotates the real one (`annotate_payment`, before the burn). |
| `cancel_receipt_hash` | `[u8; 32]` | The receipt hash `cancel_pagare` was given; zero until the burn. |
| `procurador` | `pubkey` | The procurador of the endoso en procuración in force; the zero key when none. Cleared by `revoke_procuracion` and by the extinction. |
| `flags` | `u8` | `PAGARE_FLAG_*` bits. |
| `log_root` | `[u8; 32]` | The registry: `sha256(log_root \|\| entry_hash)` over every entry, the issuance entry first. |
| `entry_count` | `u32` | Entries in the chain; the issuance entry is 1. |
| `last_kind` | `u8` | `KIND_*` of the last entry. |
| `bump` | `u8` |  |
| `fix_rate` | `u64` | The FIX the borrower signed for this draw (`Pool::fix_rate` then) and its date. |
| `fix_date` | `u32` |  |
| `usdc_delivered` | `u64` | USDC base units delivered: `fx::usdc_for_mxn(amount_mxn, fix_rate)`. The pool's cost of the tranche. |
| `delivery_hash` | `[u8; 32]` | `sha256(pagare_hash \|\| fix_rate \|\| fix_date \|\| usdc_delivered)`: the delivery the borrower instructed, kept apart from the pagaré, which promises MXN only. |
| `cancel_grace_seconds` | `u32` | The pool's `cancel_grace_seconds` when the repayment that covered it landed, fixed then: the borrower may cancel alone after `paid_at + cancel_grace_seconds`, whatever the pool sets later. Zero while outstanding. |
| `reserved` | `[u8; 28]` | Zero; for fields added after mainnet. |

### `Pool`

| Field | Type | Notes |
|---|---|---|
| `council` | `pubkey` | The vault of the council multisig: signs every council instruction. |
| `asset_mint` | `pubkey` |  |
| `share_mint` | `pubkey` | The share token mint; its mint authority is the pool PDA. |
| `vault` | `pubkey` | PDA token account `[VAULT, pool]` for `asset_mint`, owned by the pool PDA. |
| `dead_shares` | `pubkey` | PDA share token account `[DEAD, pool]`, owned by the pool PDA; never burned. |
| `collection` | `pubkey` | The series' Metaplex Core collection `draw` mints the pagaré assets into. |
| `outstanding_principal` | `u64` | Principal drawn and neither repaid nor written off. |
| `interest_collected` | `u64` | Ordinary interest repaid on every line; informative. |
| `written_off` | `u64` | Principal written off on every line; informative. |
| `line_count` | `u64` |  |
| `grace_period_seconds` | `u32` | Reserved, always zero: there is no grace period. Late interest runs from the first second after a due date (or the term), and collection and write-offs count from the due dates themselves. `init_pool` writes zero and nothing reads it; the field stays so the layout does not move. |
| `collection_period_seconds` | `u32` | Seconds from the mark (`CreditLine::accelerated_at`) before a line in collection can be written off; at least `MIN_COLLECTION_PERIOD_SECONDS`. Set with `set_collection_period`. |
| `max_line_bps` | `u16` | The line cap, as a share of the NAV in basis points (`1..=BPS`): `approve_line` measures the principal (in MXN at the FIX) against the NAV at the approval, and every `draw` the line's USDC drawn, the tranche included, against the NAV at the draw. Set at `init_pool` and with `set_max_line_bps`. |
| `asset_controls` | `u8` | `ASSET_CONTROL_*` bits: issuer controls of the asset mint that can stop the vault without the program. Recorded at `init_pool`, never refused. |
| `permissioned` | `bool` | The share mint is `DefaultAccountState = Frozen`: a holder's account must be thawed by the mint's freeze authority before it can receive or burn shares. Recorded at `init_pool`. |
| `paused` | `bool` |  |
| `bump` | `u8` |  |
| `creator` | `pubkey` | Paid for and seeds the pool; no other power. |
| `council_kind` | `u8` | `COUNCIL_SQUADS` or `COUNCIL_GENERIC`. |
| `multisig` | `pubkey` | The Squads multisig whose vault is the council; the zero key under `COUNCIL_GENERIC`. |
| `council_vault_bump` | `u8` | The bump of the council's vault PDA `["multisig", multisig, "vault", 0]`. |
| `pending_council` | `pubkey` | The authority `propose_council` named, until it signs `accept_council`; the zero key when none. |
| `pending_council_kind` | `u8` |  |
| `guardian` | `pubkey` | A key that may only pause the pool, beside every council member; the zero key when none. |
| `max_outstanding_principal` | `u64` | The most principal the pool lends at once, in asset units; checked at every draw. |
| `acceleration_notice_seconds` | `u32` | Seconds from `propose_acceleration` before `accelerate_line`; at least `MIN_ACCELERATION_NOTICE_SECONDS`. A proposal stores its own deadline. |
| `cancel_grace_seconds` | `u32` | Seconds from a pagaré's payment before its borrower may cancel it alone, `MIN_CANCEL_GRACE_SECONDS..=MAX_CANCEL_GRACE_SECONDS`; each repayment fixes it on the pagarés it marks paid. |
| `impaired_principal` | `u64` | Principal of the lines in collection, for a provision at the mark; zero, not yet used. |
| `provision_bps` | `u16` | The provision on `impaired_principal`, in basis points; zero, not yet used. |
| `fx_feeder` | `pubkey` | The key that may publish the FIX within the band of `fx::within_band` from the one in force; the zero key when none (the council publishes). Set and rotated by the council. |
| `fix_rate` | `u64` | The FIX in force: MXN per USD times `fx::FIX_SCALE`; zero before the first publication. |
| `fix_date` | `u32` | The date Banxico determined `fix_rate`, `yyyymmdd`; never moves back. |
| `fix_posted_at` | `i64` |  |
| `fx_realized` | `i64` | The FX gain (positive) or loss realized by repayments, in USDC base units; informative. |
| `min_fix_rate` | `u64` | The lowest and highest FIX `post_fix` accepts from anyone, feeder or council; zero and zero for no bounds. Set at `init_pool` and with `set_fix_bounds`. |
| `max_fix_rate` | `u64` |  |
| `reserved` | `[u8; 48]` | Zero; for fields added after mainnet. |

## Events

- **`AccelerationNoticeSet`** `{ pool: pubkey, acceleration_notice_seconds: u32 }`
- **`AccelerationProposed`** `{ pool: pubkey, line_id: u64, due_at: i64 }`: The council proposed to collect the line early; draws are suspended at once, and `accelerate_line` may fire from `due_at`. The notice is addressed to the borrower.
- **`AccelerationWithdrawn`** `{ pool: pubkey, line_id: u64 }`: The proposal is withdrawn; draws stay suspended until `resume_draws`.
- **`CancelGraceSet`** `{ pool: pubkey, cancel_grace_seconds: u32 }`
- **`CollectionPeriodSet`** `{ pool: pubkey, collection_period_seconds: u32 }`: The pool's collection period after `set_collection_period`: seconds from the mark before a line in collection can be written off.
- **`CouncilAccepted`** `{ pool: pubkey, old: pubkey, new: pubkey, kind: u8 }`
- **`CouncilProposed`** `{ pool: pubkey, council: pubkey, kind: u8 }`: `council` must sign `accept_council` before it becomes the pool's council.
- **`Deposited`** `{ pool: pubkey, depositor: pubkey, assets: u64, shares: u64, nav_after: u64, supply_after: u64 }`
- **`FixBoundsSet`** `{ pool: pubkey, min_fix_rate: u64, max_fix_rate: u64 }`: The FIX bounds after `set_fix_bounds`: `post_fix` accepts `min_fix_rate ..= max_fix_rate`, or any FIX when both are zero.
- **`FixPosted`** `{ pool: pubkey, fix_rate: u64, fix_date: u32, previous_rate: u64, by: pubkey, slot: u64, posted_at: i64, reference_rate: u64, reference_at: i64 }`: A FIX publication: Banxico's rate for `fix_date` as published by `by` (the feeder or the council) at `slot`. Each one can be checked against the DOF. `reference_rate` (MXN per USD × 10^4) and `reference_at` are the market reference at the publication; zero and zero when it was missing (localnet, devnet) or, for the council, unreadable.
- **`FxFeederSet`** `{ pool: pubkey, fx_feeder: pubkey }`: The zero key means no feeder: only the council publishes.
- **`FxRealized`** `{ pool: pubkey, line_id: u64, receipt_hash: [u8; 32], cost: u64, received: u64, delta: i64, nav_after: u64 }`: The FX result of one repayment's principal, in USDC base units: `cost` is the pool's cost of the principal repaid (at each pagaré's FIX), `received` the USDC that paid it (its centavos at the FIX of the payment, rounded up), `delta = received - cost`. Emitted by `repay` before `LineRepaid`, with the same `receipt_hash`, whenever principal is applied; the deltas add up to `Pool.fx_realized`.
- **`GuardianSet`** `{ pool: pubkey, guardian: pubkey }`: The zero key means no guardian.
- **`LateInterestCollected`** `{ pool: pubkey, line_id: u64, amount: u64 }`: Late interest collected by a repayment of a scheduled line; it stays in the pool and raises the NAV like interest. Emitted before the `LineRepaid` of the same repayment.
- **`LineAccelerated`** `{ pool: pubkey, line_id: u64, remaining: u64, interest_forgiven: u64 }`: The council accelerated the line after the notice: the same effect as `LineInCollection`, without missed installments.
- **`LineApproved`** `{ pool: pubkey, line_id: u64, expediente_root: [u8; 32], terms_hash: [u8; 32], availability_end: i64 }`: The council approved exactly this expediente and these terms.
- **`LineCancelled`** `{ pool: pubkey, line_id: u64 }`
- **`LineDrawn`** `{ pool: pubkey, line_id: u64, draw_index: u16, amount_mxn: u64, fix_rate: u64, fix_date: u32, usdc_delivered: u64, drawn: u64, available: u64, interest_due: u64, availability_end: i64, outstanding_after: u64 }`: One tranche of `amount_mxn` centavos delivered as `usdc_delivered` at the FIX (`fix_rate`, `fix_date`), and the line after it; `outstanding_after` is the pool's cost in USDC.
- **`LineDrawsResumed`** `{ pool: pubkey, line_id: u64, availability_end: i64 }`: Draws resumed with the window they had before the suspension.
- **`LineDrawsSuspended`** `{ pool: pubkey, line_id: u64, available_forfeited: u64 }`: `available_forfeited` is the undrawn remainder.
- **`LineInCollection`** `{ pool: pubkey, line_id: u64, missed: u16, remaining: u64, interest_forgiven: u64 }`: A scheduled line moved to collection and its balance accelerated. `remaining` is the principal and interest still due after the acceleration, without late interest; `interest_forgiven` is the ordinary interest of the installments not yet due at the mark, which the line no longer owes.
- **`LineRejected`** `{ pool: pubkey, line_id: u64 }`
- **`LineRepaid`** `{ pool: pubkey, line_id: u64, payer: pubkey, receipt_hash: [u8; 32], usdc_received: u64, fix_rate: u64, principal_part: u64, interest_part: u64, remaining: u64, nav_after: u64 }`: A repayment: `principal_part` and `interest_part` in centavos of MXN, `usdc_received` the USDC taken at `fix_rate`, `remaining` the MXN still due. Emitted after `FxRealized`, with the same `receipt_hash`.
- **`LineRequested`** `{ pool: pubkey, line_id: u64, borrower: pubkey, payer: pubkey, sponsor: Option<pubkey>, sponsor_role: u8, agreement_hash: [u8; 32], expediente_root: [u8; 32], pagare_template_hash: [u8; 32], terms_hash: [u8; 32], currency: u16, principal: u64, rate_bps: u16, term_end: i64, first_due_at: i64, interval_seconds: i64, installments: u16, late_bps: u16, max_missed: u8 }`: Every term `request_line` stored, with the hash of their fixed encoding. On a bullet line the schedule fields other than `late_bps` are zero; `payer` funded the request and gets its rent back.
- **`LineWrittenOff`** `{ pool: pubkey, line_id: u64, loss: u64, loss_usdc: u64, nav_after: u64 }`: The principal left in MXN (`loss`) and its cost in USDC (`loss_usdc`), which leaves the NAV.
- **`MaxLineBpsSet`** `{ pool: pubkey, max_line_bps: u16 }`
- **`NavUpdated`** `{ pool: pubkey, total_assets: u64, share_supply: u64 }`: The NAV and the share supply after every instruction that moves either (`deposit`, `redeem`, `repay`, `write_off`), read back after the CPIs: `price per share = total_assets / share_supply`.
- **`OutstandingCapSet`** `{ pool: pubkey, max_outstanding_principal: u64 }`
- **`PagareCancelled`** `{ pool: pubkey, line_id: u64, draw_index: u16, asset: pubkey, receipt_hash: [u8; 32], holder: pubkey, by_borrower: bool }`: Emitted by `cancel_pagare`, before the burn. `holder` is the key that extinguished the pagaré (the council, the procurador or the borrower); the sidecar stays as the PAGADO mark.
- **`PagareEntry`** `{ pool: pubkey, line_id: u64, draw_index: u16, asset: pubkey, kind: u8, from: pubkey, to: pubkey, entry_hash: [u8; 32], entry_count: u32, recorded_at: i64, stated_at: i64 }`: One entry of a pagaré's registry: the full content the sidecar's hash chain commits to. `stated_at` is the date the entry states and `recorded_at` the slot's time; every instruction writes both as the slot's time.
- **`PagareIssued`** `{ pool: pubkey, line_id: u64, draw_index: u16, asset: pubkey, pagare_hash: [u8; 32], amount_mxn: u64, delivery_hash: [u8; 32], due_at: i64, holder: pubkey }`: Emitted by `draw` after `LineDrawn`: the pagaré issued as the Metaplex Core asset `asset`, owned by `holder` (the pool PDA) and frozen. `due_at` is the line's last due date at issuance.
- **`PagarePayment`** `{ pool: pubkey, line_id: u64, payer: pubkey, receipt_hash: [u8; 32], late_part: u64, interest_part: u64, principal_part: u64, entries: Vec<PaymentEntry> }`: The payment entries of one `repay`: the payer's receipt hash (zero means "receipt to follow"), the payment's split and one `PaymentEntry` per sidecar touched, oldest first. Emitted after the `PagareEntry` of each entry.
- **`PagareReceiptAnnotated`** `{ pool: pubkey, line_id: u64, draw_index: u16, entry_count: u32, payment_entry_count: u32, receipt_hash: [u8; 32] }`: A `Recibo` entry written by `annotate_payment`: the receipt attached to the payment entry `payment_entry_count` of the pagaré's chain, as entry `entry_count`.
- **`PoolInitialized`** `{ pool: pubkey, council: pubkey, creator: pubkey, multisig: pubkey, guardian: pubkey, asset_mint: pubkey, share_mint: pubkey, collection: pubkey, collection_period_seconds: u32, max_line_bps: u16, max_outstanding_principal: u64, acceleration_notice_seconds: u32, cancel_grace_seconds: u32, asset_controls: u8, permissioned: bool, min_fix_rate: u64, max_fix_rate: u64 }`: Every parameter `init_pool` recorded. `asset_controls` are the `ASSET_CONTROL_*` bits; `permissioned`: the share mint is frozen by default.
- **`PoolPaused`** `{ pool: pubkey, by: pubkey }`
- **`PoolResumed`** `{ pool: pubkey }`
- **`Redeemed`** `{ pool: pubkey, holder: pubkey, shares: u64, assets: u64, nav_after: u64, supply_after: u64 }`
- **`RequestWithdrawn`** `{ pool: pubkey, line_id: u64 }`

## Errors

| Code | Name | Message |
|---|---|---|
| 6000 | `Unauthorized` | This signer may not do this |
| 6001 | `SponsorRequired` | The sponsor named on this line must sign |
| 6002 | `NotHolder` | The pagaré's holder must sign |
| 6003 | `Paused` | The pool is paused |
| 6004 | `CouncilInvalid` | The council is not the vault of an autonomous Squads v4 multisig with a majority threshold |
| 6005 | `CouncilTooLarge` | The council multisig has more members than the program checks |
| 6006 | `CouncilKindInvalid` | This council kind is not accepted |
| 6007 | `NoPendingCouncil` | No council rotation is pending, or this signer is not the one proposed |
| 6008 | `ProposalExpired` | The council's decision is past its valid_until |
| 6009 | `WrongAccount` | Wrong mint or token account |
| 6010 | `AssetMintNotAllowed` | The asset mint carries a Token-2022 extension the pool does not accept |
| 6011 | `ShareMintNotAllowed` | The share mint is not one this pool can own and price: wrong authority, supply, decimals or extensions |
| 6012 | `AssetNotListed` | The asset mint is not on this cluster's allow-list |
| 6013 | `AssetIsShareMint` | The asset mint and the share mint are the same |
| 6014 | `CollectionNotAllowed` | The collection is not a bare Metaplex Core collection whose update authority is the pool |
| 6015 | `PagareAccountsMismatch` | The pagaré records passed are not the ones this payment touches, oldest first |
| 6016 | `BorrowerIsCouncil` | The council cannot be the borrower |
| 6017 | `BorrowerIsCurator` | A member of the council cannot be the borrower |
| 6018 | `EndorseeInvalid` | The endorsee is missing, or is the pool, the council, a curator, the guardian, the creator, the FIX feeder, or the line's borrower, payer or sponsor |
| 6019 | `WrongAmount` | Amount is zero or exceeds what is due |
| 6020 | `Overflow` | Arithmetic overflow |
| 6021 | `InvariantBroken` | A pool invariant does not hold after this instruction; the transaction is refused |
| 6022 | `InsufficientLiquidity` | Not enough liquidity in the vault |
| 6023 | `ZeroShares` | The deposit would mint zero shares |
| 6024 | `FirstDepositTooSmall` | First deposit below the minimum |
| 6025 | `SlippageExceeded` | The result is below the minimum the caller accepts |
| 6026 | `LineTooLarge` | The line exceeds max_line_bps of the NAV: its principal at the approval, or its USDC drawn at a draw |
| 6027 | `OutstandingCapExceeded` | The draw would take the pool's outstanding principal above its cap |
| 6028 | `CollectionPeriodTooShort` | The collection period is shorter than the minimum |
| 6029 | `AccelerationNoticeTooShort` | The acceleration notice is below the seven-day floor |
| 6030 | `CancelGraceTooShort` | The borrower's cancel grace is below the thirty-day floor |
| 6031 | `LineStatus` | The line is not in the required status |
| 6032 | `AgreementHashRequired` | The signed agreement's hash is required |
| 6033 | `ExpedienteRootRequired` | The expediente root is required |
| 6034 | `PagareTemplateHashRequired` | The pagaré template's hash is required |
| 6035 | `ExpedienteMismatch` | The expediente root or the terms hash is not the one the borrower requested |
| 6036 | `RequestExpired` | The request is older than the council's review window |
| 6037 | `SponsorRoleInvalid` | sponsor_role is unknown |
| 6038 | `TermEnded` | The term has ended, or would end before the opening |
| 6039 | `InvalidSchedule` | The installment schedule is not valid |
| 6040 | `MaxMissedTooLow` | max_missed is below the floor: at least one missed installment must come before the collection trigger |
| 6041 | `ScheduleEndsAfterTerm` | The last installment would fall due after the term |
| 6042 | `NotScheduled` | The line has no installment schedule |
| 6043 | `AvailabilityEnded` | The draw window of this line has closed |
| 6044 | `TooManyDraws` | The line has reached its maximum number of draws |
| 6045 | `DrawsNotSuspended` | The line's draws are not suspended |
| 6046 | `WindowClosed` | The line's draw window has already closed |
| 6047 | `AccelerationState` | No acceleration is pending on this line, or one already is |
| 6048 | `AccelerationNotDue` | The acceleration notice has not run |
| 6049 | `LateInterestUnpaid` | The repayment must cover the late interest due |
| 6050 | `NotEnoughMissed` | Fewer installments missed than the line allows before collection |
| 6051 | `TermNotEnded` | The term has not ended |
| 6052 | `WriteOffTooEarly` | A line in collection is written off only after its collection period, and after its oldest missed due date or its term |
| 6053 | `FixDateInvalid` | The FIX date is not a valid date, is in the future, or is older than the published one |
| 6054 | `FixJump` | The FIX moves more than the feeder may publish; the council must |
| 6055 | `FixMismatch` | The FIX signed is not the one published now |
| 6056 | `FixStale` | No FIX is published, or it is too old for a draw or an approval |
| 6057 | `DocumentHashRequired` | The signed document's hash is required |
| 6058 | `SignatureMethodInvalid` | signature_method is unknown, or lacks its certificate fingerprint |
| 6059 | `PagareNotCovered` | No repayment has marked this pagaré paid |
| 6060 | `PagareAlreadyPaid` | This pagaré is already extinguished |
| 6061 | `CancelGraceNotElapsed` | The borrower may cancel alone only after the cancel grace from the payment |
| 6062 | `EndosoKindInvalid` | This kind of endoso is not one the program records |
| 6063 | `ProcuracionState` | The pagaré has no procuración in force, or already has one |
| 6064 | `AnnotationKindInvalid` | This kind is not a court annotation |
| 6065 | `EntryNotFound` | The referenced registry entry does not exist |
| 6066 | `TooManySidecars` | A repayment takes at most MAX_SIDECARS_PER_REPAY pagaré records; split the payment |
| 6067 | `CancelGraceTooLong` | The borrower's cancel grace is above the one-year ceiling |
| 6068 | `ProposalTooFar` | The council's decision is dated more than fifteen days ahead |
| 6069 | `FixOutOfBounds` | The FIX is outside the pool's bounds |
| 6070 | `FxReferenceInvalid` | The FX reference account is not RedStone's MXNe price |
| 6071 | `FxReferenceStale` | The FX reference is too old for the feeder to publish against |
| 6072 | `FixOffReference` | The FIX is too far from the FX reference for the feeder; the council must publish it |

## Compute

Measured by `programs/elote_pool/tests/compute.rs` (the program alone) and `tests/squads.rs` (the whole Squads `vault_transaction_execute` transaction). The rule is 100,000 CU per transaction, Squads included. `init_pool` and `draw` vary with the PDA bump searches (1,500 CU per failed try); `compute.rs` pins `init_pool` under 70,000 and `draw` at or below 90,000 (`DRAW_CEILING`, 10,000 below the general ceiling: the Metaplex Core CPI is fixed by the plugin set).

| Case | CU |
|---|---|
| accelerate_line at 120 installments, all due | 49251 |
| annotate_payment at 120 installments | 20364 |
| approve_line (with the multisig check) | 15271 |
| approve_line through Squads | 41411 |
| cancel_pagare (council) through Squads | 83924 |
| cancel_pagare at 120 installments | 51805 |
| cancel_pagare by the borrower at 120 installments | 51398 |
| cancel_pagare by the procurador at 120 installments | 53516 |
| court_annotation at 120 installments | 17686 |
| draw, first tranche at 120 installments | 82336 |
| draw, second tranche at 120 installments | 82510 |
| draw, tranche 10 at 120 installments | 82523 |
| endorse_pagare (procuracion, plugin added) at 120 installments | 38403 |
| endorse_pagare (procuracion, plugin approved) at 120 installments | 44988 |
| endorse_pagare through Squads | 70198 |
| init_pool, classic asset and permissioned shares | 40921 |
| init_pool, classic asset and plain shares | 36887 |
| mark_in_collection at 120 installments | 36725 |
| mark_in_collection at 120 installments, 3 missed | 13708 |
| mark_in_collection at 18 installments | 16715 |
| pause by a member | 9418 |
| post_fix, by the council | 12968 |
| post_fix, by the feeder | 11541 |
| post_fix, by the feeder against the reference | 11828 |
| propose_acceleration | 14585 |
| propose_council through Squads | 34950 |
| repay after the mark at 120 installments | 57990 |
| repay at 120 installments | 68364 |
| repay at 18 installments | 39480 |
| repay covering three sidecars at 120 installments, all missed | 89946 |
| request_line at 120 installments | 20798 |
| resume_draws | 13888 |
| revoke_procuracion at 120 installments | 40088 |
| suspend_draws | 14571 |
| write_off after the mark at 120 installments | 55323 |
| write_off at 120 installments | 55322 |
| write_off at 18 installments | 26558 |
