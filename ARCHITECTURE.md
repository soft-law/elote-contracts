# `elote_pool` architecture

How the program is built, instruction by instruction, byte by byte. The [README](README.md) says
what the pool does for investors and borrowers; this file says how the code does it, for whoever
has to change it, audit it or write a client against it. Source of truth is
`programs/elote_pool/src/`; where this file and the code disagree, the code wins and this file is
the bug.

State of the code: `main` as of 2026-10-20 plus the Phase 1 hardening branch of 2026-10-31
(program id `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`, Anchor 1.2.1, Solana 4.1.2, classic
SPL Token only, 82 tests).

## 0. One program, not two: where the loan contract lives

There is no separate "loan program". The programmable loan contract the README talks about is
the `CreditLine` account, and the code that executes it is `instructions/lines.rs`. The fund is
the `Pool` account plus `instructions/pool.rs`. Both halves live in `elote_pool` because they
share one thing that cannot be split: the money.

```
 off chain                                   on chain (elote_pool)
 ─────────                                   ─────────────────────
 agreements engine (softlaw-backend,         open_line(agreement_hash, terms, schedule)
 formerly elote-baby/mcp):                        │  admin + borrower (+ sponsor) sign
   credit agreement template 0.1               ▼
   (apertura de crédito, LGTOC 291-301)    CreditLine PDA  ◄── the programmable loan contract
   parties sign the JSON document               │
   keccak256(utf8(JSON)) = agreement_hash       │ draw / repay / mark_in_collection / write_off
                                                ▼
                                           Pool PDA + vault  ◄── the fund (NAV, shares)
 web app reads the hash back from the
 CreditLine and asks the API
 GET /api/agreements/by-hash/:hash
 for the signed document's summary
```

- **The legal text stays off chain.** The program stores 32 bytes, checks only that they are not
  zero, and never interprets them. What it enforces are the *economic* terms copied into the
  line at `open_line`: principal, fixed rate, term, and optionally the installment schedule,
  late rate and collection threshold. The hash is the integrity link between the signed document
  and the line; anyone can recompute it from the document and compare.
- **The intersection is three places:** `open_line` (the contract becomes a line; the borrower's
  signature on the transaction is what binds the wallet to the hash), `Pool.outstanding_principal`
  (every loan event moves this field, which is half of the NAV), and `pool_signer!` (the pool PDA
  is the only signer that can pay a `draw`, so a line can only ever spend the vault of its own
  pool).
- **Why not two programs.** A lending program that is separate from the fund would have to be
  trusted by the fund to move vault money through a CPI, which is the exact privilege a split is
  supposed to isolate; the admin already approves each line, so the privilege boundary is between
  signers (admin, borrower, anyone), not between programs. One program keeps each loan event and
  its NAV effect in one atomic state transition under one invariant, one IDL, one upgrade
  authority and one audit surface, and avoids the CPI depth, trace-length and account-lock
  budgets that composing programs spends. The sibling `softlaw` program
  (`F7omfV66XSjsYm1VVVCPyhnnnW72ZYUtqwZPLJWdS82G`, repo `soft-law/softlaw-solana`) made the same
  call: one program replaced five coupled contracts, and its IP-backed credit facilities sit in
  the same program as the marketplace because the repayment waterfall needs the sale proceeds.
  `elote_pool` does not call that program, and that program does not know about `elote_pool`.
- **When a split would earn its place:** a second product borrowing from the pool (the pool would
  then expose a lending interface), loan servicing that must upgrade independently of the money
  logic, or one loan funded by several pools (the loan becomes an account of its own program and
  pools hold positions). None of these exists today.

## 1. One program, five modules

```
programs/elote_pool/src/
├── lib.rs            declare_id!, security.txt, the #[program] module: nine thin entry points
├── state.rs          Pool and CreditLine layouts, PDA seeds, constants, Schedule packing
├── errors.rs         PoolError (19 codes, 6000..6018)
├── events.rs         12 events, all emitted through emit_cpi!
├── math.rs           pure arithmetic: share pricing, installments, late interest, arrears
└── instructions/
    ├── pool.rs       init_pool, deposit, redeem, set_paused, and the pool_signer! macro
    └── lines.rs      open_line, draw, repay, mark_in_collection, write_off
```

`math.rs` has no Anchor dependency and no I/O: every function takes integers and returns
`Option` (`None` on overflow or division by zero), and the handlers turn `None` into
`PoolError::Overflow`. That split is what makes the arithmetic testable with plain `cargo test`
and property tests (`tests/math_props.rs`) without a runtime.

The entry points in `lib.rs` do nothing but forward to `instructions::*`. Every instruction
struct carries `#[event_cpi]`, so each one takes two extra accounts (the program's event
authority PDA and the program itself) and emits its events as self-CPIs, which indexers read from
the inner instructions instead of from logs that can be truncated.

## 2. Accounts and the PDA tree

Every account the program owns or controls hangs off the pool PDA. The user-chosen inputs are
the admin key, the asset mint and, since pool v2, the share mint (created by the admin and
validated at `init_pool`).

```
Pool            PDA ["pool", admin, asset_mint]          data account, program-owned
├── share_mint  admin-supplied mint (any address)        classic or Token-2022, decimals = asset decimals,
│                                                        mint authority = pool, freeze authority = the
│                                                        admin's choice (Token ACL MintConfig in production)
├── vault       PDA ["vault", pool]                      token account of asset_mint, owner = pool
├── dead_shares PDA ["dead", pool]                       token account of share_mint, owner = pool
└── CreditLine  PDA ["line", pool, line_id as u64 LE]    one per approved line, program-owned
```

- The pool address is a function of `(admin, asset_mint)`: one admin can run one pool per asset,
  and the admin can never change (it is part of the address).
- The vault and the dead-shares account are PDAs, not associated token accounts, so nobody can
  create them ahead of `init_pool` and block the pool.
- `line_id` must equal `pool.line_count` at opening, so lines are a dense sequence `0, 1, 2…`
  and a client can enumerate them without an indexer.
- The pool PDA signs every outbound CPI (minting shares, paying a draw, paying a redemption) with
  `pool_signer!(pool)`, which rebuilds the seeds `["pool", admin, asset_mint, bump]` from the
  account's own fields.

### 2.1 `Pool` (299 bytes; 235 before pool v2)

Offsets include the 8-byte Anchor discriminator, so they can be used directly in `memcmp`
filters.

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 1 | `version` | `POOL_VERSION = 1` |
| 9 | 32 | `admin` | signer of `init_pool`; part of the PDA seeds |
| 41 | 32 | `asset_mint` | part of the PDA seeds |
| 73 | 32 | `vault` | |
| 105 | 32 | `share_mint` | |
| 137 | 32 | `dead_shares` | |
| 169 | 8 | `outstanding_principal` | principal drawn and neither repaid nor written off |
| 177 | 8 | `interest_collected` | informative running total (ordinary interest only) |
| 185 | 8 | `written_off` | informative running total |
| 193 | 8 | `line_count` | next `line_id` |
| 201 | 1 | `paused` | |
| 202 | 32 | `reserved` | bytes 0..4: `grace_period_seconds` (u32 LE); bytes 4..6: `max_line_bps` (u16 LE); byte 6: `permissioned` flag; byte 7: `asset_controls` bits (0 freeze authority, 1 Pausable); rest zero |
| 234 | 1 | `bump` | |
| 235 | 32 | `attestor` | pool v2: the agreements engine's key; co-signs openings, draws and endosos; rotated by `set_attestor` |
| 267 | 32 | `collection` | pool v2: the series' Metaplex Core collection the pagaré assets are minted into; validated from step 6 |

`grace_period_seconds` and `max_line_bps` live inside `reserved` so the layout did not change
when they were added. They are read and written only through `Pool::grace_period_seconds()` /
`set_grace_period_seconds()` and `Pool::max_line_bps()` / `set_max_line_bps()`
(`max_line_principal(nav)` applies the cap), and the JS client mirrors the reads in
`clients/js/src/schedule.ts` (`poolGracePeriodSeconds`, `poolMaxLineBps`, `maxLinePrincipal`).

### 2.2 `CreditLine` (246 bytes)

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 1 | `version` | 1 single maturity, 2 installment schedule |
| 9 | 32 | `pool` | |
| 41 | 8 | `line_id` | |
| 49 | 32 | `borrower` | co-signer of `open_line`; the only key allowed to `draw` |
| 81 | 33 | `sponsor` | `Option<Pubkey>`: 1 tag byte + 32 |
| 114 | 32 | `agreement_hash` | hash of the signed credit agreement; never zero |
| 146 | 8 | `principal` | |
| 154 | 2 | `rate_bps` | ≤ 10 000 |
| 156 | 8 | `interest_due` | `principal * rate_bps / 10 000`, fixed at opening |
| 164 | 8 | `term_end` | unix seconds |
| 172 | 8 | `principal_repaid` | |
| 180 | 8 | `interest_repaid` | |
| 188 | 1 | `status` | `LineStatus` |
| 189 | 8 | `opened_at` | |
| 197 | 8 | `drawn_at` | 0 until drawn; the anchor of every due date |
| 205 | 8 | `closed_at` | set by the repayment that clears the line or by `write_off` |
| 213 | 32 | `reserved` | zero on version 1; the packed `Schedule` on version 2 |
| 245 | 1 | `bump` | |

`Schedule` packing inside `reserved` (version 2 only):

| bytes | field | type |
|---|---|---|
| 0..8 | `interval_seconds` | i64 LE |
| 8..10 | `installments` | u16 LE, 1..=120 |
| 10..12 | `late_bps` | u16 LE, ≤ 10 000 |
| 12 | `max_missed` | u8, 3..=installments |
| 13 | `flags` | reserved, zero |
| 14..22 | `late_collected` | u64 LE |
| 22..30 | `late_paid_until` | i64 LE |
| 30..32 | unused | zero |

The first four fields are the terms and never change after `open_line`; `late_collected` and
`late_paid_until` are the only mutable bytes and only `repay` writes them. `CreditLine::schedule()`
returns `None` on a version-1 line, which is how every handler tells the two kinds apart.

### 2.3 Constants (`state.rs`)

| constant | value | role |
|---|---|---|
| `min_first_deposit(decimals)` | `max(10^decimals, 1 000 × DEAD_SHARES)` | floor on the first deposit: one whole unit of the asset, never below 1 000 000 base units |
| `DEAD_SHARES` | 1 000 | shares locked forever by the first deposit |
| `MIN_FIRST_DEPOSIT_DEAD_RATIO` | 1 000 | the first deposit is at least this many dead shares, whatever the decimals |
| `BPS` | 10 000 | basis points |
| `MAX_INSTALLMENTS` | 120 | bounds the arrears loop and its compute |
| `MIN_MAX_MISSED` | 3 | policy: three missed installments before collection |
| `SECONDS_PER_YEAR` | 31 536 000 | 365 days, annualises `late_bps` |

## 3. The money model

```
NAV   = vault.amount + pool.outstanding_principal
price = NAV / share_mint.supply
```

- **Deposit** mints `assets * supply / NAV` shares (one-to-one while `supply == 0`).
- **Redeem** pays `shares * NAV / supply` assets.
- Both products go through `u128` and both divisions round down, so rounding always favours the
  pool, never the caller (`math.rs`, `tests/math_props.rs`).
- Nothing accrues and there is no oracle. The NAV moves only when tokens actually move or when a
  loss is recognised:

| event | vault | outstanding_principal | NAV | price |
|---|---|---|---|---|
| deposit | + | | + | same (shares minted pro rata) |
| redeem | − | | − | same (shares burned pro rata) |
| draw | − principal | + principal | same | same |
| repay principal | + | − | same | same |
| repay interest or late interest | + | | + | **up** |
| write_off | | − unpaid principal | − | **down** |

`outstanding_principal` is touched in exactly three places: `draw` adds the principal, `repay`
subtracts `principal_part`, `write_off` subtracts the unpaid principal. Interest never enters it.

**Dead shares.** The first deposit must be at least `min_first_deposit(decimals)` and `DEAD_SHARES` of the
shares it mints go to the pool-owned `dead_shares` account, which no instruction ever reads
again. The supply therefore never falls below 1 000, so a donation to the vault cannot push the
price high enough to round a later deposit down to zero shares (the classic ERC-4626 inflation
attack; `tests/pool.rs::dead_shares_stop_redeem_then_donate_inflation`).

**Slippage floors.** `deposit` takes `min_shares_out` and `redeem` takes `min_assets_out`,
because the price can move between the client's quote and execution (a repayment or a write-off
can land first).

## 4. Status machine

```
                open_line            draw                 repay (last unit)
     (none) ───────────────► Approved ─────────► Active ───────────────────► Repaid
                                                   │  ▲
                            mark_in_collection     │  │ repay keeps the status
                            (v2 only, anyone)      ▼  │
                                              InCollection ─── repay (last unit) ──► Repaid
                                                   │
                     write_off (admin) ◄───────────┘        write_off (admin) ◄── Active
                           │
                           ▼
                       WrittenOff
```

- `Approved` lines hold no money; an undrawn line past `term_end` simply can never be drawn.
- `Repaid` and `WrittenOff` are terminal. No instruction closes or reopens an account.
- A payment that cures the arrears of a line in collection leaves it `InCollection`; only full
  repayment moves it to `Repaid`. A cured line cannot be written off until it misses again
  (`tests/schedule.rs::a_cured_line_in_collection_is_not_written_off_until_it_misses_again`).

**Pause matrix.** `set_paused(true)` blocks `deposit`, `redeem`, `open_line` and `draw`. It never
blocks `repay`, `mark_in_collection`, `write_off` or `set_paused(false)`: money can always come
back in and facts can always be recorded, but nothing leaves the vault while paused.

## 5. Instructions in detail

For each instruction: who signs, what the account constraints already guarantee before the
handler runs, the checks in the order the handler makes them, what it writes, which CPIs it
makes and which events it emits. Error codes are `PoolError` variants (6000 + index in
`errors.rs`).

### 5.1 `init_pool(grace_period_seconds: u32, max_line_bps: u16, attestor: Pubkey)`

- **Signers:** `admin` (also pays rent).
- **Constraints:** `pool` is `init` at `["pool", admin, asset_mint]`; `share_mint` is an
  existing `InterfaceAccount<Mint>` the admin created (validated below, not `init`); `vault` is
  `init` at `["vault", pool]` for `asset_mint` with `authority = pool` under `token_program`
  (the asset's program); `dead_shares` is `init` at `["dead", pool]` for `share_mint` with
  `authority = pool` under `share_token_program` (the share mint's program). Both token
  programs are `Interface<TokenInterface>`: classic SPL Token or Token-2022, independently.
- **Checks:** `1 <= max_line_bps <= 10 000` (`WrongAmount`); `asset_mint.decimals <= 19`, so
  that `min_first_deposit` exists (`WrongAccount`); the asset-mint policy (`asset_controls` in
  `pool.rs`): the mint is unpacked with its extensions and refused (`AssetMintNotAllowed`) if
  it carries `TransferFeeConfig`, `PermanentDelegate`, `TransferHook`, `NonTransferable`,
  `ConfidentialTransferMint`, `ConfidentialTransferFeeConfig`, `ConfidentialMintBurn`,
  `MintCloseAuthority`, or `DefaultAccountState` other than `Initialized`; a freeze authority
  and `Pausable` are accepted and recorded as bits of `asset_controls`; other extensions are
  accepted and ignored. The share-mint policy (`validate_share_mint`): owned by
  `share_token_program`, `mint_authority == pool`, `supply == 0`,
  `decimals == asset_mint.decimals`; refused extensions `TransferFeeConfig`, `TransferHook`,
  `MintCloseAuthority`, `PermanentDelegate`, `InterestBearingConfig` and the three confidential
  ones; `DefaultAccountState == Frozen` sets `permissioned` and then requires a freeze
  authority; `NonTransferable` is allowed (deposit mints, redeem burns); all failures are
  `ShareMintNotAllowed`. Who the freeze authority is (Token ACL's `MintConfig` or a key) is not
  recorded. `attestor != Pubkey::default()` (`WrongAccount`). Any value of
  `grace_period_seconds` is accepted, including zero. The `collection` account is recorded by
  key only until step 6 validates it as a Metaplex Core collection.
- **Writes:** every `Pool` field (`version = 2`, `attestor`, `collection`); `reserved` zeroed
  then the grace period, `max_line_bps`, `permissioned` and `asset_controls` packed in. On a
  permissioned share mint the dead account is born frozen: the freeze authority thaws it in this
  transaction or before the first deposit, or the first deposit fails inside Token-2022
  (`AccountFrozen`).
- **CPIs:** the three `init`s (system program + token programs) done by Anchor.
- **Events:** `PoolInitialized`, then `PoolConfigured`, `PoolLineCapSet`, `PoolFlags`,
  `AttestorSet`, `PoolCollectionSet`.

### 5.2 `deposit(assets: u64, min_shares_out: u64)`

- **Signers:** `depositor`.
- **Constraints:** `pool` re-derived from its own `admin`/`asset_mint`/`bump`; `asset_mint`
  (read-only, for the decimals `transfer_checked` needs), `share_mint`, `vault`, `dead_shares`
  must equal the addresses stored in the pool (`WrongAccount`); `token_program` (asset) and
  `share_token_program` (shares) are `Interface<TokenInterface>`s;
  `depositor_asset_account` must have `mint == pool.asset_mint` and `owner == depositor`;
  `depositor_share_account` must have `mint == pool.share_mint` and `owner == depositor`.
- **Checks, in order:** not paused (`Paused`); `assets > 0` (`WrongAmount`); read
  `supply` and `NAV` *before any token moves*; if `supply == 0`,
  `assets >= min_first_deposit(share_mint.decimals)` (`FirstDepositTooSmall`; the share mint
  carries the asset's decimals); `minted = shares_for_deposit(assets, supply, nav)` (`Overflow`,
  which also covers `NAV == 0` with `supply > 0`); `shares = minted - DEAD_SHARES` on a first
  deposit, else `minted`; `shares > 0` (`ZeroShares`); `shares >= min_shares_out`
  (`SlippageExceeded`).
- **CPIs:** `transfer` assets depositor → vault (depositor signs); `mint_to` shares → depositor
  (pool signs); on a first deposit, `mint_to` `DEAD_SHARES` → dead account (pool signs).
- **Writes:** none to `Pool`. The state is in the token accounts.
- **Events:** `Deposited` with `nav_after` and `supply_after` read back after `reload()`, then
  `NavUpdated`.

### 5.3 `redeem(shares: u64, min_assets_out: u64)`

- **Signers:** `holder`.
- **Constraints:** as in `deposit`, with `holder_share_account` and `holder_asset_account`
  owned by `holder` and matching the pool's mints.
- **Checks, in order:** not paused; `shares > 0`; price against the *pre-burn* supply;
  `assets = assets_for_redeem(shares, supply, nav)` (`Overflow`); `assets > 0` (`WrongAmount`);
  `assets >= min_assets_out` (`SlippageExceeded`); `assets <= vault.amount`
  (`InsufficientLiquidity`: the NAV counts money lent out, but only the vault can pay).
- **CPIs:** `burn` shares (holder signs); `transfer` vault → holder (pool signs).
- **Events:** `Redeemed`, then `NavUpdated`.

A redemption while principal is out and the vault is thin fails rather than paying a partial
amount; the holder retries with fewer shares.

### 5.4 `open_line(line_id, principal, rate_bps, term_end, agreement_hash, sponsor, schedule)`

- **Signers:** `admin` (pays rent for the line), `borrower`, and `sponsor` when one is named
  in the arguments (`Option<Signer>`).
- **Constraints:** `pool` has `has_one = admin` (`Unauthorized`); `line` is `init` at
  `["line", pool, line_id LE]`, so an existing `line_id` fails at account creation; `vault`
  is read-only and must be `pool.vault` (`WrongAccount`), for the NAV the cap is measured
  against.
- **Checks, in order:** not paused; `line_id == pool.line_count` (`WrongAccount`);
  `borrower != pool.admin` (`BorrowerIsAdmin`); `principal > 0` (`WrongAmount`);
  `principal <= (vault.amount + outstanding_principal) * max_line_bps / 10 000`
  (`LineTooLarge`; the NAV counts principal lent out, and an empty pool opens no line);
  `rate_bps <= 10 000`; `term_end > now`; `agreement_hash != [0; 32]` (all `WrongAmount`); if
  `sponsor` is `Some(k)`, the `sponsor` account must be present and be `k`
  (`SponsorRequired`); `interest_due = principal * rate_bps / 10 000` (`Overflow`); if
  `schedule` is `Some`, `validate_schedule`:
  `1 <= installments <= 120`, `interval_seconds > 0`, `late_bps <= 10 000`,
  `3 <= max_missed <= installments`, `principal + interest_due >= installments` (one unit per
  installment) — all `InvalidSchedule`; and `now + installments * interval <= term_end`
  (`ScheduleEndsAfterTerm`), re-checked at the draw.
- **Writes:** every `CreditLine` field; `status = Approved`; `version` 1 or 2; the schedule
  packed into `reserved` with `late_collected = 0`, `late_paid_until = 0`;
  `pool.line_count += 1`.
- **CPIs:** none that move tokens.
- **Events:** `LineOpened`, then `LineScheduled` on a version-2 line.

The borrower's signature is what binds the on-chain line to a person who agreed to the
`agreement_hash`; the sponsor's signature records a public programme's endorsement and grants
no authority afterwards (a sponsor repays through `repay` like anyone else).

### 5.5 `draw()`

- **Signers:** `borrower`.
- **Constraints:** `line` has `has_one = pool` and `has_one = borrower` (`Unauthorized`);
  `asset_mint` (read-only) and `vault` match the pool; `borrower_asset_account` has the pool's asset mint and is owned by
  the borrower.
- **Checks, in order:** not paused; `status == Approved` (`LineStatus`); `now <= term_end`
  (`TermEnded`); on a scheduled line, `now + installments * interval <= term_end`
  (`ScheduleEndsAfterTerm`): due dates count from the draw, so a late draw could otherwise push
  the last installment past the term; `principal <= vault.amount` (`InsufficientLiquidity`).
- **CPIs:** `transfer` the whole principal vault → borrower (pool signs). Partial draws do not
  exist.
- **Writes:** `pool.outstanding_principal += principal`; `line.status = Active`;
  `line.drawn_at = now`.
- **Events:** `LineDrawn` with `outstanding_after`.

### 5.6 `repay(amount: u64)`

- **Signers:** `payer`, anyone. The event records who paid.
- **Constraints:** `line` has `has_one = pool`; `asset_mint` (read-only) and `vault` match the
  pool; `payer_asset_account` has the pool's asset mint and is owned by the payer.
- **Common checks:** status accepted (`Active` for version 1; `Active` or `InCollection` for
  version 2, `LineStatus`); `remaining = (principal - principal_repaid) + (interest_due -
  interest_repaid)`; `amount > 0`.
- **Version 1 (single maturity):** `amount <= remaining` (`WrongAmount`: overpayment is
  rejected); `principal_part = min(amount, principal_left)`, `interest_part = amount -
  principal_part`. Principal first, then interest.
- **Version 2 (scheduled):** `late = arrears(now).late_due`; `amount >= late`
  (`LateInterestUnpaid`); `rest = min(amount - late, remaining)`; `interest_part = min(rest,
  interest_left)`; `principal_part = rest - interest_part`. Late interest first, then interest,
  then principal. **The transfer is `late + interest_part + principal_part`, which can be less
  than `amount`:** a scheduled line never takes more than is due and the surplus stays in the
  payer's account (`tests/schedule.rs::late_interest_is_capped_and_overpayment_stays_with_the_payer`).
- **CPIs:** one `transfer` payer → vault (payer signs) for the computed total.
- **Writes:** `pool.outstanding_principal -= principal_part`;
  `pool.interest_collected += interest_part` (late interest is *not* counted here);
  `line.principal_repaid`, `line.interest_repaid`; on version 2, when `late > 0`,
  `late_collected += late` and `late_paid_until = now`, which stops the same seconds being
  charged twice. **When `late` is zero the watermark does not move** (R-11): the accrual that
  rounded down to zero keeps running, so a borrower cannot erase it by paying one unit every
  few seconds; if `remaining` reaches zero, `status = Repaid` and `closed_at = now`.
- **Events:** `LateInterestCollected` (only when `late > 0`), then `LineRepaid` (and `NavUpdated` last) with
  `remaining` after this payment (without late interest) and `nav_after`.

Because `late_due` is computed at execution time, a client that quotes it a few seconds
earlier can be short by a few units and hit `LateInterestUnpaid`; clients pad the quote
(`tests/known_limits.rs::limit_late_interest_is_settled_at_execution_time_so_clients_must_pad`,
`clients/js/src/schedule.ts` computes it the same way as the program).

### 5.7 `mark_in_collection()`

- **Signers:** none required beyond the fee payer. Anyone, also while paused.
- **Constraints:** `line` has `has_one = pool`.
- **Checks, in order:** the line has a schedule (`NotScheduled`); `status == Active`
  (`LineStatus`); `arrears(now)`; `k = covered + max_missed <= installments`
  (`NotEnoughMissed`: with `covered` installments paid, the `max_missed`-th missed one is
  installment `k`); `now > drawn_at + k * interval + grace` (`NotEnoughMissed`: the grace of
  the last missed installment has run).
- **Writes:** `status = InCollection`. Nothing else: collection does not accelerate the
  balance, the remaining installments keep their due dates.
- **Events:** `LineInCollection` with `missed` and `remaining` (principal + interest, no late
  interest).

### 5.8 `write_off()`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`). Not blocked by the pause.
- **Constraints:** `line` has `has_one = pool`; `vault` is read only for the NAV in the event.
- **Checks:** `after_term = now > term_end + grace`. If `status == Active`: require
  `after_term` (`TermNotEnded`). If `status == InCollection`: require `after_term` **or**
  (`arrears(now).first_missed_due` is `Some(due)` and `now > due + grace`)
  (`WriteOffTooEarly`); a cured line has no `first_missed_due`, so only the term can release it.
  Any other status: `LineStatus`.
- **Writes:** `loss = principal - principal_repaid`; `pool.outstanding_principal -= loss`;
  `pool.written_off += loss`; `status = WrittenOff`; `closed_at = now`.
- **CPIs:** none. No tokens move; the NAV drops by `loss` because `outstanding_principal` does.
- **Events:** `LineWrittenOff` with `loss` and `nav_after`, then `NavUpdated`.

Unpaid interest is never part of the loss: it was never in the NAV.

### 5.9 `set_paused(paused: bool)`

- **Signers:** `admin`.
- **Writes:** `pool.paused`. **Events:** `PoolPaused`.

### 5.10 `set_attestor(attestor: Pubkey)`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`).
- **Checks:** `attestor != Pubkey::default()` (`WrongAccount`): the zero key could never
  co-sign, so a pool with it would open no line.
- **Writes:** `pool.attestor`. **Events:** `AttestorSet`.

The attestor can block every opening, draw and endoso by withholding its signature and can move
nothing; this instruction is the recovery when its key rotates or is lost.

## 6. Installments, arrears and late interest (`math.rs`)

A scheduled line stores its terms and its repayments; everything else is recomputed on every
call by `arrears(terms, paid, now)`. Nothing about arrears is stored, so there is no state to
drift.

**Due dates.** Installment `k` (1-based) is due at `drawn_at + k * interval_seconds`, and at
that very second it already counts as due. `installments_due(now)` is
`min(installments, (now - drawn_at) / interval)`, zero before the first due date.

**Installment size.** `total = principal + interest_due`. Each installment is `total /
installments` rounded down and the last one carries the remainder, so `cumulative_due(k)` is
`k * base` for `k < installments` and exactly `total` for the last. Example: 1 000 003 in six is
five of 166 667 and one of 166 668.

**Installments covered.** `installments_covered(paid)` is `paid / base`, capped at
`installments - 1` unless `paid >= total`. Paying 6 × 166 667 = 1 000 002 of the example above
covers five installments, not six: the last one is only covered by paying the total. Late
interest never pays an installment (`paid` is principal + ordinary interest only).

**Missed** is `due - covered` when positive; the grace period does not change the count, it
only delays late interest and collection.

**Late interest** accrues on each overdue part `part_k` separately, from
`start_k = max(due_k + grace, late_paid_until)` until `now`, linearly at `late_bps` a year of
365 days:

```
late_due = Σ_k part_k * min(now - start_k, cap_k) * late_bps / (10 000 * SECONDS_PER_YEAR)
cap      = ceil(10 000 * SECONDS_PER_YEAR / late_bps)      seconds after which a part would
                                                             exceed itself
```

The weighted sum is accumulated in `u128` and divided once, so rounding happens once per
settlement. Two caps apply, and they are different things:

- **Per settlement:** `late_due <= overdue` for this repayment; each part accrues at most its own
  amount in one settlement because of `cap`. After a repayment moves `late_paid_until`, a part
  that is still unpaid accrues again, so across several repayments one part can accrue more than
  itself.
- **Over the life of the line:** `late_due <= principal - late_collected`. Once a borrower has
  paid late interest equal to the principal, no more is charged; every further payment goes to
  interest and principal.

`arrears` returns `Arrears { due, covered, missed, overdue, late_due, first_missed_due }`.
`repay` uses `late_due`; `mark_in_collection` uses `covered` and `missed`; `write_off` uses
`first_missed_due`. The loop runs at most `installments <= 120` times, which is what
`tests/compute.rs` bounds: the arrears instructions stay under the compute ceiling at 18 and at
120 installments.

## 7. Invariants

What must hold between instructions, and which code keeps it:

1. `NAV == vault.amount + outstanding_principal` by definition; `outstanding_principal` only
   changes in `draw`, `repay`, `write_off` and only by the amount that moved or was lost.
2. `outstanding_principal == Σ (principal - principal_repaid)` over lines in `Active` or
   `InCollection`. Not checked on chain (it would need every line account); what is checked
   at the end of every money instruction is `Pool::check()` (R-25): `vault.amount +
   outstanding_principal` fits a `u64`, on the balance reloaded after the CPIs, else
   `InvariantBroken`.
3. `share_mint.supply >= DEAD_SHARES` once anything was deposited: the dead account's shares are
   never burned (no instruction takes `dead_shares` as a burn source). `Pool::check()` re-checks
   it (`supply == 0 || supply >= DEAD_SHARES`) in `deposit`, `redeem`, `repay` and `write_off`,
   the four instructions that carry the share mint (since pool v2, for `NavUpdated`); `draw`
   runs the NAV leg only.
4. Only `draw` and `redeem` move tokens out of the vault, both signed by the pool PDA, both to
   an account whose owner signed the instruction (the borrower of an admin-approved line, the
   holder burning shares).
5. `principal_repaid <= principal` and `interest_repaid <= interest_due`: version 1 rejects
   overpayment, version 2 clamps to `remaining`.
6. `late_collected <= principal` on a version-2 line, by the lifetime cap in `arrears`.
7. Every arithmetic step is `checked_*` or in `u128`; a `None` becomes `Overflow` and the
   transaction fails. `tests/math_props.rs` fuzzes the pure functions with `proptest`.
8. Events are append-only: fields of an existing event never change; new facts arrive as new
   events (`PoolConfigured`, `LineScheduled`, `LateInterestCollected` were added that way).

## 8. Roles

| role | identified by | can |
|---|---|---|
| admin | `Pool.admin`, fixed forever | `open_line` (up to `max_line_bps` of the NAV per line: `LineTooLarge`; any borrower other than itself: `BorrowerIsAdmin`; no limit on the number of lines), `write_off` (only when overdue past grace), `set_paused`, `set_attestor` |
| attestor | `Pool.attestor`, set at `init_pool`, rotated by `set_attestor` | recorded in pool v2 step 4; from step 5 it co-signs `open_line` and `draw` (and `endorse_pagare` in step 6), so it can block every opening, draw and endoso and move nothing |
| share-mint freeze authority | the freeze authority of the admin-supplied share mint (Token ACL's `MintConfig` in production); not recorded by the program | thaw or freeze share accounts, the dead account included: admits and bars investors |
| borrower | `CreditLine.borrower` | `draw` its own approved line before the term |
| sponsor | `CreditLine.sponsor` | nothing after co-signing the opening |
| depositor / holder | any signer with the asset or shares | `deposit`, `redeem` |
| anyone | | `repay` any drawn line, `mark_in_collection` |
| upgrade authority | BPF loader authority of the program id | replace the program; a single key on devnet |

There is no operator role, no fee switch, no admin withdrawal, no account closing and no way to
change the asset, the admin or the grace period after `init_pool`.

## 9. Known limits (proved by `tests/known_limits.rs`)

Each of these is a passing test that documents behaviour the program *has*, not a bug it fixed.
The review ids refer to the 2026-10-12 review in theOffice.

| test | behaviour | review |
|---|---|---|
| `admin_cannot_be_the_borrower_of_its_own_line` | **inverted 2026-10-31**: `open_line` refuses `borrower == admin` (`BorrowerIsAdmin`) | R-01 delta |
| `a_line_for_the_whole_vault_is_refused_under_the_concentration_cap` | **inverted 2026-10-31**: a line above `max_line_bps` of the NAV fails with `LineTooLarge` (1,000 bps in the test and in production pools; the harness opens its other pools at 10,000 bps, where the cap is inert). The admin can still reach the whole vault through several lines to wallets it controls, and the NAV the cap is measured against can be inflated by a deposit in the same transaction: the cap bounds one honestly proposed line, not the admin | decision 20, P1-1 |
| `first_deposit_minimum_is_one_unit_of_the_asset_with_a_dead_shares_floor` | **inverted 2026-10-31**: the minimum is one whole unit of the asset for six or more decimals; below six decimals the floor of 1 000 × `DEAD_SHARES` base units applies, so a two-decimal asset needs 10,000.00 | R-07 delta |
| `limit_deposit_just_before_a_repayment_shares_the_interest` | interest counts when collected, so a deposit that lands just before a repayment gets the same uplift as one made months earlier | (design) |
| `limit_redeem_between_collection_and_write_off_exits_at_par` | a line in collection still counts fully in the NAV until the admin writes it off; a holder who redeems in between exits at par and the loss falls on the rest | (design) |
| `limit_pause_blocks_exit_while_repayments_continue` | while paused, repayments raise the NAV but nobody can redeem | (design) |
| `limit_total_write_off_leaves_a_pool_that_cannot_take_deposits` | if every line is written off and the vault is empty, `NAV == 0` with `supply > 0` and `shares_for_deposit` divides by zero: the pool is dead | (design) |
| `limit_collection_is_unreachable_when_covered_plus_max_missed_exceeds_installments` | a line with `installments < covered + max_missed` can never enter collection; only the term releases it | (design) |
| `limit_late_interest_is_settled_at_execution_time_so_clients_must_pad` | see §5.6 | (client) |

Other points outside the test file, from the same review and from the Etherfuse research
(theOffice, 2026-10-19):

- Closed 2026-10-31 (series-shelf 2b, decision 15): `init_pool` applies an asset-mint policy
  (§5.1) and records the issuer controls it accepts in `asset_controls`. If the asset issuer
  freezes the vault or pauses the mint, every instruction that moves tokens fails until it is
  undone; that is accepted and visible to clients (`poolFlags` in the JS client), not refused.
- Inverted 2026-10-31 (series-shelf 2a): Token-2022 mints (MEX, CETES) can be the asset. The
  program uses `anchor_spl::token_interface` (`InterfaceAccount`, `Interface<TokenInterface>`)
  and `transfer_checked` everywhere, and every money instruction takes the asset mint read-only
  for its decimals. `tests/pool.rs::a_pool_on_a_token_2022_asset_runs_the_full_cycle` runs the
  whole cycle on a Token-2022 asset.
- Missed installments are counted from amounts paid, not from due dates: a borrower who pays
  one installment of principal and one of interest in a different order is still "covered"
  by amount (review STR-1 discusses counting by due date).

## 10. Tests and what each file proves

| file | harness | proves |
|---|---|---|
| `src/math.rs` (unit) | plain | rounding, caps, installment arithmetic, arrears cases |
| `tests/math_props.rs` | proptest | the pure functions never panic and keep their bounds over random inputs |
| `tests/vectors.rs` | plain + `vectors.json` | committed numeric vectors match the program arithmetic (shared with the JS client's tests) |
| `tests/pool.rs` | LiteSVM | init, first deposit, pricing after interest, redeem liquidity, wrong accounts, pause, dead-shares inflation defence, rounding and slippage floors |
| `tests/lines.rs` | LiteSVM | open/draw/repay full cycle raises the share price, sponsor co-sign, sponsor pays interest, overpay and wrong status, double draw, write-off lowers the NAV once, bad params, term checks |
| `tests/schedule.rs` | LiteSVM, clock warped | schedule validation, installments on their due dates, missed count over time, late interest after grace, caps, repayment order, collection after three missed, cure beats collection, write-off timing in both paths |
| `tests/known_limits.rs` | LiteSVM | the nine tests in §9: six limits still in force and three inverted on 2026-10-31 |
| `tests/compute.rs` | LiteSVM | arrears instructions under a 100 000 CU ceiling at 18 and 120 installments (worst measured: 67 688 CU at 120) |
| `tests/common/mod.rs` | | the LiteSVM fixture: loads `target/deploy/elote_pool.so`, builds every instruction, warps the clock |

```sh
NO_DNA=1 anchor build --ignore-keys --arch v0      # programs → target/deploy/elote_pool.so + target/idl
cargo test -p elote_pool                           # 82 tests, LiteSVM in-process, no validator
```

The LiteSVM tests load the `.so` from `target/deploy`, so `anchor build` must run first.

## 11. IDL and client

```
anchor build ──► target/idl/elote_pool.json ──npm run idl:sync──► idl/elote_pool.json
                                                                     │
                                                 codama.mjs ◄────────┘
                                                     │
                                                     ▼
                                   clients/js/src/generated/   (Codama, @solana/kit, do not edit)
                                   clients/js/src/schedule.ts  (hand-written: reserved-byte
                                                                decoding and arrears, mirrors math.rs)
                                   clients/js/src/index.ts
                                                     │
                                                     ▼
                                   @softlaw/elote-pool-client  (npm, trusted publishing on tag client-v*)
```

`npm run check:client` regenerates the client and fails if the committed output differs, so the
IDL in `idl/` and the client cannot drift from the program silently. The generated client knows
nothing about `reserved`; anything a UI shows about grace, schedule or arrears comes from
`schedule.ts`, which must change in step with `math.rs`.

## 12. Deployment

| cluster | program id | upgrade authority | built from |
|---|---|---|---|
| devnet | `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM` | `GU7CfoGZYsfkx23aJoqFjfwyAmLCExqBBoBFrn2EvZKS` (single hot key) | the `colosseum-2026` build (Anchor 1.1.2 / Solana 3.1.10; same program logic as tag `pre-audit-2026-10-19`), **not `main`** |
| mainnet-beta | not deployed | | |

The devnet program data (`4CfrXy4vai6ETomrEqrHKMCdUM532UkdXG92BJh7oSLs`) was compared byte for
byte on 2026-10-19 with the hackathon build (sha256 `a88121a6…`). Builds are path-dependent on a
developer machine (the same `main` commit produced two different `.so` hashes in two directories),
so only a `solana-verify` build in its container is comparable with the on-chain hash. Before
mainnet: deploy from `main`, Squads multisig as upgrade authority, IDL on chain, verified build,
and the asset-mint policy of §9 settled.
