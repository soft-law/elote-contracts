# `elote_pool` architecture

How the program is built, instruction by instruction, byte by byte. The [README](README.md) says
what the pool does for investors and borrowers; this file says how the code does it, for whoever
has to change it, audit it or write a client against it. The interface reference (per-instruction
accounts, arguments and errors) lives in the private docs repo.
is the account-by-account reference generated from the IDL; [SECURITY.md](SECURITY.md) lists the
roles, the trust assumptions and the known limits. The source of truth is
`programs/elote_pool/src/`: where this file and the code disagree, the code is right and this file
needs fixing. Code is cited by item name (`council::check`, `fx::within_band`,
`instructions/lines.rs::repay`), not by line number.

Program id `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`, Anchor 1.2.1, Solana 4.1.2, Rust
1.89.0. 38 instructions, 3 account types, 41 events, 79 error codes (five of them retired and kept
so nothing renumbers). Decision numbers (32–41, 38-g, 40b) follow the
owner review of 2026-10-31 and the record of 2026-11-08 in the decision register (private docs repo).

## 1. Overview and model

### 1.1 One program, nine modules

```
programs/elote_pool/src/
├── lib.rs        declare_id!, security.txt, the #[program] module: 38 thin entry points
├── state.rs      Pool, CreditLine and Pagare layouts, LineStatus, PDA seeds, constants, the
│                 asset allow-list, Pool::nav / Pool::entry_nav / Pool::check
├── council.rs    the Squads v4 council: fresh (valid_until), check, is_member, the vault address
├── fx.rs         the FIX: dates in Mexico City, the feeder's band, the RedStone reference, the
│                 bounds, MXN⇄USDC conversions and their rounding
├── terms.rs      LineTerms and the terms hash the council approves
├── math.rs       pure arithmetic: share pricing, ordinary interest, the payment order, installments, arrears
├── registry.rs   the pagaré registry: payment annotation hashes, entry hashes, the hash chain
├── errors.rs     PoolError (79 codes, 6000..=6078)
├── events.rs     41 events, all emitted through emit_cpi!
└── instructions/
    ├── pool.rs   init_pool, deposit, redeem, pause, resume, the council's pool setters,
    │             propose_council, accept_council, set_fx_feeder, post_fix, pool_signer!
    ├── lines.rs  request_line, approve_line, reject_line, withdraw_request, draw (mints the
    │             pagaré), repay, mark_in_collection, write_off, suspend_draws, resume_draws,
    │             propose_acceleration, withdraw_acceleration, accelerate_line, cancel_line
    └── pagare.rs deliver_pagare, burn_pagare, annul_pagare, endorse_pagare,
                  revoke_procuracion, court_annotation, annotate_payment
```

`math.rs` and `fx.rs` have no Anchor accounts and no I/O. Their functions take integers and return
`Option` (`None` on overflow or division by zero), and the handlers turn `None` into
`PoolError::Overflow`. Every instruction struct carries `#[event_cpi]`, so it takes two extra
accounts (the event authority PDA and the program) and emits its events as self-CPIs.

**Why one program.** There is no separate loan program. The loan contract is the `CreditLine`
account and `instructions/lines.rs`; the fund is the `Pool` account and `instructions/pool.rs`.
Every loan event moves the pool's NAV (`Pool.outstanding_principal`, and since decision 40
`Pool.impaired_principal`) in the same atomic state transition, and only the pool PDA signs the
vault's outbound transfers (`pool_signer!`). A lending program separate from the fund would have to
be trusted by the fund to move vault money through a CPI, which is the privilege a split is meant
to isolate. One program keeps each loan event and its NAV effect under one invariant check, one
IDL, one upgrade authority and one audit surface. A split would earn its place with a second
product borrowing from the pool, loan servicing that must upgrade apart from the money logic, or
one loan funded by several pools; none exists.

**What stays off chain.** The program stores hashes of the signed documents (the credit agreement,
the pagaré template, the credit file's salted merkle root, receipts, endosos, court documents) and
never interprets them. What it enforces are the economic terms copied into the line at
`request_line` and approved by the council: principal, rate, term, the installment schedule, the
late rate and the collection threshold.

### 1.2 The money model

```
entry NAV       = vault.amount + outstanding_principal + recoverable          Pool::entry_nav
NAV             = entry NAV − ⌊impaired_principal × provision_bps / 10 000⌋   Pool::nav
deposit price   = entry NAV / share_mint.supply
redeem price    = NAV / share_mint.supply
```

All three amounts are in the asset's base units (USDC in production).

- **Shares (ERC-4626).** `deposit` mints `assets × supply / entry NAV` shares, one to one while
  `supply == 0` (`math::shares_for_deposit`). `redeem` pays `shares × NAV / supply` assets
  (`math::assets_for_redeem`). Both products go through `u128` and both divisions round down, in
  the pool's favour.
- **The dual price (decisions 40 and 40b, Maple v2's dual exchange rate).** A line that enters
  collection adds its outstanding USDC cost to `impaired_principal`, and the NAV takes
  `provision_bps` of it (100% unless the council moves it). Redemptions, the line caps and the
  invariant check use that provisioned NAV, so nobody redeems at a price that still counts a
  defaulted credit as good. Deposits use the entry NAV, with the impaired book at par: whoever
  enters while a line is impaired pays par for that book, so a later recovery, which releases the
  provision, hands the entrant nothing, and depositing just before a recovery nets zero or less,
  flash-funded or patient. The same holds for a line written off: its cost stays in the entry NAV
  (`recoverable`) until it is recovered or the council condones it (decision 51). Deposits are
  never gated. When nothing is impaired and nothing written off is still owed the two prices are
  the same.
- **Outstanding principal at USDC cost.** `Pool.outstanding_principal` is the USDC the pool
  delivered on draws and has not recovered, condoned or written off. `draw` adds the USDC it
  delivered; `repay` subtracts the cost of the principal repaid, valued at the FIX each pagaré was
  drawn at (`fx::cost_of`); `condone` the cost of the principal condoned, valued the same way
  (decision 51); `write_off` the line's remaining cost, `usdc_drawn − usdc_cost_repaid`.
- **The loss at USDC cost.** `Pool.written_off` is the cost the pool has recognised as lost and
  not recovered: `write_off` and a principal quita add to it, a recovery on a written-off line
  takes the cost it recovered back out of it (decision 51).
- **Debt in MXN, settlement in USDC at the FIX.** Every amount on a line and a pagaré is in
  centavos of MXN (`CreditLine.currency = CURRENCY_MXN = 484`). The vault, deposits, redemptions
  and repayments are in asset base units. A draw delivers `usdc_for_mxn(amount_mxn, fix)`, rounded
  down. A repayment credits `mxn_for_usdc(max_usdc, fix)` centavos, rounded down, and takes
  `usdc_ceil_for_mxn(applied, fix)`, rounded up (§6).
- **Interest counts when collected.** Nothing accrues in the NAV. Interest, late interest and the
  FX result of the principal enter the NAV only when a repayment brings the USDC into the vault.

| instruction | vault | `outstanding_principal` | `impaired_principal` | NAV (redeem) | entry NAV (deposit) |
|---|---|---|---|---|---|
| `deposit` | + assets | | | + | + (the entry price does not move) |
| `redeem` | − assets | | | − (the redeem price does not move) | − |
| `draw` | − USDC delivered | + USDC delivered | | unchanged | unchanged |
| `repay`, principal part | + USDC that paid it | − cost at the pagarés' FIX | − the same cost, when the line is `InCollection` | + `FxRealized.delta`, plus the provision released | + `FxRealized.delta` |
| `repay`, interest and late interest | + | | | + | + |
| `mark_in_collection`, `accelerate_line` | | | + the line's `usdc_drawn − usdc_cost_repaid` | − that cost × `provision_bps` / 10 000 | unchanged |
| `write_off` of an `InCollection` line | | − the line's remaining cost | − the same | − the cost × (10 000 − `provision_bps`) / 10 000 (zero at 100%) | − the cost |
| `write_off` of an `Active` line | | − the line's remaining cost | | − the cost | − the cost |
| `condone`, principal, of an `Active` or `InCollection` line (decision 51) | | − cost at the pagarés' FIX | − the same cost, when `InCollection` | − the cost × (10 000 − `provision_bps`) / 10 000 in collection, − the cost when active | − the cost |
| `condone`, principal, of a `WrittenOff` line | | | | unchanged (the write-off took it) | unchanged |
| `repay` on a `WrittenOff` line, principal part (decision 51) | + USDC that paid it | | | + that USDC (`written_off −=` the cost; `FxRealized.delta` is the difference) | + that USDC |
| `set_provision_bps` | | | | moves with the rate | unchanged |

Up to the rounding of the provision (one floor over the pool's whole `impaired_principal`).
`Pool.interest_collected` (`usdc_for_mxn(interest_part)` per repayment), `Pool.written_off` (net
of recoveries) and `Pool.fx_realized` (signed) are informative running totals in USDC. Condoned
interest never reaches `interest_collected`: it was never received.

With nothing impaired a deposit leaves both prices where they were. With an impaired book the
entrant pays the higher entry price, so the redemption price of the holders already in rises
slightly; that is the entrant buying the impaired book at par.

`NavUpdated` is emitted by `deposit`, `redeem`, `repay`, `write_off` and a `condone` that
recognises a loss. The provision moves the
NAV in three instructions that do not emit it: `mark_in_collection` and `accelerate_line` carry the
pool's new `impaired_principal` in `LineInCollection` and `LineAccelerated`, and
`set_provision_bps` emits `ProvisionSet`; an indexer recomputes the NAV from those.

**Dead shares.** The first deposit must be at least `min_first_deposit(decimals)` =
`max(10^decimals, MIN_FIRST_DEPOSIT_DEAD_RATIO × DEAD_SHARES)` = `max(10^decimals, 1 000 000)` base
units — 1 000 000 on the 6-decimal assets `init_pool` admits — and `DEAD_SHARES` (1 000) of the shares it mints go to the pool-owned `dead_shares` account,
which no instruction burns or moves. The supply never falls below 1 000 once anything was
deposited, so a donation to the vault cannot round later deposits down to nothing.

**Slippage floors.** `deposit` takes `min_shares_out` and `redeem` takes `min_assets_out`
(`SlippageExceeded`).

**`Pool::check`** runs at the end of every money instruction on the balances reloaded after the
CPIs: the NAV, provision included, is computable (`vault + outstanding` fits a `u64` and the
provision does not exceed it), `impaired_principal <= outstanding_principal`, and a share supply,
when passed, is zero or at least `DEAD_SHARES` (`InvariantBroken` otherwise). `deposit`, `redeem`,
`repay`, `write_off` and `condone` pass the supply; `draw` checks the NAV legs only. It returns the provisioned
NAV, which is the `nav_after` of every event and `NavUpdated.total_assets`.

**Invariants** that hold between instructions:

1. `outstanding_principal == Σ (usdc_drawn − usdc_cost_repaid)` over the lines `Active` or
   `InCollection`, and `impaired_principal` is the same sum over the lines `InCollection`;
   `written_off == Σ (usdc_drawn − usdc_cost_repaid)` over the lines `WrittenOff`, plus the cost of
   every principal quita (`LineCondoned.cost`) on any line (decision 51: `usdc_cost_repaid` counts
   repaid and condoned principal alike). Not checked on chain (it would need every line account);
   `Pool::check` checks the bound between the first two.
2. Only `draw` and `redeem` move tokens out of the vault, both signed by the pool PDA, each to a
   token account owned by the instruction's signer (the borrower of an approved line, the holder
   burning shares).
3. `drawn <= principal` and `drawn` never decreases; `principal_repaid <= drawn` and
   `interest_repaid <= interest_accrued(now)`, because every repayment and every quita clamps to
   what is owed. `principal_seconds` and `accrued_at` never decrease, and `accrued_at <=
   accrual_end` (decision 48). After any `repay` that applies principal, `interest_unpaid(now) ==
   0`: interest is taken first (`tests/schedule.rs`, the state machine and
   `after_any_payment_that_touches_principal_no_interest_is_left_unpaid`). A quita is not a
   payment and may leave interest unpaid while it condones principal (decision 51); the interest
   accrued never falls, because `accrue(now)` runs before the principal moves, so
   `interest_unpaid` cannot underflow.
4. `late_collected <= drawn`, by the lifetime cap in `math::arrears`.
5. Every arithmetic step is `checked_*` or in `u128`; a `None` becomes `Overflow` and the
   transaction fails. `tests/math_props.rs` fuzzes the pure functions.

## 2. Roles

| role | identified by | can |
|---|---|---|
| council | `Pool.council`: vault 0 of `Pool.multisig`, a Squads v4 multisig (`council_kind = COUNCIL_SQUADS`) | every council instruction (§4, marked **C**): approve and reject requests; cancel, suspend, resume and accelerate lines; write off; pause and resume the pool; set the pool's parameters (line cap, outstanding cap, provision, FIX bounds, acceleration notice, collection period); publish the FIX outside the feeder's band and name the feeder; rotate itself; and, as the holder of the pagarés in the pool, endorse en procuración, revoke, annotate court acts and receipts, and annul a título issued in error |
| council member | a key in the multisig's member list | nothing alone (decision 39); votes in Squads. Cannot be a borrower (`BorrowerIsCurator` at `approve_line`) or a procurador (`EndorseeInvalid`) |
| fx_feeder | `Pool.fx_feeder` (the zero key for none), set by `set_fx_feeder` | `post_fix` for a later date, within the band from the FIX in force, within 2% of the RedStone reference, within the pool's FIX bounds, once per Mexico City day (§6) |
| creator | `Pool.creator`: signer and payer of `init_pool`, a seed of the pool's address | nothing after `init_pool`; cannot be a procurador |
| borrower | `CreditLine.borrower` | `request_line`, `withdraw_request`, `draw` its approved line alone (signing the FIX); receives its paid pagarés from `deliver_pagare` once the line owes nothing (decision 49) and, as their owner, may `burn_pagare` them |
| payer (relayer) | any signer passed as `payer` | funds the rent of a request (`CreditLine.payer`, refunded on reject or withdraw) and of the pagaré asset and its record at `draw`; sends `deliver_pagare` and pays what its CPIs need. The `payer` of `repay` is a different role: whoever's USDC moves |
| sponsor | `CreditLine.sponsor`, `sponsor_role` 0 promoter or 1 guarantor | co-signs `request_line` when named (`SponsorRequired`). A guarantor (aval) whose payment covers a pagaré receives that título at `deliver_pagare` once the line owes nothing (LGTOC 115, decision 49); otherwise the role is recorded, not enforced |
| procurador | `Pagare.procurador`, named by `endorse_pagare` | nothing over the asset (decision 38): it collects through `repay` like any payer and litigates with the registry entry as the endorsement |
| investor | holder of shares | `deposit`, `redeem`. When `Pool.permissioned` (share mint `DefaultAccountState = Frozen`), the share mint's freeze authority (Token ACL in production) thaws each investor's share account first; the program records `permissioned`, not who holds that authority |
| anyone | | `repay` any drawn line, `mark_in_collection`, `deliver_pagare` |
| upgrade authority | the BPF loader's authority for the program id | replace the program (§13) |

There is no fee switch, no withdrawal by any role and no way to change the asset, the share mint,
the collection or the creator after `init_pool`.

### 2.1 The council and `council::check`

The council is not a key the program trusts by address alone. Every council instruction takes the
stored multisig account (`address = pool.multisig`, else `CouncilInvalid`), has the council sign
(`has_one = council` → `Unauthorized`, or `holder == pool.council` → `NotHolder` on the pagaré
instructions, `annotator == pool.council` → `Unauthorized` on `annotate_payment`) and runs
`council::check(multisig, pool.council, pool.council_vault_bump)`, which reads the Squads account
at fixed offsets, with no Squads crate and no CPI:

1. the account is owned by Squads v4 (`SQUADS_PROGRAM_ID`,
   `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`);
2. `create_program_address(["multisig", multisig, "vault", 0, vault_bump])` under Squads equals
   `pool.council` (`council::vault_address`);
3. the data is longer than the `rent_collector` offset (94) and starts with the discriminator
   `sha256("account:Multisig")[..8]`;
4. **autonomous:** the `config_authority` bytes 40..72 are all zero, so only the members' votes
   change the multisig;
5. the members list (after `threshold` at 72, `rent_collector: Option<Pubkey>` at 94 and the bump)
   has at most `MAX_COUNCIL_MEMBERS` = 10 entries of 33 bytes (`CouncilTooLarge`);
6. **majority:** with `voters` the members whose permission mask has the Vote bit (`1 << 1`),
   `voters > 0` and `threshold > voters / 2`.

Any other failure is `CouncilInvalid`. `init_pool` and `accept_council` run the same check on the
multisig they are given, with its canonical vault bump (`council::canonical_vault`), and store that
bump in `Pool.council_vault_bump`.

The council signs only through Squads `vault_transaction_execute`, so the CPI depth of a council
instruction that reaches Metaplex Core would be Squads (1), `elote_pool` (2), Core (3), system (4)
(`tests/squads.rs`). Since decision 38 no council instruction calls Core.

**`valid_until`.** Every instruction a council signs takes `valid_until: i64` as its last argument
and starts with `council::fresh(valid_until)`: `now > valid_until` is `ProposalExpired`, and
`valid_until > now + MAX_PROPOSAL_HORIZON_SECONDS` (15 days) is `ProposalTooFar`; both bounds are
inclusive. Squads proposals never expire and execute even after a config change, so the program
bounds them. `pause` is a council instruction like the others (decision 39) and takes it.
`post_fix` takes it on both paths, the feeder's included. `init_pool` (the council does not sign
it) and the instructions no council signs (`deposit`, `redeem`, `request_line`, `withdraw_request`,
`draw`, `repay`, `mark_in_collection`, `deliver_pagare`, `burn_pagare`) take none.

**Rotation** is two steps. `propose_council` (the council signs, after `council::check` on the
current multisig; only `kind == COUNCIL_SQUADS`, else `CouncilKindInvalid`) stores
`pending_council` and its kind; a new proposal replaces a pending one and the zero key withdraws
it. `accept_council` is signed by the proposed key: `pending_council` must be non-zero and equal to
the signer (`NoPendingCouncil`), its kind `COUNCIL_SQUADS` (`CouncilKindInvalid`), the canonical
vault 0 of the given multisig must be the signer (`CouncilInvalid`), and `council::check` runs on
that multisig before `council`, `council_kind`, `multisig` and `council_vault_bump` are written. The
pool never rotates to an address that cannot sign or to a multisig without a majority.
`COUNCIL_GENERIC` (1) is defined for a future governance PDA and refused by both steps.

`council::is_member` reads the same member list (owner checked) and is used by `approve_line`
(`BorrowerIsCurator`) and `endorse_pagare` (`EndorseeInvalid`).

## 3. Accounts and the PDA tree

```
Pool            PDA ["pool", creator, asset_mint]              program-owned, 495 bytes
├── share_mint  creator-supplied mint                          classic or Token-2022, decimals = asset's,
│                                                              mint authority = pool PDA
├── vault       PDA ["vault", pool]                            token account of asset_mint, authority = pool
├── dead_shares PDA ["dead", pool]                             token account of share_mint, authority = pool
├── collection  creator-supplied Core CollectionV1             update authority = pool PDA, bare at init
└── CreditLine  PDA ["line", pool, line_id u64 LE]             program-owned, 528 bytes
    ├── Pagare  PDA ["pagare", line, draw_index u16 LE]        program-owned, 363 bytes: the pagaré record
    └── asset   PDA ["pagare_asset", line, draw_index u16 LE]  Metaplex Core AssetV1, owned by the Core
                                                               program; held by the pool PDA until
                                                               deliver_pagare, then by the debtor

council         Squads PDA ["multisig", multisig, "vault", [0]] under SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf
fx reference    8QuwResPgdQdja4gdTbvNxeUznc58XX8baUM3cCwfVW2   RedStone's MXNe PriceData (mainnet only)
event authority PDA ["__event_authority"]                      Anchor #[event_cpi]
```

The seeds are `state::seeds`.

| PDA | bump | how it is verified |
|---|---|---|
| pool | `Pool.bump`, canonical (Anchor `init`) | every instruction re-derives `["pool", pool.creator, pool.asset_mint, pool.bump]` |
| vault, dead_shares | canonical at `init`, not stored | later instructions compare the address with `Pool.vault` / `Pool.dead_shares` |
| line | `CreditLine.bump`, canonical (Anchor `init` in `request_line`) | re-derived with the stored bump in `draw`, `repay`, `mark_in_collection`, `write_off`, the council line instructions and the pagaré instructions; `approve_line` and `reject_line` use `has_one = pool`, `withdraw_request` uses `has_one = borrower` |
| pagare | client-passed `pagare_bump` at `draw`, then `Pagare.bump` | `draw` verifies the seeds with one `create_program_address`; the pagaré instructions re-derive with the stored bump; `repay` identifies its records by owner, discriminator, `line` and `draw_index` |
| pagare_asset | client-passed `asset_bump` at `draw`, not stored | `draw` verifies the seeds; later instructions take `address = pagare.asset` |
| council vault | `Pool.council_vault_bump`, canonical at `init_pool` / `accept_council` | `council::vault_address` |

- The pool address is a function of `(creator, asset_mint)`.
- `line_id` must equal `pool.line_count` at the request (`WrongAccount`), so line ids are dense
  `0, 1, 2…`. A rejected or withdrawn request closes its account; its id is not reused.
- `draw_index` is `CreditLine.draws` before the draw, so a line's pagarés are dense `0, 1, 2…`
  below `MAX_DRAWS` (64).
- The pool PDA signs every outbound CPI (share mints, vault transfers, the Core create, and the
  thaw, renunciation and transfer of `deliver_pagare`) with `pool_signer!(pool)` =
  `["pool", creator, asset_mint, bump]`.
- No account of this program is ever closed except a `Requested` line that is rejected or
  withdrawn. The pagaré asset is the only account that can disappear, burnt by its owner through
  `burn_pagare`.

All three accounts are Borsh-serialized with no padding, end in a zeroed `reserved` tail for fields
added after mainnet, and are sized by `InitSpace` (`tests/layout.rs` pins 495, 528 and 363 bytes).
Offsets below include the 8-byte Anchor discriminator, so they can be used directly in `memcmp`
filters.

| account | discriminator (hex) |
|---|---|
| `Pool` | `f19a6d0411b16dbc` |
| `CreditLine` | `dce2cd18dc978168` |
| `Pagare` | `8c8e1bef938349f7` |

### 3.1 `Pool` (495 bytes)

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 32 | `council` | vault 0 of `multisig`; signs every council instruction |
| 40 | 32 | `asset_mint` | a seed; on this build's allow-list (§12.2) |
| 72 | 32 | `share_mint` | mint authority = pool PDA |
| 104 | 32 | `vault` | `["vault", pool]` |
| 136 | 32 | `dead_shares` | `["dead", pool]` |
| 168 | 32 | `collection` | the series' Core collection |
| 200 | 8 | `outstanding_principal` | USDC cost lent out (§1.2) |
| 208 | 8 | `interest_collected` | USDC, informative |
| 216 | 8 | `written_off` | USDC cost written off or condoned, net of recoveries; informative |
| 224 | 8 | `line_count` | the next `line_id` |
| 232 | 4 | `collection_period_seconds` | from the mark to the earliest write-off; ≥ 30 days; `set_collection_period` |
| 236 | 2 | `max_line_bps` | the line cap as a share of the NAV, `1..=10 000`; `init_pool`, `set_max_line_bps` |
| 238 | 1 | `asset_controls` | bit 0: the asset mint has a freeze authority; bit 1: Token-2022 `Pausable` |
| 239 | 1 | `permissioned` | the share mint is `DefaultAccountState = Frozen` |
| 240 | 1 | `paused` | |
| 241 | 1 | `bump` | the pool PDA's bump |
| 242 | 32 | `creator` | a seed |
| 274 | 1 | `council_kind` | `COUNCIL_SQUADS` (0); `COUNCIL_GENERIC` (1) is defined and refused |
| 275 | 32 | `multisig` | the Squads multisig |
| 307 | 1 | `council_vault_bump` | |
| 308 | 32 | `pending_council` | set by `propose_council`; zero when none |
| 340 | 1 | `pending_council_kind` | |
| 341 | 8 | `max_outstanding_principal` | cap on `outstanding_principal` at every draw, ≥ 1 |
| 349 | 4 | `acceleration_notice_seconds` | ≥ 7 days |
| 353 | 8 | `impaired_principal` | the outstanding USDC cost of the lines in collection (decision 40) |
| 361 | 2 | `provision_bps` | the NAV's provision on `impaired_principal`, `0..=10 000`; 10 000 at `init_pool` |
| 363 | 32 | `fx_feeder` | zero when none |
| 395 | 8 | `fix_rate` | MXN per USD × 10⁴; zero before the first publication |
| 403 | 4 | `fix_date` | `yyyymmdd`, the date Banxico determined it; never moves back |
| 407 | 8 | `fix_posted_at` | slot time of the last `post_fix` |
| 415 | 8 | `fx_realized` | `i64`, Σ `FxRealized.delta` |
| 423 | 8 | `min_fix_rate` | the lowest FIX `post_fix` accepts; zero with `max_fix_rate` zero for no bounds |
| 431 | 8 | `max_fix_rate` | the highest |
| 439 | 8 | `recoverable` | the cost written off and still owed: `write_off` adds, a recovery and a quita on a written-off line subtract; counted by `entry_nav` (decision 51) |
| 447 | 48 | `reserved` | zero |

Methods: `asset_freezable()`, `asset_pausable()`, `max_line_principal(nav) = nav × max_line_bps /
10 000` rounded down, `entry_nav(vault)`, `nav(vault)`, `check(vault, supply)` (§1.2).

### 3.2 `CreditLine` (528 bytes)

Every field sits at one offset: `sponsor` is a key, the zero key meaning "none", as `procurador`
and `fx_feeder` are.

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 32 | `pool` | |
| 40 | 8 | `line_id` | |
| 48 | 32 | `borrower` | |
| 80 | 32 | `sponsor` | co-signed the request; the zero key when none |
| 112 | 1 | `sponsor_role` | 0 promoter, 1 guarantor |
| 113 | 32 | `agreement_hash` | never zero |
| 145 | 32 | `expediente_root` | salted merkle root of the credit file; never zero |
| 177 | 1 | `status` | `LineStatus`: 0 `Requested`, 1 `Approved`, 2 `Active`, 3 `Repaid`, 4 `WrittenOff`, 5 `InCollection`, 6 `Cancelled` |
| 178 | 8 | `principal` | MXN centavos; the commitment |
| 186 | 2 | `rate_bps` | annual, ≤ 10 000 |
| 188 | 8 | `term_end` | unix seconds |
| 196 | 8 | `availability_end` | draws accepted while `now < availability_end`; 0 until approval |
| 204 | 8 | `drawn` | MXN centavos drawn; never decreases |
| 212 | 2 | `draws` | tranches so far, ≤ 64 |
| 214 | 16 | `principal_seconds` | `u128`, centavo-seconds: `∫ (drawn − principal_repaid) dt` to `accrued_at` (decision 48) |
| 230 | 8 | `accrued_at` | the instant `principal_seconds` runs to; never past the accrual end |
| 238 | 8 | `principal_repaid` | MXN; principal settled, repaid or condoned (decision 51) |
| 246 | 8 | `interest_repaid` | MXN |
| 254 | 2 | `pagares_paid` | payment cursor: the oldest pagaré not yet marked paid |
| 256 | 8 | `first_due_at` | 0 on a bullet line |
| 264 | 8 | `interval_seconds` | 0 on a bullet line |
| 272 | 2 | `installments` | `1..=120`; 0 on a bullet line |
| 274 | 2 | `late_bps` | annual late rate, ≤ 10 000, scheduled or bullet |
| 276 | 1 | `max_missed` | `3..=installments`; 0 on a bullet line |
| 277 | 8 | `late_collected` | MXN |
| 285 | 8 | `late_paid_until` | late interest settled to this instant |
| 293 | 8 | `drawn_at` | the first draw |
| 301 | 8 | `closed_at` | `Repaid`, `WrittenOff` or `Cancelled` |
| 309 | 8 | `accelerated_at` | the mark or the council's acceleration; 0 before |
| 317 | 1 | `bump` | |
| 318 | 2 | `currency` | 484 |
| 320 | 8 | `requested_at` | |
| 328 | 8 | `approved_at` | |
| 336 | 32 | `terms_hash` | `terms::terms_hash` (§4.3) |
| 368 | 32 | `pagare_template_hash` | hash of the approved pagaré text |
| 400 | 32 | `payer` | funded the request; gets the rent on reject or withdraw |
| 432 | 8 | `availability_end_saved` | the window's end before a suspension; non-zero ⇔ suspended |
| 440 | 8 | `acceleration_due_at` | when `accelerate_line` may fire; 0 when none is pending; kept after it fires |
| 448 | 8 | `usdc_drawn` | USDC delivered over the draws |
| 456 | 8 | `usdc_cost_repaid` | USDC cost of the principal repaid |
| 464 | 64 | `reserved` | zero |

Methods: `schedule()` is `None` when `installments == 0` (a bullet line); `anchor()` =
`first_due_at − interval`; `last_due()` = `first_due_at + (installments − 1) × interval`, or
`term_end` on a bullet line; `available()` = `principal − drawn`; `outstanding()` = `drawn −
principal_repaid`; `accrual_end()`, `accrue(now)`, `interest_accrued(now)`, `interest_unpaid(now)`,
`owed(now)` and `can_draw(now)`, the accrual of decision 48 (§7.5), which every handler reads.

### 3.3 `Pagare` (363 bytes)

The program's record of one pagaré, written by `draw` next to the Core asset. It holds no folio and
nothing personal and is never closed: after the payment, the delivery and even the owner's burn it
stays as the PAGADO mark and the registry. A título the council's quita extinguished is
`PAGARE_PAID` too, with `PAGARE_FLAG_CONDONED` (decision 51): deliverable like a paid one, naming no
payer.

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 32 | `line` | |
| 40 | 2 | `draw_index` | |
| 42 | 32 | `asset` | `["pagare_asset", line, draw_index]` |
| 74 | 32 | `pagare_hash` | derived on chain at `draw` (§4.3) |
| 106 | 8 | `amount_mxn` | the sum the pagaré promises, MXN centavos |
| 114 | 8 | `issued_at` | |
| 122 | 8 | `due_at` | the line's `last_due()` at issuance |
| 130 | 8 | `drawn_through` | the line's `drawn` after this tranche |
| 138 | 32 | `cert_fingerprint` | `sha256(salt ‖ fingerprint)` of the PSC or e.firma certificate; zero only with `signature_method` 0 |
| 170 | 1 | `signature_method` | 0 wallet, 1 PSC + wallet, 2 SAT e.firma + wallet |
| 171 | 1 | `status` | 0 `PAGARE_OUTSTANDING`, 1 `PAGARE_PAID`; never reverts (2 is retired, never reused) |
| 172 | 8 | `paid_at` | when the covering repayment or quita landed; zero while outstanding |
| 180 | 32 | `receipt_hash` | the covering repayment's receipt hash, the payer's and provisional, or the quita's `document_hash`; never rewritten (a correction is a `Recibo` entry) |
| 212 | 32 | `procurador` | the procurador in force; zero when none (`PAGARE_PROCURADOR_OFFSET = 212` in `clients/js/src/pagare.ts`) |
| 244 | 1 | `flags` | bit 1 `PROCURACION` in force, bit 2 `DELIVERED`, bit 3 `PAID_BY_AVAL`, bit 5 `CONDONED` (decision 51); bits 0 and 4 are retired, never reused |
| 245 | 32 | `log_root` | the registry hash chain (§9.3) |
| 277 | 4 | `entry_count` | `u32`; 1 after the draw |
| 281 | 1 | `last_kind` | `KIND_*` of the last entry |
| 282 | 1 | `bump` | |
| 283 | 8 | `fix_rate` | the FIX the borrower signed for this draw |
| 291 | 4 | `fix_date` | its date |
| 295 | 8 | `usdc_delivered` | `usdc_for_mxn(amount_mxn, fix_rate)`: the pool's cost of the tranche |
| 303 | 32 | `delivery_hash` | `sha256(pagare_hash ‖ fix_rate ‖ fix_date ‖ usdc_delivered)` |
| 335 | 28 | `reserved` | zero |

### 3.4 Constants

| constant | value | module | role |
|---|---|---|---|
| `DEAD_SHARES` | 1 000 | `state` | shares locked by the first deposit |
| `MIN_FIRST_DEPOSIT_DEAD_RATIO` | 1 000 | `state` | first deposit ≥ 1 000 × `DEAD_SHARES` base units |
| `BPS` | 10 000 | `state` | |
| `MAX_DRAWS` | 64 | `state` | tranches per line |
| `MAX_SIDECARS_PER_REPAY` | 3 | `state` | pagaré records one `repay` takes |
| `MAX_INSTALLMENTS` | 120 | `state` | bounds the arrears loop |
| `MIN_MAX_MISSED` | 3 | `state` | |
| `MIN_COLLECTION_PERIOD_SECONDS` | 2 592 000 (30 d) | `state` | |
| `SECONDS_PER_YEAR` | 31 536 000 (365 d) | `state` | annualizes `rate_bps` and `late_bps` |
| `CURRENCY_MXN` | 484 | `state` | |
| `MAX_COUNCIL_MEMBERS` | 10 | `state` | |
| `REQUEST_TTL_SECONDS` | 1 296 000 (15 d) | `state` | approval window from the request |
| `MAX_PROPOSAL_HORIZON_SECONDS` | 1 296 000 (15 d) | `state` | furthest `valid_until` ahead |
| `MIN_ACCELERATION_NOTICE_SECONDS` | 604 800 (7 d) | `state` | |
| `SPONSOR_PROMOTER`, `SPONSOR_GUARANTOR` | 0, 1 | `state` | `sponsor_role` |
| `COUNCIL_SQUADS`, `COUNCIL_GENERIC` | 0, 1 | `state` | `council_kind` |
| `FIX_SCALE` | 10 000 | `fx` | FIX × 10⁴ |
| `MAX_FIX_AGE_DAYS` | 5 | `fx` | oldest FIX an approval or a draw accepts (skipped when the bounds pin the FIX, `Pool::fix_pinned`, decision 42), oldest date the council publishes |
| `MAX_FIX_JUMP_BPS_PER_DAY`, `MAX_FIX_JUMP_DAYS` | 500, 3 | `fx` | the feeder's band: 5% a day between the FIX dates, one to three days, 15% at most |
| `MAX_REFERENCE_AGE_SECONDS` | 93 600 (26 h) | `fx` | the RedStone feed's 24-hour heartbeat plus two hours |
| `MAX_FIX_REFERENCE_GAP_BPS` | 200 | `fx` | the feeder's FIX within 2% of the reference |
| `REFERENCE_PROGRAM` | `REDSTBDUecGjwXd6YGPzHSvEUBHQqVRfCcjUVgPiHsr` | `fx` | RedStone's Solana price adapter |
| `REFERENCE_ACCOUNT` | `8QuwResPgdQdja4gdTbvNxeUznc58XX8baUM3cCwfVW2` | `fx` | its MXNe `PriceData`, PDA `["price", "MXNe"]` (seeds zero-padded to 32 bytes) |
| `PAYMENT_DOMAIN` | `"elote.pago.v2"` | `registry` | domain of a payment entry's annotation hash |
| `TERMS_DOMAIN` | `"elote_pool:terms:v1"` | `terms` | domain of the terms hash |

## 4. Instructions

38 instructions in `lib.rs`. For each: accounts (beyond the two `#[event_cpi]` accounts every
instruction takes), arguments, checks in the order the handler makes them, the errors it returns,
what it writes and the events it emits. Anchor's own constraint errors (seeds, `has_one` mapped to
the codes shown, `AccountNotInitialized`, …) are raised before the handler. **C** marks the
instructions that run `council::check`; **V** the ones that take `valid_until`.

### 4.1 Summary

| instruction | module | signers | V | C | paused |
|---|---|---|---|---|---|
| `init_pool` | `pool` | creator | | on the given multisig | n/a |
| `deposit` | `pool` | depositor | | | blocked |
| `redeem` | `pool` | holder | | | blocked |
| `pause` | `pool` | council | V | C | allowed |
| `resume` | `pool` | council | V | C | allowed |
| `set_max_line_bps` | `pool` | council | V | C | allowed |
| `set_provision_bps` | `pool` | council | V | C | allowed |
| `set_outstanding_cap` | `pool` | council | V | C | allowed |
| `set_fix_bounds` | `pool` | council | V | C | allowed |
| `set_acceleration_notice` | `pool` | council | V | C | allowed |
| `set_collection_period` | `pool` | council | V | C | allowed |
| `propose_council` | `pool` | council | V | C | allowed |
| `accept_council` | `pool` | proposed vault | V | on the new multisig | allowed |
| `set_fx_feeder` | `pool` | council | V | C | allowed |
| `post_fix` | `pool` | feeder or council | V | council path | allowed |
| `request_line` | `lines` | borrower, payer, sponsor if named | | | blocked |
| `approve_line` | `lines` | council | V | C | blocked |
| `reject_line` | `lines` | council | V | C | allowed |
| `withdraw_request` | `lines` | borrower | | | allowed |
| `cancel_line` | `lines` | council | V | C | allowed |
| `draw` | `lines` | borrower, payer | | | blocked |
| `repay` | `lines` | payer | | | allowed |
| `mark_in_collection` | `lines` | anyone (fee payer) | | | allowed |
| `write_off` | `lines` | council | V | C | allowed |
| `suspend_draws` | 16 532 |
| `resume_draws` | 15 849 |
| `condone` | `lines` | council | V | C | allowed |
| `renounce_availability` | `lines` | borrower | | | allowed |
| `propose_acceleration` | 16 544 |
| `withdraw_acceleration` | `lines` | council | V | C | allowed |
| `accelerate_line` | `lines` | council | V | C | allowed |
| `deliver_pagare` | `pagare` | anyone (payer) | | | allowed |
| `burn_pagare` | 31 004 |
| `annul_pagare` | `pagare` | council (holder) | V | C | allowed |
| `endorse_pagare` | `pagare` | council (holder) | V | C | allowed |
| `revoke_procuracion` | `pagare` | council (holder) | V | C | allowed |
| `court_annotation` | `pagare` | council (holder) | V | C | allowed |
| `annotate_payment` | `pagare` | council (annotator) | V | C | allowed |

The pause blocks exactly `deposit`, `redeem`, `request_line`, `approve_line` and `draw`. Money can
always come back in, the FIX can always be published, and facts can always be recorded.

Council instructions of the shape `CouncilPool` (`council` signer, `multisig`, `pool` mut,
`has_one = council` → `Unauthorized`) and `CouncilLine` (the same plus `line` mut with its seeds and
stored bump, `has_one = pool` → `WrongAccount`) share their accounts. `SetCollectionPeriod`,
`ProposeCouncil` and `SetFxFeeder` have the `CouncilPool` shape under their own names. The five
pagaré council instructions take `holder` (or `annotator`), `multisig`, `pool`, `line` (seeds,
`has_one = pool`) and `pagare` (mut, `["pagare", line, draw_index]` with its stored bump,
`has_one = line` → `WrongAccount`); none of them touches the Core asset.

### 4.2 Pool and council

#### `init_pool(params: InitPoolParams)`

- **Accounts:** `creator` (signer, mut, pays); `asset_mint`; `pool` (`init` at
  `["pool", creator, asset_mint]`); `share_mint` (existing, validated); `vault` (`init` at
  `["vault", pool]`, mint `asset_mint`, authority pool, `token_program`); `dead_shares` (`init` at
  `["dead", pool]`, mint `share_mint`, authority pool, `share_token_program`); `collection`;
  `multisig` (does not sign); `token_program`; `share_token_program`; `system_program`.
- **Args** (`InitPoolParams`): `max_line_bps: u16`, `collection_period_seconds: u32`,
  `max_outstanding_principal: u64`, `acceleration_notice_seconds: u32`, `min_fix_rate: u64`,
  `max_fix_rate: u64`.
- **Checks, in order:** `1 <= max_line_bps <= 10 000` (`WrongAmount`); `max_outstanding_principal
  >= 1` (`WrongAmount`); collection period ≥ 30 d (`CollectionPeriodTooShort`); acceleration
  notice ≥ 7 d (`AccelerationNoticeTooShort`); `fx::bounds_valid(min, max)`: both zero, or `min <=
  max` with `max > 0` (`WrongAmount`); `council::check` on `multisig` with its canonical vault
  (`CouncilInvalid`, `CouncilTooLarge`); `asset_listed(asset_mint)` (`AssetNotListed`, §12.2);
  asset mint ≠ share mint (`AssetIsShareMint`); the asset mint has 6 decimals, the scale
  `fx::usdc_for_mxn` counts in (`AssetDecimalsInvalid`).
  - The asset-mint policy (`asset_controls`) refuses `TransferFeeConfig`, `PermanentDelegate`,
    `TransferHook`, `NonTransferable`, the three confidential extensions, `MintCloseAuthority`,
    and `DefaultAccountState` other than `Initialized` (`AssetMintNotAllowed`); it records a
    freeze authority and `Pausable` as bits, and accepts everything else unrecorded.
  - The share-mint policy (`validate_share_mint`): owned by `share_token_program`, mint authority =
    pool, supply 0, decimals = the asset's; refuses `TransferFeeConfig`, `TransferHook`,
    `MintCloseAuthority`, `PermanentDelegate`, `InterestBearingConfig`, `ScaledUiAmount`,
    `Pausable` and the confidential extensions; `DefaultAccountState = Frozen` sets `permissioned`
    and requires a freeze authority (`ShareMintNotAllowed`). `NonTransferable` is allowed.
  - The collection (`validate_collection`): owned by Core, key `CollectionV1`, update authority =
    pool, exactly the base record with no plugin header after it, `num_minted == 0`,
    `current_size == 0`; every read is at a fixed offset or bounded by a length read before it
    (`CollectionNotAllowed`).
- **Writes:** every `Pool` field: `council` = the canonical vault 0 of `multisig`, `council_kind =
  COUNCIL_SQUADS`, `council_vault_bump`; `provision_bps = 10 000`; `impaired_principal`, the FIX fields, `fx_feeder`, the pending council and `reserved` zero. On a
  permissioned share mint the dead account is born frozen and is thawed by the freeze authority
  before the first deposit.
- **Events:** `PoolInitialized`.

#### `deposit(assets: u64, min_shares_out: u64)`

- **Accounts:** `depositor` (signer); `pool`; `asset_mint` (= `pool.asset_mint`); `share_mint`
  (mut, = `pool.share_mint`); `vault` (mut); `dead_shares` (mut); `depositor_asset_account` (mut,
  mint and owner checked); `depositor_share_account` (mut, mint and owner checked);
  `token_program`; `share_token_program`. Address mismatches are `WrongAccount`.
- **Checks:** not paused (`Paused`); `assets > 0` (`WrongAmount`); supply and `entry_nav(vault)`
  read before any transfer (`Overflow`); on the first deposit `assets >= min_first_deposit(decimals)`
  (`FirstDepositTooSmall`); `minted = shares_for_deposit(assets, supply, entry NAV)` (`Overflow`,
  which also covers an entry NAV of zero with `supply > 0`); `shares = minted − DEAD_SHARES` on the
  first deposit (`Overflow`); `shares > 0` (`ZeroShares`); `shares >= min_shares_out`
  (`SlippageExceeded`).
- **CPIs:** `transfer_checked` depositor → vault; `mint_to` shares → depositor (pool signs); on the
  first deposit `mint_to` 1 000 → `dead_shares`.
- **Writes:** no `Pool` field. `Pool::check` on the reloaded vault and supply.
- **Events:** `Deposited`, `NavUpdated`.

#### `redeem(shares: u64, min_assets_out: u64)`

- **Accounts:** `holder` (signer); `pool`; `asset_mint`; `share_mint` (mut); `vault` (mut);
  `holder_share_account` (mut); `holder_asset_account` (mut); `token_program`;
  `share_token_program`.
- **Checks:** not paused; `shares > 0` (`WrongAmount`); `assets = assets_for_redeem(shares, supply,
  nav(vault))` on the pre-burn supply at the provisioned NAV (`Overflow`); `assets > 0`
  (`WrongAmount`); `assets >= min_assets_out` (`SlippageExceeded`); `assets <= vault.amount`
  (`InsufficientLiquidity`).
- **CPIs:** `burn` (the holder signs); `transfer_checked` vault → holder (pool signs).
- **Writes:** no `Pool` field; `Pool::check`.
- **Events:** `Redeemed`, `NavUpdated`.

#### `pause(valid_until)` — **V C**

- **Accounts:** `CouncilPool`. **Checks:** `ProposalExpired` / `ProposalTooFar`; `council::check`.
  No member and no other key pauses alone (decision 39); an emergency stop needs the council's
  majority.
- **Writes:** `paused = true`. **Events:** `PoolPaused { by = pool.council }`.

#### `resume(valid_until)` — **V C**

- **Accounts:** `CouncilPool`. **Checks:** `valid_until`; `council::check`.
- **Writes:** `paused = false`. **Events:** `PoolResumed`.

#### `set_max_line_bps(max_line_bps: u16, valid_until)` — **V C**

- **Checks:** `valid_until`; `1..=10 000` (`WrongAmount`); `council::check`. Lines already
  approved keep their principal and draw within the new cap.
- **Writes:** `max_line_bps`. **Events:** `MaxLineBpsSet`.

#### `set_provision_bps(provision_bps: u16, valid_until)` — **V C**

- **Checks:** `valid_until`; `0..=10 000` (`WrongAmount`); `council::check`.
- **Writes:** `provision_bps` (decision 40); the redemption NAV moves at once. **Events:**
  `ProvisionSet`.

#### `set_outstanding_cap(max_outstanding_principal: u64, valid_until)` — **V C**

- **Checks:** `valid_until`; `>= 1` (`WrongAmount`); `council::check`. A cap below the current
  outstanding stops new draws only.
- **Writes:** `max_outstanding_principal`. **Events:** `OutstandingCapSet`.

#### `set_fix_bounds(min_fix_rate: u64, max_fix_rate: u64, valid_until)` — **V C**

- **Checks:** `valid_until`; `fx::bounds_valid` (`WrongAmount`); `council::check`. The FIX in
  force is not re-checked.
- **Writes:** `min_fix_rate`, `max_fix_rate` (zero and zero remove the bounds). **Events:**
  `FixBoundsSet`.

#### `set_acceleration_notice(acceleration_notice_seconds: u32, valid_until)` — **V C**

- **Checks:** `valid_until`; ≥ 7 d (`AccelerationNoticeTooShort`); `council::check`. A pending
  proposal keeps its own deadline.
- **Writes:** `acceleration_notice_seconds`. **Events:** `AccelerationNoticeSet`.

#### `set_collection_period(collection_period_seconds: u32, valid_until)` — **V C**

- **Checks:** `valid_until`; ≥ 30 d (`CollectionPeriodTooShort`); `council::check`. Applies to
  every line from its next `write_off`, marked or not.
- **Writes:** `collection_period_seconds`. **Events:** `CollectionPeriodSet`.

#### `propose_council(new_council: Pubkey, kind: u8, valid_until)` — **V C**

- **Checks:** `valid_until`; `kind == COUNCIL_SQUADS` (`CouncilKindInvalid`); `council::check` on
  the current multisig.
- **Writes:** `pending_council = new_council`, `pending_council_kind = kind`; a new proposal
  replaces a pending one and the zero key withdraws it.
- **Events:** `CouncilProposed`.

#### `accept_council(valid_until)` — **V**, check on the new multisig

- **Accounts:** `new_council` (signer); `multisig` (the new one, any address); `pool` (mut).
- **Checks:** `valid_until`; `pending_council` non-zero and equal to the signer
  (`NoPendingCouncil`); `pending_council_kind == COUNCIL_SQUADS` (`CouncilKindInvalid`); the
  canonical vault 0 of `multisig` equals the signer (`CouncilInvalid`); `council::check` on
  `multisig`.
- **Writes:** `council`, `council_kind`, `multisig`, `council_vault_bump`; the pending fields
  cleared.
- **Events:** `CouncilAccepted { old, new, kind }`.

#### `set_fx_feeder(fx_feeder: Pubkey, valid_until)` — **V C**

- **Checks:** `valid_until`; `council::check`.
- **Writes:** `fx_feeder` (zero for none: then only the council publishes). **Events:**
  `FxFeederSet`.

#### `post_fix(fix_rate: u64, fix_date: u32, valid_until)` — **V**, **C** on the council path

See §6.

### 4.3 Lines

#### `request_line(line_id: u64, terms: LineTerms)`

- **Accounts:** `borrower` (signer); `payer` (signer, mut, pays the line's rent); `sponsor`
  (optional signer); `pool` (mut); `line` (`init` at `["line", pool, line_id]`, payer `payer`);
  `system_program`.
- **Args** (`LineTerms`): `principal` (MXN centavos), `rate_bps`, `late_bps`, `term_end`,
  `agreement_hash`, `sponsor: Option<Pubkey>`, `sponsor_role`, `schedule: Option<ScheduleParams>`
  (`first_due_at`, `interval_seconds`, `installments`, `max_missed`), `expediente_root`,
  `pagare_template_hash`.
- **Checks, in order:** not paused (`Paused`); `line_id == pool.line_count` (`WrongAccount`);
  borrower ≠ council (`BorrowerIsCouncil`); `principal > 0` (`WrongAmount`); `rate_bps` and
  `late_bps` ≤ 10 000 (`WrongAmount`); `term_end > now` (`TermEnded`); non-zero `agreement_hash`
  (`AgreementHashRequired`), `expediente_root` (`ExpedienteRootRequired`) and
  `pagare_template_hash` (`PagareTemplateHashRequired`); `sponsor_role <= 1`
  (`SponsorRoleInvalid`); a named sponsor signed (`SponsorRequired`); a schedule passes
  `validate_schedule`: `1 <= installments <= 120` and `interval > 0` (`InvalidSchedule`),
  `max_missed >= 3` (`MaxMissedTooLow`), `max_missed <= installments` and `first_due_at > now`
  (`InvalidSchedule`), the last due date ≤ `term_end` (`ScheduleEndsAfterTerm`),
  `principal >= installments`, one centavo of principal per installment (`InvalidSchedule`,
  decision 48).
- **Writes:** every `CreditLine` field: `status = Requested`, `currency = 484`, `requested_at =
  now`, `terms_hash`, `payer`, the schedule fields (zero on a bullet line), everything else zero;
  `pool.line_count += 1`.
- **Events:** `LineRequested` (every term and the terms hash).

The terms hash (`terms::terms_hash`) is `sha256("elote_pool:terms:v1" ‖ pool ‖ line_id u64 ‖
borrower ‖ currency u16 ‖ principal u64 ‖ rate_bps u16 ‖ term_end i64 ‖ first_due_at i64 ‖
interval_seconds i64 ‖ installments u16 ‖ late_bps u16 ‖ max_missed u8 ‖ sponsor_present u8 ‖
sponsor (zero key when none) ‖ sponsor_role u8 ‖ agreement_hash ‖ pagare_template_hash ‖
expediente_root)`, integers little endian, the schedule fields zero on a bullet line.
`clients/js/src/terms.ts` reproduces it against a vector both test suites pin.

#### `approve_line(expediente_root: [u8; 32], terms_hash: [u8; 32], valid_until)` — **V C**

- **Accounts:** `council`; `multisig`; `pool` (`has_one = council`); `line` (mut, `has_one = pool`
  → `WrongAccount`); `vault` (= `pool.vault`, for the NAV).
- **Checks, in order:** `valid_until`; not paused (`Paused`); `council::check`; `status ==
  Requested` (`LineStatus`); `now <= requested_at + 15 d` (`RequestExpired`); both hashes equal the
  stored ones (`ExpedienteMismatch`); the borrower is not a member of the multisig
  (`BorrowerIsCurator`); a FIX is published (`FixStale`) and, unless the bounds pin it
  (`fix_pinned`: `min_fix_rate == max_fix_rate != 0`, decision 42), is at most `MAX_FIX_AGE_DAYS`
  (5) old in Mexico City days (`FixStale`); `principal <= mxn_for_usdc(max_line_principal(nav), fix_rate)`,
  the cap measured on the provisioned NAV now and converted at the FIX in force (`LineTooLarge`);
  the window end (`first_due_at`, or `term_end` on a bullet line) is after now (`TermEnded`).
- **Writes:** `status = Approved`, `approved_at = now`, `availability_end`.
- **Events:** `LineApproved { expediente_root, terms_hash, availability_end }`.

#### `reject_line(valid_until)` — **V C**

- **Accounts:** `council`; `multisig`; `pool` (`has_one = council`); `line` (mut, `has_one =
  pool`, `has_one = payer` → `WrongAccount`, `status == Requested` → `LineStatus`, `close =
  payer`); `payer` (mut).
- **Checks:** `valid_until`; `council::check`.
- **Writes:** the line account is closed and its rent goes to `payer`. The id stays taken.
- **Events:** `LineRejected`.

#### `withdraw_request()`

- **Accounts:** `borrower` (signer); `line` (mut, `has_one = borrower` → `Unauthorized`, `has_one =
  payer`, `status == Requested`, `close = payer`); `payer` (mut).
- Expired or not. **Writes:** the account is closed, rent to `payer`. **Events:**
  `RequestWithdrawn`.

#### `cancel_line(valid_until)` — **V C**

- **Accounts:** `CouncilLine`. **Checks:** `valid_until`; `council::check`; `Approved` and
  `drawn == 0` (`LineStatus`).
- **Writes:** `status = Cancelled`, `closed_at = now`. **Events:** `LineCancelled`.

#### `draw(amount_mxn, fix_rate, fix_date, asset_bump, pagare_bump, cert_fingerprint, signature_method)`

- **Accounts:** `borrower` (signer, alone: the signature covers the FIX and the pagaré the program
  derives); `pool` (mut); `line` (mut, seeds, `has_one = pool` → `WrongAccount`, `has_one =
  borrower` → `Unauthorized`); `asset_mint`; `vault` (mut); `borrower_asset_account` (mut, mint
  and owner); `token_program`; `payer` (signer, mut, rent of the asset and the record); `asset`
  (mut, `["pagare_asset", line, line.draws]` with `asset_bump`); `pagare` (mut, `["pagare", line,
  line.draws]` with `pagare_bump`); `collection` (mut, = `pool.collection`); `mpl_core_program`
  (= Core's id); `system_program`. With the bumps given, each seeds constraint is one
  `create_program_address`, so the compute of a draw does not depend on the keys.
- **Checks, in order (all before any write):** not paused; status `Approved` or `Active`
  (`LineStatus`); `0 < amount_mxn <= principal − drawn` (`WrongAmount`); `now < availability_end`
  (`AvailabilityEnded`); a FIX is published and the signed `fix_rate`, `fix_date` equal the pool's
  (`FixMismatch`); unless the bounds pin the FIX (`fix_pinned`, decision 42), it is at most 5 days
  old in Mexico City days (`FixStale`); `drawn >= installments` after the tranche, one centavo of
  principal per installment (`InvalidSchedule`, decision 48: no installment is empty, so the
  arrears arithmetic never divides by zero); `draws < 64`
  (`TooManyDraws`); `usdc = usdc_for_mxn(amount_mxn, fix_rate) > 0` (`WrongAmount`); `usdc <=
  vault.amount` (`InsufficientLiquidity`); `usdc_drawn + usdc <= max_line_principal(nav)`, the
  line's whole USDC drawn against the provisioned NAV now (`LineTooLarge`); `outstanding + usdc <=
  max_outstanding_principal` (`OutstandingCapExceeded`); `signature_method <= 2` and a non-zero
  `cert_fingerprint` unless the method is 0 (`SignatureMethodInvalid`).
- **Derived on chain:** `pagare_hash = sha256(pagare_template_hash ‖ terms_hash ‖ line ‖
  draw_index u16 ‖ amount_mxn u64 ‖ now i64)`; `delivery_hash = sha256(pagare_hash ‖ fix_rate u64 ‖
  fix_date u32 ‖ usdc u64)`. No interest is fixed (decision 48).
- **CPIs:** `transfer_checked` of `usdc` vault → borrower (pool signs); any lamports already on the
  asset or record address are swept to `payer` (signed by each PDA), because both creations
  require an unfunded address and both addresses are predictable; Core `CreateV2` of the asset
  (§9.1), the pool PDA as authority and owner, the asset PDA signing for its address;
  `create_account` of the record.
- **Writes:** `pool.outstanding_principal += usdc`; `line.accrue(now)` (§7.5), then `line.drawn
  += amount_mxn`, `usdc_drawn += usdc`, `draws += 1`; on the first tranche `status = Active`, `drawn_at = now`; the `Pagare` record with the
  issuance entry (`entry_count = 1`, `last_kind = Emision`). `Pool::check` on the reloaded vault.
- **Events:** `LineDrawn`, `PagareIssued`, `PagareEntry` (Emision).

#### `repay(max_usdc: u64, fix_rate: u64, fix_date: u32, receipt_hash: [u8; 32])`

- **Accounts:** `payer` (signer: anyone, the borrower, a sponsor, a procurador or a third party);
  `pool` (mut); `line` (mut, seeds, `has_one = pool`); `asset_mint`; `share_mint` (for the
  supply); `vault` (mut); `payer_asset_account` (mut, mint and owner); `token_program`.
  **Remaining:** exactly the records `[pagares_paid ..= last_touched]`, writable, oldest first,
  where `last_touched` is the first record this payment leaves partly unpaid, or the last draw;
  none once the cursor has passed the last draw; never more than `MAX_SIDECARS_PER_REPAY` (3).
  `clients/js/src/pagare.ts` (`repaySidecars`) computes the set and splits a larger payment.
- **Checks and arithmetic, in order:**
  1. status `Active`, `InCollection` or `WrittenOff` (a recovery, decision 51) (`LineStatus`); a FIX is published and the signed one equals
     the pool's, **whatever its age** (`FixMismatch`): a payment is never refused for an old FIX;
  2. `amount = mxn_for_usdc(max_usdc, fix_rate)`; `amount > 0` (`WrongAmount`);
  3. `late_part = line_arrears(line, now).late_due` (§7); `amount >= late_part`
     (`LateInterestUnpaid`);
  4. `principal_left = drawn − principal_repaid`, `interest_unpaid = interest_accrued(now) −
     interest_repaid` (§7.5). While the line can still draw (`can_draw`: `Active`,
     `now < availability_end`, something undrawn), a payment when nothing is payable (no
     principal, no interest unpaid, no late interest) is refused (`WrongAmount`): never a payment
     that moves nothing, and no third party can close the line and forfeit the borrower's
     commitment;
  5. `math::allocate(amount, late_part, interest_unpaid, principal_left)`: late interest, then
     `interest_part = min(amount − late_part, interest_unpaid)`, then `principal_part =
     min(rest, principal_left)`; `usdc_in = usdc_ceil_for_mxn(late + interest + principal,
     fix_rate)`, never more than `max_usdc`; `usdc_principal = usdc_ceil_for_mxn(principal_part,
     fix_rate)`;
  6. the remaining accounts: at most three (`TooManySidecars`); none when `pagares_paid >= draws`,
     at least one otherwise; each loaded as a program-owned `Pagare` with its discriminator, naming
     this line and the expected index, `PAGARE_OUTSTANDING`; an uncovered record must end the set
     and a covered one may end it only if it is the last draw (`PagareAccountsMismatch`);
  7. `cost` = Σ over the records of `cost_of(usdc_delivered, amount_mxn, applied after) −
     cost_of(…, applied before)`; `delta = usdc_principal − cost`.
- **CPIs:** one `transfer_checked` payer → vault of `usdc_in`, when non-zero.
- **Writes:** `pool.outstanding_principal −= cost`; on an `InCollection` line also
  `pool.impaired_principal −= cost` (the recovery releases its provision, decision 40); on a
  `WrittenOff` line neither — the write-off already took the cost out — but `pool.written_off −=
  cost` (decision 51: the principal recovered is income against the loss recognised);
  `pool.interest_collected += usdc_for_mxn(interest_part)`; `pool.fx_realized += delta`;
  `line.accrue(now)` before the principal moves (decision 48: every second before this one
  accrued on the principal it had; the interest accrued at `now` is unchanged by it);
  `line.principal_repaid`, `interest_repaid`, `usdc_cost_repaid += cost`; when `late_part > 0`,
  `late_collected += late_part` and `late_paid_until = now` (an accrual that rounds to zero keeps
  running); `status = Repaid`, `closed_at = now` when nothing remains (`owed(now)` after the
  writes, `(principal_left − principal_part) + (interest_unpaid − interest_part)`, the
  `LineRepaid.remaining`) and the line cannot draw. The
  payment entries (§9.3): per record, oldest first, a `Pagado`, `PagadoAval` or `Abono` entry; a
  covered record gets `status = PAGARE_PAID`, `paid_at`, `receipt_hash`, `PAID_BY_AVAL` when the
  aval paid, and the cursor moves past it; a procuración in force on it ends with a `Terminacion`
  entry. `Pool::check` with the supply.
- **Events, in order:** per record the payment's `PagareEntry` (and the `Terminacion`'s);
  `PagarePayment` when records were passed; `LateInterestCollected` when `late_part > 0`;
  `FxRealized` when principal was applied or `delta != 0`; on a `WrittenOff` line `LossRecovered {
  late_part, interest_part, principal_part, cost_recovered, received, written_off_after, nav_after
  }`; `LineRepaid`; `NavUpdated`.

A payment on a zero debt of a line that cannot draw is the closing repayment: it must still credit
at least one centavo (`amount > 0`), moves nothing (`usdc_received = 0`) and sets `Repaid`. The
delivery of the títulos is a separate instruction, `deliver_pagare`, refused until the line owes
nothing (decision 49): the client appends it to the transaction of the payment that settles the
line, and sends what does not fit there right after (§9.4).

#### `mark_in_collection()`

- **Accounts:** `pool` (mut); `line` (mut, seeds, `has_one = pool`). No signer beyond the fee
  payer.
- **Checks:** scheduled (`NotScheduled`); `status == Active` (`LineStatus`); with `a =
  line_arrears(now)`, `needed = min(max_missed, installments − covered)` (decision 41): `needed >
  0` and `a.missed >= needed`, and `now > anchor + k × interval` with `k = min(covered +
  max_missed, installments)`, the due date of the last missed installment it counts
  (`NotEnoughMissed`). A line paid ahead that stops paying is marked one second after its last due
  date; a fully covered schedule has nothing to miss.
- **Writes:** `line.accrue(now)`, then `status = InCollection`, `accelerated_at = now`: ordinary
  interest accrues to the mark and stops there, nothing is cut (decision 48, §7.4);
  `acceleration_due_at = 0`; `pool.impaired_principal += usdc_drawn − usdc_cost_repaid` (decision
  40).
- **Events:** `LineInCollection { missed, remaining = owed(now), impaired_principal }`;
  `AccelerationWithdrawn` when it superseded a pending council proposal.

#### `write_off(valid_until)` — **V C**

- **Accounts:** `council`; `multisig`; `pool` (mut, `has_one = council`); `line` (mut, seeds,
  `has_one = pool`); `share_mint`; `vault`.
- **Checks:** `valid_until`; `council::check`; with `after_term = now > term_end`: `Active`
  requires `after_term` (`TermNotEnded`); `InCollection` requires (`after_term`, or on a scheduled
  line `now > first_missed_due` with a missed installment still unpaid, or a bullet line) and `now >
  accelerated_at + collection_period_seconds` (`WriteOffTooEarly`); any other status `LineStatus`.
- **Writes:** `loss = drawn − principal_repaid` (MXN), `loss_usdc = usdc_drawn −
  usdc_cost_repaid`; `pool.outstanding_principal −= loss_usdc`; `pool.written_off += loss_usdc`;
  from `InCollection` also `pool.impaired_principal −= loss_usdc` (the provision becomes the loss);
  `status = WrittenOff`, `closed_at = now`. No CPI. No other line field moves: the debt stays owed
  (the write-off is the pool's loss, not the debtor's release), ordinary interest stays stopped
  where it stopped (the last due date, or the acceleration), late interest keeps running on the
  principal owed within its lifetime cap, and the line still takes `repay` (a recovery) and
  `condone` (decision 51).
- **Events:** `LineWrittenOff { loss, loss_usdc, nav_after }`, `NavUpdated`.

#### `suspend_draws(valid_until)` — **V C**

- **Accounts:** `CouncilLine`. **Checks:** `valid_until`; `council::check`; status `Approved` or
  `Active` (`LineStatus`); `now < availability_end` (`WindowClosed`).
- **Writes:** `availability_end_saved = availability_end`, `availability_end = now`.
- **Events:** `LineDrawsSuspended { available_forfeited = principal − drawn }`.

#### `resume_draws(valid_until)` — **V C**

- **Accounts:** `CouncilLine`. **Checks:** `valid_until`; `council::check`; `Approved` or `Active`
  (`LineStatus`); suspended (`DrawsNotSuspended`); no acceleration pending (`AccelerationState`).
- **Writes:** `availability_end = availability_end_saved` (never later), `availability_end_saved
  = 0`.
- **Events:** `LineDrawsResumed { availability_end }`.

#### `condone(interest: u64, principal: u64, document_hash: [u8; 32], valid_until)` — **V C**

The quita (decisions 50 and 51; CCF 2209: what is condoned is extinguished). One instruction for the
whole remission a written record documents, in the payment order where it applies.

- **Accounts:** `council`; `multisig`; `pool` (mut, `has_one = council`); `line` (mut, seeds,
  `has_one = pool`); `share_mint`; `vault`. **Remaining:** with `principal > 0`, exactly the
  records of `repay` for a principal of `principal` (`[pagares_paid ..= last_touched]`, at most
  three, a larger quita split into several calls; `repaySidecars` computes them); none when
  `principal == 0`.
- **Checks, in order:** `valid_until`; `council::check`; `document_hash != 0`
  (`DocumentHashRequired`); `Active`, `InCollection` or `WrittenOff` (`LineStatus`); `interest <=
  interest_unpaid(now)`, `principal <= drawn − principal_repaid`, and something to condone —
  `interest`, `principal` or late interest accrued (`WrongAmount`); the records as in `repay`
  (`TooManySidecars`, `PagareAccountsMismatch`); `cost` as in `repay`, each pagaré at its own FIX.
- **Writes:** on an `Active` or `InCollection` line `pool.outstanding_principal −= cost`, in
  collection `pool.impaired_principal −= cost`, and `pool.written_off += cost`: the loss is
  recognised now, as a write-off of that cost would; on a `WrittenOff` line the pool does not move
  (the write-off took it). `line.accrue(now)`; when late interest has accrued, `late_paid_until =
  now` (all of it is condoned; `late_collected` is untouched); `interest_repaid += interest`;
  `principal_repaid += principal`, `usdc_cost_repaid += cost` — condoned principal counts as
  settled, so it meets installments (arrears) and accrues no interest from now. Unlike `repay`, the
  quita may condone principal and leave ordinary interest unpaid: the council remits what the
  record says, and that interest stays owed. When the line then owes nothing and can draw nothing
  more: `status = Repaid`, `closed_at = now`, and `deliver_pagare` opens (decision 49). The entries
  (§9.3): per record the principal reaches, a `Condonacion` entry (pool → borrower, over
  `condonation_hash`); a record it extinguishes gets `status = PAGARE_PAID`, `paid_at = now`,
  `receipt_hash = document_hash`, `flags |= CONDONED` (never `PAID_BY_AVAL`), the cursor moves past
  it and a procuración in force ends with a `Terminacion` entry. No funds move, no `FxRealized`.
  `Pool::check` with the supply.
- **Events, in order:** per record the `PagareEntry` (and the `Terminacion`'s); `LineCondoned {
  document_hash, late_interest, interest, principal, cost, loss_recognized, remaining, nav_after,
  condoned_at, entries }`, where `loss_recognized` is `cost` or zero on a written-off line and
  `entries` are `CondonationEntry { draw_index, entry_count, principal_condoned, principal_balance
  }`; `NavUpdated` when `loss_recognized > 0`.

A collection settlement is a quita of part of the principal and the debtor's payment of the rest
(`tests/quita.rs::a_partial_quita_then_the_debtor_pays_the_rest_and_the_line_settles`); a total
quita closes the line by itself. There is no release of títulos with a balance: the quita brings the
balance to zero first.

#### `renounce_availability()` — borrower

- **Accounts:** `borrower` (signer), `line` (`has_one = borrower`, `Unauthorized`). **Checks:**
  `Active` (`LineStatus`); the window is open and something is left to draw
  (`now < availability_end && available() > 0`, else `WindowClosed` — a suspended line's window is
  already closed).
- **Writes:** `availability_end = now`, `availability_end_saved = 0`. Irrevocable: `resume_draws`
  needs a saved end and `suspend_draws` an open window, so neither applies afterwards. The line
  can draw nothing more and the `repay` that leaves it owing nothing closes it (decision 45).
  No funds move; the pause does not block it.
- **Events:** `AvailabilityRenounced { borrower, available_forfeited, renounced_at }`.

#### `propose_acceleration(valid_until)` — **V C**

- **Accounts:** `CouncilLine`. **Checks:** `valid_until`; `council::check`; `Active` and `drawn >
  0` (`LineStatus`; a never-drawn line is cleared with `cancel_line`); none pending
  (`AccelerationState`).
- **Writes:** `acceleration_due_at = now + acceleration_notice_seconds`, a deadline the proposal
  keeps whatever the pool's notice becomes; an open draw window is suspended as in `suspend_draws`
  (a closed one is left alone).
- **Events:** `AccelerationProposed { due_at }`.

#### `withdraw_acceleration(valid_until)` — **V C**

- **Accounts:** `CouncilLine`. **Checks:** `valid_until`; `council::check`; `Active` with a pending
  proposal (`AccelerationState`).
- **Writes:** `acceleration_due_at = 0`. Draws stay suspended until `resume_draws`.
- **Events:** `AccelerationWithdrawn`.

#### `accelerate_line(valid_until)` — **V C**

- **Accounts:** `CouncilLine`. **Checks:** `valid_until`; `council::check`; `Active` with a pending
  proposal (`AccelerationState`); `now >= acceleration_due_at` (`AccelerationNotDue`).
- **Writes:** `line.accrue(now)`, then `status = InCollection`, `accelerated_at = now`, as the
  mark does: the accrual stops at `now` (decision 48); `pool.impaired_principal += usdc_drawn −
  usdc_cost_repaid` (decision 40). `acceleration_due_at` stays set: with `accelerated_at` it
  records the council's route.
- **Events:** `LineAccelerated { remaining = owed(now), impaired_principal }`.

### 4.4 Pagarés

Every instruction here takes `line` with its seeds and `has_one = pool`, and `pagare` at
`["pagare", line, draw_index]` with its stored bump and `has_one = line` (`WrongAccount`). Each
appends one registry entry (§9.3) and emits its `PagareEntry`. None is blocked by the pause. Only
`deliver_pagare` and `burn_pagare` touch the Core asset; they take `asset` (= `pagare.asset`),
`collection` (= `pool.collection`), `mpl_core_program` (= Core's id) and `system_program`.

#### `deliver_pagare(draw_index: u16)`

- **Accounts:** `payer` (signer, mut: anyone; pays what the CPIs may need); `pool`; `line`;
  `pagare` (mut); `asset` (mut); `collection` (mut); `recipient` (does not sign);
  `mpl_core_program`; `system_program`.
- **Checks, in this order:** `status == PAGARE_PAID` (`PagareNotCovered`); not delivered yet
  (`PagareAlreadyDelivered`); the line owes nothing, `owed(now) = (drawn − principal_repaid) +
  interest_unpaid(now) == 0` (`BalanceOutstanding`, decision 49); `recipient` is the line's sponsor
  when `PAID_BY_AVAL` is set (the aval settled it, LGTOC 115), else the line's borrower
  (`WrongAccount`): never whoever sends or pays for the instruction. The pagaré promises interest
  until maturity and interest is owed by the line, not by one pagaré, so a pagaré whose principal
  is covered while the line still owes is not paid in full and the holder keeps it (LGTOC 17 and
  130). The balance test holds in every line status: an `Active` line that can still draw and
  owes nothing delivers; a line in collection delivers once a payment or a quita settles it; a
  written-off line still owes, and delivers once recoveries and quitas leave it owing nothing
  (decision 51), while one never paid keeps its títulos with the pool. A título the quita
  extinguished (`CONDONED`) goes to the borrower. No council and no
  pause check: the delivery is the debtor's right, not a decision.
- **CPIs, in this order and no other** (the pool PDA signs, as the collection's update authority
  and the asset's owner): Core `UpdatePluginV1` sets `PermanentFreezeDelegate { frozen: false }`;
  `ApprovePluginAuthorityV1` moves that plugin's authority to `None`, so nobody can ever freeze the
  título again; `TransferV1` to `recipient`. The renunciation never runs on a frozen asset: a
  frozen asset whose freeze authority is `None` can never be thawed or moved
  (`tests/core_capabilities.rs::only_the_permanent_freeze_delegate_thaws_and_a_frozen_asset_with_no_delegate_is_stuck`),
  and it runs while the pool still owns the asset.
- **Writes, after the CPIs:** the `Entrega` entry (pool → recipient, over the record's
  `receipt_hash`: the line owes nothing, so there is no interest left to reserve, decision 49);
  `flags |= DELIVERED`.
- **Events:** `PagareEntry`, `PagareDelivered { recipient, payer, receipt_hash, entry_count,
  delivered_at }`.

#### `burn_pagare(draw_index: u16, document_hash: [u8; 32])`

- **Accounts:** `owner` (signer, mut, receives the asset's rent); `pool`; `line`; `pagare` (mut);
  `asset` (mut); `collection` (mut); `mpl_core_program`; `system_program`.
- **Checks:** `PAGARE_PAID` and `DELIVERED` (`PagareNotCovered`); the signer is the asset's owner
  as read from the Core account (`BaseAssetV1.owner`; `NotHolder`, or `WrongAccount` if the account
  does not decode): a delegate that Core would also accept cannot write the owner's entry.
- **Writes:** the `Terminacion` entry (owner → owner, over `document_hash`; zero is allowed, the
  burn being the owner's unilateral act). Optional: the record stays whether or not the asset is
  burnt.
- **Events:** `PagareEntry`, `PagareBurned { owner, document_hash }`, both before the burn.
- **CPI, last:** Core `BurnV1` with the owner as authority and payer, so a Core refusal leaves
  nothing half done and the rent returns to the owner.

#### `annul_pagare(draw_index: u16, document_hash: [u8; 32], valid_until)` — **V C**

- **Accounts:** `holder` (`== pool.council` → `NotHolder`); `multisig`; `pool`; `line`; `pagare`
  (mut).
- **Checks:** `valid_until`; `council::check`; non-zero `document_hash`, the annulment document's
  own hash, never a receipt (`DocumentHashRequired`); `status == PAGARE_OUTSTANDING`
  (`PagareAlreadyPaid`: a paid título is the debtor's).
- **Writes:** the `Terminacion` entry (pool → pool, over `document_hash`). Nothing burns, no field
  moves and the debt follows the line as before: a título issued in error is annotated, never
  destroyed (decision 38). No flag records the annulment; the entry is its only trace.
- **Events:** `PagareEntry`, `PagareAnnulled { document_hash }`.

#### `endorse_pagare(draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32], valid_until)` — **V C**

- **Accounts:** `holder` (`== pool.council` → `NotHolder`); `multisig`; `pool`; `line`; `pagare`
  (mut). No Core account and no payer.
- **Checks, in order:** `valid_until`; `kind == KIND_PROCURACION` (`EndosoKindInvalid`), the only
  endoso the pool makes; `council::check`; `to` is not the zero key, the pool, the council, the
  pool's creator, the FIX feeder, the line's borrower, rent payer or sponsor, or a member of the
  multisig (`EndorseeInvalid`); non-zero `endoso_hash` (`DocumentHashRequired`); line not
  `WrittenOff` (`LineStatus`); `status == PAGARE_OUTSTANDING` (`PagareAlreadyPaid`); no procuración
  in force (`ProcuracionState`).
- **Writes:** the `Procuracion` entry (pool → `to`, over `endoso_hash`); `flags |= PROCURACION`;
  `procurador = to`. The pool stays the owner and lends no power over the asset (decision 38).
- **Events:** `PagareEntry`.

#### `revoke_procuracion(draw_index: u16, entry_hash: [u8; 32], valid_until)` — **V C**

- **Accounts:** as `annul_pagare`.
- **Checks:** `valid_until`; `council::check`; non-zero `entry_hash`, the signed revocation's hash
  (`DocumentHashRequired`); a procuración in force (`ProcuracionState`).
- **Writes:** the `Revocacion` entry (pool → the recorded procurador); `PROCURACION` and
  `procurador` cleared. There is no power over the asset to take back.
- **Events:** `PagareEntry`.

#### `court_annotation(draw_index: u16, kind: u8, document_hash: [u8; 32], valid_until)` — **V C**

- **Accounts:** as `annul_pagare`.
- **Checks:** `valid_until`; `council::check`; `kind` is `KIND_ANOTACION_JUDICIAL` (6) or
  `KIND_CONSTANCIA_JUDICIAL` (7) (`AnnotationKindInvalid`); non-zero `document_hash`
  (`DocumentHashRequired`); the título not yet delivered (`PagareAlreadyPaid` once `PAID` and
  `DELIVERED`: from then on it is the owner's). Any line status, beside a procuración, and on a
  paid título whose delivery is pending.
- **Writes:** the entry (pool → pool, over `document_hash`) and nothing else. It gates no
  instruction: the judicial stage of a collection runs off chain (decision 38-g).
- **Events:** `PagareEntry`.

#### `annotate_payment(draw_index: u16, entry_count: u32, receipt_hash: [u8; 32], valid_until)` — **V C**

- **Accounts:** `annotator` (`== pool.council` → `Unauthorized`); `multisig`; `pool`; `line`;
  `pagare` (mut).
- **Checks:** `valid_until`; `council::check`; non-zero `receipt_hash` (`DocumentHashRequired`);
  `1 <= entry_count <= pagare.entry_count` (`EntryNotFound`). The chain keeps no per-entry kinds,
  so the program does not check that the referenced entry is a payment entry; the agreements engine
  does. Any status, a delivered or burnt título included: a receipt may arrive late.
- **Writes:** the `Recibo` entry (annotator → pool, over `receipt_hash`). No field is rewritten,
  `Pagare.receipt_hash` included: the correction is the entry (LGTOC 41).
- **Events:** `PagareEntry`, `PagareReceiptAnnotated { entry_count, payment_entry_count,
  receipt_hash }`.

## 5. State machines

### 5.1 The line

```
                 request_line                approve_line (C)              draw
   (none) ──────────────────► Requested ─────────────────────► Approved ──────────► Active
                                 │  reject_line (C)               │                   │ │ │
                                 │  withdraw_request              │ cancel_line (C)   │ │ │
                                 ▼  (account closed)              ▼ (drawn == 0)      │ │ │
                              (none)                          Cancelled               │ │ │
                                                                                      │ │ │
  repay / condone: nothing due and nothing drawable ◄─────────────────────────────────┘ │ │
                      │                                                                 │ │
                      ▼          write_off (C), now > term_end ◄────────────────────────┘ │
                   Repaid                     │                                           │
                      ▲                       ▼                                           │
                      │                  WrittenOff ◄── write_off (C) ──┐                 │
                      │                                                 │                 │
                      └──────── repay / condone (last unit) ───── InCollection ◄──────────┘
                                                                        mark_in_collection (anyone,
                                                                        scheduled) or
                                                                        accelerate_line (C)
                                                                        both add the line's cost
                                                                        to impaired_principal
```

- `Repaid` and `Cancelled` are terminal; `reject_line` and `withdraw_request` close the account.
  `WrittenOff` is not (decision 51): the line still owes, takes recoveries (`repay`) and quitas
  (`condone`), and the one that leaves it owing nothing moves it to `Repaid`
  (`WrittenOff → Repaid`, not drawn above); a written-off line never paid stays `WrittenOff`. A `Requested` line past its 15-day window stays `Requested` until withdrawn or
  rejected; `approve_line` refuses it.
- `Active → Repaid` needs nothing due and nothing drawable (window closed, fully drawn, or in
  collection). While the line can draw, a payment of all that is payable leaves it `Active`.
- After the mark there is no return to `Active` (no cure): only full repayment, a quita that
  leaves nothing owed, or the write-off.
  The pool's exits from delinquency are selling the portfolio (the endoso en propiedad, registry
  kind 1, reserved for a future feature) or the write-off, at most at 3–6–12 months of arrears
  as the council sets them; the program enforces only the collection period from the mark, with its
  30-day floor (decision 38-g).

**Draw window** (a sub-state of `Approved` and `Active`, fields `availability_end`,
`availability_end_saved`, `acceleration_due_at`):

```
   open (now < availability_end, saved == 0)
     │  suspend_draws (C)  or  propose_acceleration (C, Active)
     ▼
   suspended (availability_end = t, saved = original end)
     │  resume_draws (C), only with no acceleration pending  ──► open (end restored)
     │
   closed by date (now >= availability_end, saved == 0): suspend_draws refuses (WindowClosed)

   acceleration:  none ──propose_acceleration──► pending (due_at) ──accelerate_line, now >= due_at──► InCollection
                                                   │  withdraw_acceleration ──► none (still suspended)
                                                   └  mark_in_collection ─────► none, InCollection
```

### 5.2 The pagaré

```
                draw                         repay covers it                         deliver_pagare (anyone)
   (none) ──────────────► OUTSTANDING (0) ─────────────────────────────► PAID (1) ──────────────────────────► PAID + DELIVERED
                              │  ▲          Pagado, or PagadoAval when      │                                  asset owned by the
       endorse_pagare (C)     │  │          the aval paid (PAID_BY_AVAL)    │ court_annotation (C)             borrower or the aval
                              ▼  │ revoke_procuracion (C)                  │ annotate_payment (C)                 │
                     OUTSTANDING + PROCURACION                             │                                      │ burn_pagare (owner,
                              │                                            │                                      ▼ optional)
                              └── repay covers it: Terminacion ends the procuración                    asset burnt, record stays

   Abono: a repay that reaches an OUTSTANDING pagaré without covering it, or settles the accessories
   on the oldest one it touches. Status unchanged.
   condone (C) with principal (decision 51): a Condonacion entry on each pagaré it reaches; one it
   extinguishes becomes PAID + CONDONED (receipt_hash = the record's hash, no PAID_BY_AVAL), any
   procuración ends with a Terminacion, and it is delivered to the borrower like a paid one.
   annul_pagare (C): OUTSTANDING only; a Terminacion entry, nothing else changes.
   court_annotation: any state before DELIVERED. annotate_payment: any state.
   deliver_pagare: PAID, and only once the line owes nothing (decision 49).
```

The status never reverts and the record is never closed. `deliver_pagare` requires `PAID` and a
line that owes nothing, whatever its status (decision 49): every covered pagaré of a line waits for
the payment or quita that settles it, in collection as well; those of a written-off line wait for
the recoveries and quitas that leave it owing nothing (decision 51), and stay with the pool while
it owes.

## 6. The FIX (`fx.rs`, `post_fix`)

The FIX is Banxico's USD/MXN rate, stored as `Pool.fix_rate` = MXN per USD × 10⁴ (18.3740 is
183 740) and `Pool.fix_date` = the date Banxico determined it, `yyyymmdd`. Dates are Mexico City
dates, UTC−6 all year (`fx::mexico_city_day`); `fx::fix_day` refuses a date that does not exist or
falls outside 2000–2199.

### 6.1 `post_fix(fix_rate, fix_date, valid_until)`

- **Accounts:** `poster` (signer, the feeder or the council); `multisig` (= `pool.multisig`); `pool`
  (mut); `fx_reference` (`address = fx::REFERENCE_ACCOUNT`, else `FxReferenceInvalid`).
- **Checks for both, in order:** `valid_until` (the feeder's publication carries one too);
  `fix_rate > 0` (`WrongAmount`); `fix_date` a real date (`FixDateInvalid`) and not after today
  in Mexico City (`FixDateInvalid`).
- **The council** (`poster == pool.council`): `council::check`; `fix_date >= pool.fix_date`
  (`FixDateInvalid`: same-day corrections allowed, the date never moves back); `fix_date` at most
  `MAX_FIX_AGE_DAYS` (5) days before today (`FixDateInvalid`). No band and no reference check. The
  council publishes the first FIX, larger moves and corrections.
- **The feeder** (any other poster):
  1. `poster == pool.fx_feeder`, non-zero (`Unauthorized`);
  2. once per Mexico City day: today must be after the day of `pool.fix_posted_at`, whoever
     published then, the council included (`FixAlreadyPostedToday`);
  3. `fix_date > pool.fix_date` (`FixDateInvalid`);
  4. a FIX already in force (`FixJump`: the first FIX is the council's);
  5. **the band** (`fx::within_band`): `|new − old| × 10 000 <= old × 500 × days`, with `days` the
     calendar days between the two FIX dates clamped to `1..=3`: 5% a day either way, 15% at most
     however long the FIX in force has stood (`FixJump`);
  6. **the reference** (`fx::reference_rate`): RedStone's MXNe `PriceData`, owned by
     `fx::REFERENCE_PROGRAM` and read at fixed offsets (discriminator, feed id `"MXNe"`, the value
     as a big-endian u256 of USD per MXNe × 10⁸, the timestamp in milliseconds, decimals 8), as
     `10¹² / value` in FIX units; an unreadable account is `FxReferenceInvalid`. It must be at most
     `MAX_REFERENCE_AGE_SECONDS` (26 h) old (`FxReferenceStale`) and the FIX within
     `MAX_FIX_REFERENCE_GAP_BPS` (2%) of it (`FixOffReference`).
- **Both:** `min_fix_rate <= fix_rate <= max_fix_rate` unless both bounds are zero
  (`FixOutOfBounds`). Bounds with `min == max != 0` pin the FIX: exactly one rate is postable, and
  a pinned FIX is exempt from the staleness check in `approve_line` and `draw` (decision 42) — a
  peso series at parity needs no re-post cadence. Unpinning via `set_fix_bounds` restores it.
- **Writes:** `fix_rate`, `fix_date`, `fix_posted_at = now`.
- **Events:** `FixPosted { fix_rate, fix_date, previous_rate, by, slot, posted_at, reference_rate,
  reference_at }`; the reference is recorded for both posters, zero when it was missing or, for the
  council, unreadable.

The reference never sets the FIX; it only decides whether the feeder may publish without the
council. **On a mainnet build the check is mandatory.** RedStone's account exists on mainnet only,
so a `localnet` or `devnet` build skips it when the account passed at that address is empty and
owned by the system program; an account that exists is checked as on mainnet. A build without a
cluster feature always requires it.

Once a day at most, the feeder publishes any one date later than the FIX in force and not after
today; the dates it skips stay unpublished. A feeder that fell behind may publish today's date,
within the band of the days elapsed (at most three).

### 6.2 Where the FIX is used

| instruction | rule |
|---|---|
| `approve_line` | a FIX is published and at most 5 days old (`FixStale`) — age unchecked when the bounds pin the FIX (`fix_pinned`, decision 42); the cap `max_line_principal(nav)` (USDC) is converted to MXN with `mxn_for_usdc` at it |
| `draw` | the borrower signs `fix_rate` and `fix_date`; both equal the pool's (`FixMismatch`); `mexico_city_day(now) − fix_day(fix_date) <= 5` (`FixStale`), unchecked when the bounds pin the FIX (`fix_pinned`, decision 42) |
| `repay` | the payer signs `fix_rate` and `fix_date`; both equal the pool's (`FixMismatch`); no age limit: a payment is never refused for an old FIX |

A FIX published between the moment a client builds a draw or a repayment and the moment it lands
fails it with `FixMismatch`; the client re-reads and re-signs.

### 6.3 Conversions and rounding

With `MXN_USDC = 10⁸` (centavos per peso × USDC base units per dollar ÷ `FIX_SCALE`):

| function | formula | rounding | used for |
|---|---|---|---|
| `usdc_for_mxn(mxn, fix)` | `mxn × 10⁸ / fix` | down | USDC a draw delivers; the USDC value of `interest_collected` |
| `mxn_for_usdc(usdc, fix)` | `usdc × fix / 10⁸` | down | MXN credited for a repayment's `max_usdc`; the line cap at approval |
| `usdc_ceil_for_mxn(mxn, fix)` | `⌈mxn × 10⁸ / fix⌉` | up | USDC a repayment takes for the MXN applied, and the USDC that paid its principal |
| `cost_of(delivered, amount_mxn, applied)` | `delivered × min(applied, amount_mxn) / amount_mxn` | down | USDC cost of the principal applied to one pagaré; the whole pagaré costs exactly `usdc_delivered` |

The pool delivers rounded down, takes rounded up and credits MXN rounded down, so a repayment
never credits more MXN than the USDC it received buys at the FIX. The cost of a repayment's
principal is the difference of two `cost_of` values per record, so partial payments of one pagaré
add up to its delivery exactly. `FxRealized.delta = usdc_ceil_for_mxn(principal_part) − cost`; the
rounding of the charges never reaches `fx_realized`, and a payment without principal realizes
nothing.

The investors carry the USD/MXN risk: the NAV is at historical cost, so a weaker peso is realized
as a loss when the borrower pays (`FxRealized`), not before.

## 7. Calendar, arrears, ordinary interest and acceleration (`math.rs`, `state.rs`, `line_arrears`)

A scheduled line stores its terms and its repayments; arrears are recomputed on every call by
`math::arrears(terms, principal_repaid, now)` through `instructions/lines.rs::line_arrears`. Nothing about
arrears is stored. There is no grace period on late payment (decision 36): late interest runs
from the first second past a due date.

### 7.1 Schedule

- **Anchor** = `first_due_at − interval`. Installment `k` (1-based) is due at `anchor + k ×
  interval` and counts as due at that very second. `installments_due(now)` = `min(installments,
  (now − anchor) / interval)`, zero before the first due date.
- **The schedule is on principal** (decision 48): the installments add up to `drawn` (MXN), and
  ordinary interest has no calendar of its own — every payment takes the interest accrued first
  (§7.5).
- **Installment** = `drawn / installments` rounded down; the last carries the remainder
  (`installment_amount`, `cumulative_due`); `draw` keeps `drawn >= installments`.
- **Covered** = `principal_repaid / installment`, capped at `installments − 1` unless
  `principal_repaid >= drawn` (interest and late interest never pay an installment; one centavo
  of principal short is a miss).
- **Missed** = `due − covered` when positive, zero when paid ahead.

### 7.2 Late interest (moratorio)

Late interest accrues at `late_bps` a year (365 days) on each overdue part of **principal**
`p_k` (no interest on interest), from `start_k = max(due_k, late_paid_until)` to `now`; before
the last due date overdue principal also keeps accruing ordinary interest (decision 48):

```
late_due = Σ_k p_k × min(now − start_k, cap) × late_bps / (10 000 × SECONDS_PER_YEAR)
cap      = ⌈10 000 × SECONDS_PER_YEAR / late_bps⌉
```

The weighted sum is accumulated in `u128` and divided once. Caps: per settlement `late_due <=
overdue`; over the life of the line `late_due <= drawn − late_collected`. `repay`
collects it first, refuses a payment that does not cover it, and moves `late_paid_until` only when
`late_part > 0`.

### 7.3 Bullet lines

A bullet line (`installments == 0`) has one maturity, `term_end`: one installment of all the
principal; ordinary interest accrues on the principal outstanding until `term_end`. `line_arrears` treats it as one installment due at `term_end` (anchor
`term_end − 1`, interval 1), so late interest at `late_bps` runs on its unpaid principal from
`term_end`, or from `accelerated_at` when the council accelerated it earlier.
`mark_in_collection` refuses a bullet line (`NotScheduled`); the council accelerates one with
`propose_acceleration` and `accelerate_line`, and `write_off` takes an `Active` bullet line after
`term_end`.

### 7.4 Acceleration

Two routes set `accelerated_at`, move the line to `InCollection` and provision its cost (§8):

| route | instruction | trigger |
|---|---|---|
| objective | `mark_in_collection` (anyone) | scheduled; `missed >= min(max_missed, installments − covered)` (decision 41) and `now > anchor + min(covered + max_missed, installments) × interval` |
| council | `propose_acceleration` → notice → `accelerate_line` | `Active`, `drawn > 0`; `now >= acceleration_due_at` |

Both run `lines.rs::accelerate`: `accrue(now)`, then `accelerated_at = now`. Nothing is cut
(decision 48): the ordinary interest accrued to the mark stays owed, and none accrues after it.

After the mark, `arrears` counts every installment as due, so `overdue` is the whole unpaid
principal. Installment `k` is exigible at `min(due_k, accelerated_at)` and accrues late interest from
`max(min(due_k, accelerated_at), late_paid_until)`: one already overdue keeps its own start, one
not yet due starts at the mark. `first_missed_due` follows the exigible instant. The loop runs over
every remaining installment, at most 120.

### 7.5 Ordinary interest (decision 48)

Ordinary interest accrues per second, ACTUAL/365, at the line's `rate_bps`, on the principal
outstanding `drawn − principal_repaid`, until `accrual_end = min(last_due, accelerated_at)`
(`last_due` is `term_end` on a bullet line); after it only late interest runs. The line keeps the
exact time-integral of its balance, in centavo-seconds, so no number of payments can round
interest away:

```
t                 = min(now, accrual_end)
principal_seconds(now) = principal_seconds + (drawn − principal_repaid) × max(0, t − accrued_at)
interest_accrued(now)  = ⌊rate_bps × principal_seconds(now) / (10 000 × SECONDS_PER_YEAR)⌋
interest_unpaid(now)   = interest_accrued(now) − interest_repaid
owed(now)              = (drawn − principal_repaid) + interest_unpaid(now)
```

`CreditLine::accrue(now)` stores `principal_seconds(now)` and moves `accrued_at` to `t`; it runs
before every change of the outstanding principal (`draw`, a `repay` that applies principal) and
before `accelerated_at` is set (`mark_in_collection`, `accelerate_line`), and leaves
`interest_accrued(now)` unchanged. Rounded once at read time, a line paid a centavo a second
accrues exactly what an untouched one accrues (`tests/schedule.rs`); `tests/reference/mod.rs` is
an independent piecewise simulation every interest figure of the suite is checked against.

The payment order is `math::allocate(credit, late, interest_unpaid, principal_left)`: late
interest (the credit must cover it), then the interest accrued and unpaid, then principal, never
more than `principal_left + interest_unpaid + late`. Interest first means a payment that applies
principal leaves `interest_unpaid(now) == 0`. Every handler reads the same `CreditLine` methods:
`interest_accrued`, `interest_unpaid`, `owed`, `can_draw`.

Bounds: `principal_seconds` fits a `u128` for any `u64` balance over any `i64` span; the interest
read must fit a `u64` (`Overflow` otherwise), which holds while `drawn × rate_bps / 10 000 ×
years to accrual_end < 1.8 × 10¹⁹` centavos — for 100% a year over 100 years, any line below
1.8 × 10¹⁷ centavos.

### 7.6 What reads the arrears

| reader | fields |
|---|---|
| `repay` | `late_due` |
| `mark_in_collection` | `covered`, `due`, `missed` |
| `condone` | `late_due` |
| `write_off` | `first_missed_due` |

`clients/js/src/schedule.ts` mirrors the arithmetic (`lineArrears`, `lineLateDue`,
`interestAccrued`, `interestUnpaid`, `lineOwed`, `maxLinePrincipal`); `tests/vectors.rs` and
`clients/js/test/schedule.test.ts` replay the same 427 vectors (48 of them accelerated) from
`clients/js/test/arrears-vectors.json` against both.

## 8. Provision and the dual price (decisions 40, 40b)

`Pool.impaired_principal` is the outstanding USDC cost of the lines in collection. It moves in these
instructions and nowhere else:

| instruction | `impaired_principal` |
|---|---|
| `mark_in_collection`, `accelerate_line` | `+ usdc_drawn − usdc_cost_repaid` of the line, at the moment it enters collection |
| `repay` on an `InCollection` line | `− cost` of the principal recovered |
| `write_off` of an `InCollection` line | `− loss_usdc`: what the provision anticipated becomes the loss |
| `condone` with principal on an `InCollection` line | `− cost` of the principal condoned: that part of the provision becomes the loss (decision 51) |

`Pool::nav` subtracts `⌊impaired_principal × provision_bps / 10 000⌋`; `provision_bps` starts at
10 000 at `init_pool` and only the council moves it (`set_provision_bps`, `0..=10 000`). At 100%
the mark takes the line's whole remaining cost out of the redemption NAV, a recovery puts back
exactly what it recovered, and the write-off and a principal quita move nothing more
(`tests/provision.rs`, `tests/quita.rs`). Below 100% the quita lowers the redemption NAV by the
unprovisioned part of its cost, as a write-off of that cost would.

Which NAV each reader uses:

| reader | NAV |
|---|---|
| `deposit` | `entry_nav`: no provision, the impaired book at par |
| `redeem` | `nav`: provisioned |
| `approve_line`, `draw` (the line cap) | `nav`: provisioned |
| `Pool::check`, every `nav_after`, `NavUpdated.total_assets` | `nav`: provisioned |

**Why two prices.** A provisioned NAV alone would let anyone deposit at the depressed price just
before a recovery and redeem after it, taking the released provision from the holders who carried
the loss; with flash-borrowed capital that needs no patience at all. The first answer tried, refusing
deposits while anything is impaired, would close a pool that always has some portfolio in
collection. Pricing deposits at the unprovisioned NAV is Maple v2's dual exchange rate, adopted for
Maple's own reason: whoever enters while a line is impaired buys that book at par, so the recovery
hands the entrant nothing and the sandwich nets zero or less, while deposits are never gated. What
an entrant does share pro rata is ordinary income (interest, late interest), like any holder. The
residuals that stay are redeeming before a foreseeable mark (inherent to objective marking) and the
cash-basis interest jumps (`tests/known_limits.rs`, SECURITY.md). A front end shows both prices.

## 9. The pagaré (decisions 38, 38-g)

The electronic pagaré is the signed data message issued through the sistema de información (the
program and the agreements engine); the Core asset is its record of issuance, holding and
circulation, and the `Pagare` account its registry. One pagaré per draw, promising the tranche in
MXN.

### 9.1 The Metaplex Core asset

`draw` creates the asset with `CreateV2` (`issue_pagare`):

- **owner** the pool PDA; **update authority** the series' collection (whose own update authority
  is the pool PDA); **name** `PAGARE <line_id>-<draw_index>`; **URI** empty; no series label and
  nothing personal.
- **plugins at creation:**
  - `PermanentFreezeDelegate { frozen: true }`, authority `UpdateAuthority` (the collection's, so
    the pool PDA): the asset cannot move, and only the program can thaw it, which `deliver_pagare`
    does right before renouncing the freeze and transferring the título;
  - `ImmutableMetadata`, authority `None`;
  - `Attributes`, authority `None`, eight facts: `pagare_hash` (lower-case hex), `amount_mxn`,
    `currency` = `MXN`, `issued_at`, `due_at`, `line` (the line id), `draw_index`,
    `signature_method` (decimal text). The certificate commitment stays in the record only;
  - `AddBlocker`, authority `None`: no authority-managed plugin can be added later.
- **no transfer or burn delegate:** the pool owns the asset, only the program signs for it, and a
  procuración lends no power over it. `init_pool` admits only a bare collection, so no
  collection-level plugin reaches the asset. No AppData: the registry lives in the `Pagare` record.

The program reads the asset back only in `burn_pagare` (its owner); otherwise later instructions
take `address = pagare.asset` and rely on Core's own checks in the CPIs.
`tests/core_capabilities.rs` pins, against the pinned Core 0.15.3 binary, what Core allows after
creation: permanent-delegate hand-off and `None` as terminal, that only the freeze delegate thaws
and a frozen asset with no delegate is stuck, that an asset's update authority cannot leave its
collection, `AddBlocker`, and owner-managed delegates resetting on transfer.

### 9.2 The record

The `Pagare` account (§3.3) is created by hand in `draw` (`create_sidecar`) so its bump is the
client's and the address is verified with one `create_program_address`. It records the promise in
MXN (`amount_mxn`, `pagare_hash`) apart from the delivery in USDC (`fix_rate`, `fix_date`,
`usdc_delivered`, `delivery_hash`). `drawn_through` is the line's `drawn` after the tranche:
pagarés are covered oldest first, so pagaré `k` is covered once `principal_repaid >=
drawn_through_k`. Interest is owed by the line, not by a tranche, and never covers a pagaré.

### 9.3 The registry: a hash chain in the record (`registry.rs`)

```
entry_hash = sha256(asset ‖ kind ‖ from ‖ to ‖ hash ‖ stated_at ‖ entry_count)
             32 B     1 B    32 B   32 B  32 B   i64 LE      u32 LE         (141 bytes)
log_root   = sha256(log_root ‖ entry_hash)            starting from 32 zero bytes
```

`registry::append` is the only writer: it increments `entry_count` (the entry's own position, 1 for
the issuance; `Overflow` if it would wrap), updates `log_root` and `last_kind`. The full entry goes
in `PagareEntry { pool, line_id, draw_index, asset, kind, from, to, entry_hash, entry_count,
recorded_at, stated_at }`, so anyone can replay the chain from the events and compare it with
`log_root`; nobody can rewrite or remove an entry. Every entry the program writes has `stated_at =
recorded_at = now`.

| kind | code | written by | from | to | hash |
|---|---|---|---|---|---|
| `Emision` | 0 | `draw` | pool | pool | `pagare_hash` |
| `Procuracion` | 2 | `endorse_pagare` | pool | procurador | `endoso_hash` |
| `Revocacion` | 3 | `revoke_procuracion` | pool | recorded procurador | `entry_hash` (the signed revocation) |
| `Terminacion` | 4 | `repay`, ending a procuración on the título it covers | pool | procurador | the payment's annotation hash |
| `Terminacion` | 4 | `condone`, ending a procuración on the título it extinguishes | pool | procurador | the `Condonacion`'s hash |
| `Terminacion` | 4 | `burn_pagare` | owner | owner | `document_hash` (zero allowed) |
| `Terminacion` | 4 | `annul_pagare` | pool | pool | `document_hash` (the annulment) |
| `Abono` | 5 | `repay` (applies to the pagaré without covering it) | payer | pool | annotation hash |
| `AnotacionJudicial` | 6 | `court_annotation` | pool | pool | `document_hash` |
| `ConstanciaJudicial` | 7 | `court_annotation` | pool | pool | `document_hash` |
| `Pagado` | 9 | `repay` (covers it) | payer | pool | annotation hash |
| `Recibo` | 11 | `annotate_payment` | council | pool | `receipt_hash` |
| `PagadoAval` | 12 | `repay` (covers it, paid by the aval) | aval | pool | annotation hash |
| `Entrega` | 13 | `deliver_pagare` | pool | borrower or aval | the record's `receipt_hash` |
| `Condonacion` | 14 | `condone` with principal (decision 51; reaches the pagaré, covering it or not) | pool | borrower | condonation hash |

Codes 1 and 8 are reserved for the endoso en propiedad and the late endoso; 10 was the holder's
cancellation request and is retired. None is reused.

**Payment entries** (decision 38; LGTOC 17 and 130: every abono states the amount and the
balance). Each `Abono`, `Pagado` or `PagadoAval` entry's hash is

```
annotation_hash = sha256("elote.pago.v2" ‖ receipt_hash ‖ principal_applied u64 ‖
                         accessory_applied u64 ‖ principal_balance u64 ‖ by_borrower u8)
```

integers little endian, built by `registry::annotation_hash`. For each record `repay` takes,
oldest first:

- `principal_applied` = the part of this payment's principal that falls in the pagaré's range
  `(drawn_through − amount_mxn, drawn_through]`;
- `accessory_applied` = the ordinary and late interest the payment settled, annotated **once, on
  the oldest pagaré touched** (zero on the others), so a payment of interest or late interest alone
  also leaves its entry there;
- `principal_balance` = the pagaré's unpaid principal after the payment, zero when covered;
- `by_borrower` = the payer is the line's borrower;
- the kind: `PagadoAval` when the payment covers the pagaré and the payer is the line's sponsor
  with `SPONSOR_GUARANTOR` (and not the borrower); `Pagado` when it covers it; `Abono` otherwise;
- a record with nothing applied, in principal or accessories, gets no entry.

No entry reserves interest (decision 48, on counsel's advice): interest is taken first, so a
payment that applies principal leaves no ordinary interest accrued and unpaid, and a reservation
that would always read zero weakens the ones that matter. The invariant is pinned by
`tests/schedule.rs::after_any_payment_that_touches_principal_no_interest_is_left_unpaid` and the
state machine.

The preimages travel in `PagarePayment.entries` (`PaymentEntry { draw_index, kind, entry_count,
principal_applied, accessory_applied, principal_balance, by_borrower }`, where `entry_count` is the
payment entry's own position), so anyone can replay `annotation_hash → entry_hash → log_root`.
`receipt_hash` is the payer's, unverified and provisional; zero means "receipt to follow". The
holder's correction arrives as a `Recibo` entry (`annotate_payment`); no field is rewritten (LGTOC
41).

**Quita entries** (decision 51). A principal quita is never a payment: each record its principal
reaches gets a `Condonacion` entry, from the pool (the holder remits) to the borrower, over

```
condonation_hash = sha256("elote.quita.v1" ‖ document_hash ‖ principal_condoned u64 ‖
                          principal_balance u64)
```

built by `registry::condonation_hash`, where `principal_condoned` is the part of the quita's
principal in the pagaré's range and `principal_balance` its unpaid principal after; a record with
nothing condoned gets no entry. Accessories condoned are stated in `LineCondoned`, not on the
títulos. The preimages travel in `LineCondoned.entries` (`CondonationEntry { draw_index,
entry_count, principal_condoned, principal_balance }`) with its `document_hash`, so anyone can
replay `condonation_hash -> entry_hash -> log_root`.

### 9.4 Payment, delivery, burn and annulment

- **Covered** (`repay`): the record becomes `PAGARE_PAID` with `paid_at` and `receipt_hash`; a
  covering payment by the aval sets `PAID_BY_AVAL`. A procuración in force ends in the same
  instruction with a `Terminacion` entry from the pool to the procurador over the payment's
  annotation hash; the procurador's own collection is this repayment.
- **Delivered** (`deliver_pagare`, LGTOC 129–130): only once the line owes nothing, principal
  and interest (decision 49): a covered pagaré of a line that still owes is not paid in full, since
  it promises interest until maturity, and stays with the pool. Anyone sends it, in the settling
  repayment's own transaction (the client appends a delivery for every título not yet delivered,
  as many as fit, and sends the rest right after) or later. The pool thaws the asset, renounces the
  freeze to `None` and transfers it to the borrower, or to the aval who settled it (LGTOC 115: the
  aval collects from the suscriptor with the live título). The council is not in the path and
  neither the pause nor a court annotation gates it. `Entrega` entry over the record's
  `receipt_hash`, `DELIVERED` flag.
- **Burnt** (`burn_pagare`): the delivered título's owner, read from the Core account, optionally
  writes a `Terminacion` and burns the asset; the rent returns to the owner and the record stays.
- **Annulled** (`annul_pagare`): the council annotates an outstanding título issued in error with a
  `Terminacion` over the annulment document; nothing else changes.

There is no cancellation by the holder and no burn by the pool or a procurador: the extinction
route is the delivery to the debtor (decision 38).

### 9.5 Procuración

The endoso en procuración is the only endoso the pool makes. `endorse_pagare` appends the entry
and names the procurador of record in `Pagare.procurador` (offset 212, so a client can list a
procurador's pagarés with one `memcmp`); the pool stays the owner and no power over the asset is
lent. The procurador collects through `repay`, like any payer, and litigates with the registry's
entry as the endorsement. `revoke_procuracion` appends the revocation; the covering payment ends
the procuración by itself (§9.4). One procurador key per case: a shared key would let anyone list
every line in collection from it (`scripts/endorse-pagare.ts` makes a fresh one).

### 9.6 Annotations

- **Court** (`court_annotation`): an `AnotacionJudicial` (embargo, suspension) or
  `ConstanciaJudicial` over the court document's hash, on any line status, beside a procuración, and
  on a paid título until it is delivered. It is a record only: the judicial stage of a collection
  runs off chain, after the portfolio is sold or written off, and no court act gates any pool
  instruction (decision 38-g).
- **Receipt** (`annotate_payment`): a `Recibo` entry attached to entry `entry_count` of the chain,
  on any status, a delivered or burnt título included.

## 10. Compute

The figures below are the interface reference's, measured by
`programs/elote_pool/tests/compute.rs` (the program's instruction alone) and `tests/squads.rs` (the
whole Squads `vault_transaction_execute` transaction). The rule is 100 000 CU per transaction
(`CEILING`), half the 200 000 CU an instruction gets without a compute-budget request, Squads
included for a council instruction.

| case | CU |
|---|---|
| `init_pool`, classic asset and plain shares | 38 639 |
| `init_pool`, classic asset and permissioned shares | 38 173 |
| `request_line`, 120 installments | 21 952 (20 452 to 23 452 across runs: the PDA bump searches of random keys) |
| `approve_line` (with the multisig check) | 15 357 |
| `approve_line` through Squads | 41 411 |
| `post_fix` by the council | 12 760 |
| `post_fix` by the feeder | 11 588 |
| `post_fix` by the feeder, against the reference | 11 875 |
| `propose_council` through Squads | 34 972 |
| `draw`, first tranche, 120 installments | 82 260 |
| `draw`, second tranche, 120 installments | 82 422 |
| `draw`, tranche 10, 120 installments | 82 435 |
| `repay`, 18 installments, all missed | 44 603 |
| `repay`, 120 installments, all missed | 70 121 |
| `repay` after the mark, 120 installments | 59 725 |
| `repay` covering three records, 120 installments, all missed | 86 877 |
| `repay` recovering a written-off line over three records, 120 installments, all missed | 89 848 |
| `condone`, late interest only, 120 installments, all missed | 50 397 |
| `condone` over three records, one under procuración, 120 installments, all missed | 66 902 |
| `condone` through Squads, three records (one under procuración), 18 installments, all missed | 82 868 |
| `condone` through Squads of one pagaré (two records, one entry, under procuración), 120 installments, all missed | 94 913 (87 CU inside the margin) |
| `condone` through Squads writing two / three entries, 120 installments, all missed | 96 380 / 104 452 (108 326 with a procuración): **over the margin / over the rule** |
| `mark_in_collection`, 18 installments, all missed | 17 985 |
| `mark_in_collection`, 120 installments, all missed | 34 731 |
| `mark_in_collection`, 120 installments, 3 missed | 15 808 |
| `write_off`, 18 installments | 25 430 |
| `write_off`, 120 installments | 50 828 |
| `write_off` after the mark, 120 installments | 50 830 |
| `suspend_draws` | 14 577 |
| `resume_draws` | 13 892 |
| `propose_acceleration` | 14 587 |
| `accelerate_line`, 120 installments, all due | 16 879 |
| `deliver_pagare`, 120 installments | 68 209 |
| `deliver_pagare` after a procuración | 68 115 |
| `burn_pagare` | 30 568 |
| `court_annotation`, 120 installments | 17 370 |
| `annotate_payment`, 120 installments | 20 043 |

The rows of `request_line` through `annotate_payment` above were re-measured after decision 48
(ordinary interest on the outstanding principal) and the layout clean-up that followed it (one
`CreditLine` offset per field, no `grace_period_seconds`); `accelerate_line` no longer runs the
arrears loop. Earlier, re-measured on the `localnet` build after decisions 38 and 39 (no Core CPI in the
procuración, the pause by council vote): `endorse_pagare` 17 618 (same again after a revocation;
43 760 through Squads), `revoke_procuracion` 17 386, `pause` by the council 11 198,
`annul_pagare` through Squads 46 219. interface.md's table carries the full fresh set. The `repay`
rows and the three decision-51 rows were re-measured with decision 51 (the records check and the
cost moved into helpers `repay` and `condone` share). The recovery is a `repay` (no Squads) and
keeps the three-record payment's 5 000 CU margin
(`quita_and_recovery_over_three_sidecars_stay_under_the_ceiling`). The quita is a council
instruction and runs inside `vault_transaction_execute`, which adds about 25 000 to 41 000 CU: on
an 18-installment line (the pilot's longest) a total quita over three records fits with the margin,
but on a 120-installment line with every installment missed only a quita writing one entry does —
one pagaré per call, records `[k, k+1]` (`tests/squads.rs::the_council_condones_principal_through_squads`);
two entries exceed the margin and three the 100 000 CU rule (each still runs under the 200 000 CU an
instruction gets by default). Until that is decided, a client splits a quita on a long schedule one
pagaré per call: principal up to `drawn_through` of the pagaré at the cursor less
`principal_repaid`; `MAX_SIDECARS_PER_REPAY` is unchanged.

- **Budget rules in the tests.** Every instruction stays below `CEILING`. `draw` has its own
  `DRAW_CEILING` of 90 000, 10 000 below: of its cost, the Core `CreateV2` CPI with the four
  plugins is about 37 000 CU, fixed by the plugin set. The three-record `repay`
  (`a_payment_covering_three_sidecars_stays_under_the_ceiling`) must leave a 5 000 CU margin,
  so at most 95 000; `tests/squads.rs` requires the same margin for the council instructions
  through Squads. This is why `MAX_SIDECARS_PER_REPAY` is 3: three records keep the heaviest
  payment, every installment of a 120-installment line missed, under the ceiling with that
  margin, and the client splits a payment that would touch more into several `repay` calls (the
  amount is free).
- **Key-dependent figures.** `init_pool` runs Anchor's `init` bump searches for the pool, the vault
  and the dead-shares account, plus `council::canonical_vault`; each extra try costs about
  1 500 CU, so the figure varies with the keys. `init_pool_cost_follows_the_pda_bump_searches`
  shows the tries explain the whole spread and pins `init_pool` under 70 000.
- **Deterministic figures.** `draw` takes both PDA bumps from the client and searches nothing.
  `deliver_pagare` makes three Core CPIs (update the plugin, approve `None`, transfer).
- **The arrears loop** (`math::arrears`) is the only loop over the schedule, at most 120
  iterations; with every installment missed each iteration takes the accrual branch. `repay` also
  loops over the records it is given (each loaded and written).
- **Not driven by `tests/compute.rs`:** `deposit`, `redeem`, `reject_line`, `withdraw_request`,
  `withdraw_acceleration`, `cancel_line`, `resume`, the council setters, `accept_council`,
  `set_fx_feeder` and `annul_pagare`.

## 11. Events and errors

### 11.1 Events (42, `events.rs`, all through `emit_cpi!`)

| event | emitted by | fields |
|---|---|---|
| `PoolInitialized` | `init_pool` | pool, council, creator, multisig, asset_mint, share_mint, collection, collection_period_seconds, max_line_bps, max_outstanding_principal, acceleration_notice_seconds, asset_controls, permissioned, min_fix_rate, max_fix_rate |
| `Deposited` | `deposit` | pool, depositor, assets, shares, nav_after, supply_after |
| `Redeemed` | `redeem` | pool, holder, shares, assets, nav_after, supply_after |
| `NavUpdated` | `deposit`, `redeem`, `repay`, `write_off`, `condone` recognising a loss, last in each | pool, total_assets (the provisioned NAV), share_supply |
| `PoolPaused` | `pause` | pool, by (the council) |
| `PoolResumed` | `resume` | pool |
| `MaxLineBpsSet` | `set_max_line_bps` | pool, max_line_bps |
| `ProvisionSet` | `set_provision_bps` | pool, provision_bps |
| `OutstandingCapSet` | `set_outstanding_cap` | pool, max_outstanding_principal |
| `FixBoundsSet` | `set_fix_bounds` | pool, min_fix_rate, max_fix_rate |
| `AccelerationNoticeSet` | `set_acceleration_notice` | pool, acceleration_notice_seconds |
| `CollectionPeriodSet` | `set_collection_period` | pool, collection_period_seconds |
| `CouncilProposed` | `propose_council` | pool, council, kind |
| `CouncilAccepted` | `accept_council` | pool, old, new, kind |
| `FxFeederSet` | `set_fx_feeder` | pool, fx_feeder |
| `FixPosted` | `post_fix` | pool, fix_rate, fix_date, previous_rate, by, slot, posted_at, reference_rate, reference_at |
| `LineRequested` | `request_line` | pool, line_id, borrower, payer, sponsor, sponsor_role, agreement_hash, expediente_root, pagare_template_hash, terms_hash, currency, principal, rate_bps, term_end, first_due_at, interval_seconds, installments, late_bps, max_missed |
| `LineApproved` | `approve_line` | pool, line_id, expediente_root, terms_hash, availability_end |
| `LineRejected` | `reject_line` | pool, line_id |
| `RequestWithdrawn` | `withdraw_request` | pool, line_id |
| `LineCancelled` | `cancel_line` | pool, line_id |
| `LineDrawn` | `draw` | pool, line_id, draw_index, amount_mxn, fix_rate, fix_date, usdc_delivered, drawn, available, availability_end, outstanding_after |
| `PagareIssued` | `draw` | pool, line_id, draw_index, asset, pagare_hash, amount_mxn, delivery_hash, due_at, holder (the pool PDA) |
| `PagareEntry` | `draw`, `repay`, `condone`, `deliver_pagare`, `burn_pagare`, `annul_pagare`, `endorse_pagare`, `revoke_procuracion`, `court_annotation`, `annotate_payment` | pool, line_id, draw_index, asset, kind, from, to, entry_hash, entry_count, recorded_at, stated_at |
| `PagarePayment` | `repay` that took pagaré records | pool, line_id, payer, receipt_hash, late_part, interest_part, principal_part, entries: `Vec<PaymentEntry { draw_index, kind, entry_count, principal_applied, accessory_applied, principal_balance, by_borrower }>` |
| `LateInterestCollected` | `repay` with late interest | pool, line_id, amount |
| `FxRealized` | `repay` that applies principal or realizes a non-zero delta | pool, line_id, receipt_hash, cost, received, delta, nav_after |
| `LossRecovered` | `repay` on a `WrittenOff` line (decision 51) | pool, line_id, receipt_hash, late_part, interest_part, principal_part, cost_recovered (USDC cost of that principal, off `written_off`), received (the USDC that paid it), written_off_after, nav_after |
| `LineRepaid` | `repay` | pool, line_id, payer, receipt_hash, usdc_received, fix_rate, principal_part, interest_part, remaining (without late interest), nav_after |
| `LineInCollection` | `mark_in_collection` | pool, line_id, missed, remaining (principal and the interest accrued to the mark and unpaid), impaired_principal (the pool's whole impairment after the mark) |
| `LineWrittenOff` | `write_off` | pool, line_id, loss (MXN), loss_usdc, nav_after |
| `LineDrawsSuspended` | `suspend_draws` | pool, line_id, available_forfeited |
| `LineDrawsResumed` | `resume_draws` | pool, line_id, availability_end |
| `LineCondoned` | `condone` (decisions 50, 51) | pool, line_id, document_hash, late_interest, interest, principal (MXN), cost (USDC of that principal), loss_recognized (`cost`, or zero on a written-off line), remaining, nav_after, condoned_at, entries: `Vec<CondonationEntry { draw_index, entry_count, principal_condoned, principal_balance }>` |
| `AvailabilityRenounced` | `renounce_availability` | pool, line_id, borrower, available_forfeited, renounced_at |
| `AccelerationProposed` | `propose_acceleration` | pool, line_id, due_at |
| `AccelerationWithdrawn` | `withdraw_acceleration`, `mark_in_collection` (superseding a proposal) | pool, line_id |
| `LineAccelerated` | `accelerate_line` | pool, line_id, remaining, impaired_principal |
| `PagareDelivered` | `deliver_pagare` | pool, line_id, draw_index, asset, recipient (the borrower or the aval), payer (whoever sent it), receipt_hash, entry_count, delivered_at |
| `PagareBurned` | `burn_pagare` | pool, line_id, draw_index, asset, owner, document_hash |
| `PagareAnnulled` | `annul_pagare` | pool, line_id, draw_index, asset, document_hash |
| `PagareReceiptAnnotated` | `annotate_payment` | pool, line_id, draw_index, entry_count (the `Recibo`'s), payment_entry_count (the entry it annotates), receipt_hash |

Order within one instruction: `deposit` and `redeem` emit their event, then `NavUpdated`; `draw`
emits `LineDrawn`, `PagareIssued`, `PagareEntry`; `repay` emits the `PagareEntry`s (each payment
entry, then a `Terminacion` that follows it), `PagarePayment`, `LateInterestCollected`,
`FxRealized`, `LossRecovered` (on a written-off line), `LineRepaid`, `NavUpdated`; `condone` emits
the `PagareEntry`s (each `Condonacion`, then a `Terminacion` that follows it), `LineCondoned`, then
`NavUpdated` when it recognised a loss; `mark_in_collection` emits `LineInCollection`, then
`AccelerationWithdrawn` if it superseded a proposal; `deliver_pagare`, `burn_pagare` and
`annul_pagare` emit `PagareEntry` and then their own event (`burn_pagare` both before the burn);
`annotate_payment` emits `PagareEntry`, then `PagareReceiptAnnotated`; `write_off` emits
`LineWrittenOff`, then `NavUpdated`.

### 11.2 Errors (78, `errors.rs`, numbered from 6000 in declaration order)

Squads v4 also numbers its errors from 6000 (its `NotAMember` is 6005, which is `CouncilTooLarge`
here). Decode an error by the program that
raised it, read from the invoke stack in the logs, never by the number alone; `scripts/lib/cli.ts`
does this. Five codes are retired and kept so nothing renumbers: 6030, 6061, 6067 (the cancellation
route, decision 38), 6075 (the judicial hold, decision 38-g) and 6076 (the deposit gate, decision
40b). No instruction returns them.

| code | name | group | message |
|---|---|---|---|
| 6000 | `Unauthorized` | Signers | This signer may not do this |
| 6001 | `SponsorRequired` | Signers | The sponsor named on this line must sign |
| 6002 | `NotHolder` | Signers | The pagaré's holder must sign |
| 6003 | `Paused` | Signers | The pool is paused |
| 6004 | `CouncilInvalid` | The council | The council is not the vault of an autonomous Squads v4 multisig with a majority threshold |
| 6005 | `CouncilTooLarge` | The council | The council multisig has more members than the program checks |
| 6006 | `CouncilKindInvalid` | The council | This council kind is not accepted |
| 6007 | `NoPendingCouncil` | The council | No council rotation is pending, or this signer is not the one proposed |
| 6008 | `ProposalExpired` | The council | The council's decision is past its valid_until |
| 6009 | `WrongAccount` | Accounts | Wrong mint or token account |
| 6010 | `AssetMintNotAllowed` | Accounts | The asset mint carries a Token-2022 extension the pool does not accept |
| 6011 | `ShareMintNotAllowed` | Accounts | The share mint is not one this pool can own and price: wrong authority, supply, decimals or extensions |
| 6012 | `AssetNotListed` | Accounts | The asset mint is not on this cluster's allow-list |
| 6013 | `AssetIsShareMint` | Accounts | The asset mint and the share mint are the same |
| 6014 | `CollectionNotAllowed` | Accounts | The collection is not a bare Metaplex Core collection whose update authority is the pool |
| 6015 | `PagareAccountsMismatch` | Accounts | The pagaré records passed are not the ones this payment touches, oldest first |
| 6016 | `BorrowerIsCouncil` | Roles that must stay apart | The council cannot be the borrower |
| 6017 | `BorrowerIsCurator` | Roles that must stay apart | A member of the council cannot be the borrower |
| 6018 | `EndorseeInvalid` | Roles that must stay apart | The endorsee is missing, or is the pool, the council, a curator, the creator, the FIX feeder, or the line's borrower, payer or sponsor |
| 6019 | `WrongAmount` | Amounts and invariants | Amount is zero or exceeds what is due |
| 6020 | `Overflow` | Amounts and invariants | Arithmetic overflow |
| 6021 | `InvariantBroken` | Amounts and invariants | A pool invariant does not hold after this instruction; the transaction is refused |
| 6022 | `InsufficientLiquidity` | Amounts and invariants | Not enough liquidity in the vault |
| 6023 | `ZeroShares` | Amounts and invariants | The deposit would mint zero shares |
| 6024 | `FirstDepositTooSmall` | Amounts and invariants | First deposit below the minimum |
| 6025 | `SlippageExceeded` | Amounts and invariants | The result is below the minimum the caller accepts |
| 6026 | `LineTooLarge` | Amounts and invariants | The line exceeds max_line_bps of the NAV: its principal at the approval, or its USDC drawn at a draw |
| 6027 | `OutstandingCapExceeded` | Amounts and invariants | The draw would take the pool's outstanding principal above its cap |
| 6028 | `CollectionPeriodTooShort` | Pool parameters | The collection period is shorter than the minimum |
| 6029 | `AccelerationNoticeTooShort` | Pool parameters | The acceleration notice is below the seven-day floor |
| 6030 | `CancelGraceTooShort` | Pool parameters | Retired: the cancel grace is gone with the cancellation route (decision 38) |
| 6031 | `LineStatus` | Lines | The line is not in the required status |
| 6032 | `AgreementHashRequired` | Lines | The signed agreement's hash is required |
| 6033 | `ExpedienteRootRequired` | Lines | The expediente root is required |
| 6034 | `PagareTemplateHashRequired` | Lines | The pagaré template's hash is required |
| 6035 | `ExpedienteMismatch` | Lines | The expediente root or the terms hash is not the one the borrower requested |
| 6036 | `RequestExpired` | Lines | The request is older than the council's review window |
| 6037 | `SponsorRoleInvalid` | Lines | sponsor_role is unknown |
| 6038 | `TermEnded` | Lines | The term has ended, or would end before the opening |
| 6039 | `InvalidSchedule` | Lines | The installment schedule is not valid |
| 6040 | `MaxMissedTooLow` | Lines | max_missed is below the floor: at least one missed installment must come before the collection trigger |
| 6041 | `ScheduleEndsAfterTerm` | Lines | The last installment would fall due after the term |
| 6042 | `NotScheduled` | Lines | The line has no installment schedule |
| 6043 | `AvailabilityEnded` | Lines | The draw window of this line has closed |
| 6044 | `TooManyDraws` | Lines | The line has reached its maximum number of draws |
| 6045 | `DrawsNotSuspended` | Lines | The line's draws are not suspended |
| 6046 | `WindowClosed` | Lines | The line's draw window has already closed |
| 6047 | `AccelerationState` | Lines | No acceleration is pending on this line, or one already is |
| 6048 | `AccelerationNotDue` | Lines | The acceleration notice has not run |
| 6049 | `LateInterestUnpaid` | Lines | The repayment must cover the late interest due |
| 6050 | `NotEnoughMissed` | Lines | Fewer installments missed than the line allows before collection |
| 6051 | `TermNotEnded` | Lines | The term has not ended |
| 6052 | `WriteOffTooEarly` | Lines | A line in collection is written off only after its collection period, and after its oldest missed due date or its term |
| 6053 | `FixDateInvalid` | The FIX | The FIX date is not a valid date, is in the future, or is older than the published one |
| 6054 | `FixJump` | The FIX | The FIX moves more than the feeder may publish; the council must |
| 6055 | `FixMismatch` | The FIX | The FIX signed is not the one published now |
| 6056 | `FixStale` | The FIX | No FIX is published, or it is too old for a draw or an approval |
| 6057 | `DocumentHashRequired` | Pagarés | The signed document's hash is required |
| 6058 | `SignatureMethodInvalid` | Pagarés | signature_method is unknown, or lacks its certificate fingerprint |
| 6059 | `PagareNotCovered` | Pagarés | No repayment has marked this pagaré paid |
| 6060 | `PagareAlreadyPaid` | Pagarés | This pagaré is already paid |
| 6061 | `CancelGraceNotElapsed` | Pagarés | Retired: the cancellation route is gone (decision 38) |
| 6062 | `EndosoKindInvalid` | Pagarés | This kind of endoso is not one the program records |
| 6063 | `ProcuracionState` | Pagarés | The pagaré has no procuración in force, or already has one |
| 6064 | `AnnotationKindInvalid` | Pagarés | This kind is not a court annotation |
| 6065 | `EntryNotFound` | Pagarés | The referenced registry entry does not exist |
| 6066 | `TooManySidecars` | Appended | A repayment takes at most MAX_SIDECARS_PER_REPAY pagaré records; split the payment |
| 6067 | `CancelGraceTooLong` | Appended | Retired: the cancel grace ceiling is gone with the cancellation route (decision 38) |
| 6068 | `ProposalTooFar` | Appended | The council's decision is dated more than fifteen days ahead |
| 6069 | `FixOutOfBounds` | Appended | The FIX is outside the pool's bounds |
| 6070 | `FxReferenceInvalid` | Appended | The FX reference account is not RedStone's MXNe price |
| 6071 | `FxReferenceStale` | Appended | The FX reference is too old for the feeder to publish against |
| 6072 | `FixOffReference` | Appended | The FIX is too far from the FX reference for the feeder; the council must publish it |
| 6073 | `FixAlreadyPostedToday` | Appended | A FIX was already published today in Mexico City; the feeder publishes once a day |
| 6074 | `PagareAlreadyDelivered` | Appended | This pagaré was already delivered |
| 6075 | `DeliveryHeld` | Appended | Retired: no court act gates the delivery (decision 38-g) |
| 6076 | `PoolImpaired` | Appended | Retired: deposits price at the unprovisioned NAV instead of waiting (decision 40b) |
| 6077 | `BalanceOutstanding` | Appended | The line still owes principal or interest; no título leaves until it owes nothing |
| 6078 | `AssetDecimalsInvalid` | Appended | The asset mint must have 6 decimals, the scale the FIX conversion counts in |

## 12. Toolchain, build and tests

### 12.1 Toolchain

| component | version | where pinned |
|---|---|---|
| Rust | 1.89.0 | `rust-toolchain.toml` |
| Anchor (`anchor-lang`, `anchor-spl`, CLI) | 1.2.1 | `Anchor.toml`, `programs/elote_pool/Cargo.toml` (`=1.2.1`) |
| Solana CLI | 4.1.2 | `Anchor.toml` |
| Platform tools | v1.54 | build flag `--tools-version v1.54` |
| SBPF | v0 | build flag `--arch v0` |
| `mpl-core` crate | 0.12.1 | `programs/elote_pool/Cargo.toml` (CPI builders, types and decoders) |
| LiteSVM | 0.11.0 | dev-dependency |
| Node | ≥ 22.12 | `package.json` |

### 12.2 Build

```sh
NO_DNA=1 anchor build --ignore-keys --arch v0 --tools-version v1.54 -- --features localnet
```

- `--tools-version v1.54`: `cargo-build-sbf` 4.1.0 does not build the program with its default
  platform tools.
- `--arch v0`: the SBPF v0 format of the deployed program, which the Solana 3.x runtime inside
  LiteSVM 0.11 loads.
- `--ignore-keys`: the program id in `declare_id!` and `Anchor.toml` is used as is; the
  `target/deploy/elote_pool-keypair.json` a fresh build writes is not the program's key.
- **Cluster features** select the asset allow-list (`state::ASSET_ALLOWLIST`) and whether the FX
  reference may be missing (§6.1): no feature = mainnet (USDC `EPjF…Dt1v`, MEXe `MEXeQE5U…`; the
  reference is mandatory); `devnet` = the Etherfuse sandbox MEXe and USDC and Circle's devnet USDC;
  `localnet` = any mint the asset policy accepts (the tests and the local e2e). `devnet` and
  `localnet` together is a compile error. A build that forgets its feature gets the strictest list.

Output: `target/deploy/elote_pool.so` and `target/idl/elote_pool.json`. `npm run codama` copies
the IDL to `idl/` and regenerates `clients/js/src/generated/` (Codama, `@solana/kit`; do not edit),
and `npm run check:client` fails if the committed client differs. The hand-written parts of the
client mirror the program: `schedule.ts` (the arrears, the accrual `interestAccrued` /
`interestUnpaid` / `lineOwed`, `lineSchedule`, `maxLinePrincipal`),
`pagare.ts` (the PDAs, `repaySidecars`, `MAX_SIDECARS_PER_REPAY = 3`,
`PAGARE_PROCURADOR_OFFSET = 212`), `terms.ts` (the terms hash) and `fx.ts` (the conversions and the
reference); anything a UI shows about arrears or late interest comes from `schedule.ts`, which must
change in step with `math.rs`.

### 12.3 Tests

`cargo test -p elote_pool` runs the unit tests in `src/` and the integration binaries in
`programs/elote_pool/tests/`; the LiteSVM binaries load `target/deploy/elote_pool.so`, so the
`localnet` build runs first (a `devnet` build fails them at `init_pool` with `AssetNotListed`).
`npm run test:client` runs the JS tests in `clients/js/test/`. The counts are of `#[test]`
functions in each file.

| file | `#[test]` | harness | what it checks |
|---|---|---|---|
| `src/` | 49 | plain | `math` (25), `fx` (9), `state` (10, of which two of the three allow-list tests are compiled out by any one cluster feature), `terms` (2), `instructions::lines` (2), `council` (1) |
| `tests/pool.rs` | 22 | LiteSVM | `init_pool` and the mint and collection policies, deposits and redemptions, dead shares, rounding and slippage, a Token-2022 asset cycle, `NavUpdated` |
| `tests/lines.rs` | 11 | LiteSVM | the line cycle end to end, sponsor, overpayment, liquidity, write-off, the bullet window, the line cap |
| `tests/requests.rs` | 7 | LiteSVM | request, approval of the exact terms, the 15-day window, members as borrowers, reject and withdraw returning rent, the pause on requests |
| `tests/schedule.rs` | 48 (+1 reference) | LiteSVM, clock warped | schedule validation, installments of principal, missed count, late interest and its caps, repayment order, collection and acceleration, write-off timing; decision 48 against `tests/reference/`: interest on the outstanding principal, early payoff, dust and split invariance, no interest unpaid after a principal payment, a proptest state machine (48 cases) |
| `tests/tranches.rs` | 15 (+1 reference) | LiteSVM, clock warped | tranched draws, the strict window, the payability floor, interest from each draw, `MAX_DRAWS`, calendars, `suspend_draws` and `cancel_line` |
| `tests/acceleration.rs` | 12 | LiteSVM | suspension and resumption, acceleration with notice on bullet and scheduled lines, the objective mark superseding a proposal, pool limits and floors, the pause |
| `tests/quita.rs` | 13 | LiteSVM, clock warped | decision 51: the principal quita (total in collection, partial then paid, split over more than three records, at a 50% provision, on a written-off line, ending a procuración), its guards, arrears and accrual after it; recoveries on a written-off line, partial and full, and each pagaré's own FX cost basis; every pool field and the NAV worked by hand; the registry replayed for `Condonacion` |
| `tests/provision.rs` | 9 | LiteSVM | decisions 40 and 40b: the mark and the council's acceleration provision the NAV, redemptions at the provisioned NAV, deposits at the unprovisioned NAV and a recovery handing the entrant nothing, recoveries, the write-off, the rate the council's alone |
| `tests/council.rs` | 17 | LiteSVM | the council constants, `init_pool` against multisig shapes (majority, config authority, size, owner), parameter floors, the two-step rotation |
| `tests/valid_until.rs` | 4 | LiteSVM | `valid_until` refused past or too far ahead |
| `tests/squads.rs` | 7 | LiteSVM + Squads v4 | council instructions through the real `vault_transaction_execute`, stale proposals, a multisig lowered below a majority, the rotation |
| `tests/fx.rs` | 27 | LiteSVM | the FIX: conversions, the daily band, the RedStone reference, the bounds, feeder versus council, one publication a day, draws and repayments at the FIX, FX gains and losses |
| `tests/pagare.rs` | 26 (+1 reference) | LiteSVM + Core 0.15.3 | one frozen asset and one record per draw, every plugin read back through `mpl-core`, the registry and payment entries, the delivery to the borrower or the aval once the line owes nothing, the owner's burn, the council's annulment, coverage oldest first, the procuración, the refusals |
| `tests/core_capabilities.rs` | 8 | LiteSVM + Core 0.15.3 | what Core allows after an asset's creation (§9.1) |
| `tests/known_limits.rs` | 12 | LiteSVM | documentation tests that pin current behaviour and the known limits SECURITY.md lists; they are inverted when a limit is removed |
| `tests/audit.rs`, `tests/reaudit.rs` | 4, 4 | LiteSVM | regressions of the 2026-11-01 audit and the 2026-11-02 re-audit |
| `tests/compute.rs` | 12 | LiteSVM | the figures of §10 under their ceilings |
| `tests/layout.rs` | 2 | LiteSVM | the account sizes 495 / 528 / 363, the zeroed `reserved` tails, and one `CreditLine` layout with or without a sponsor |
| `tests/math_props.rs` | 10 (+1 reference) | proptest | the pure functions over random inputs; the payment order; the accrual against the reference under any sequence; split invariance |
| `tests/reference/mod.rs` | 1 | plain, included by four binaries | the independent oracle: a line as `(time, principal change)` events, `rate × Σ balance × Δt / (10 000 × year)` rounded once; its own hand-number test |
| `tests/vectors.rs` | 1 | plain | 427 arrears vectors (48 accelerated) shared with `clients/js/test/arrears-vectors.json` |
| `clients/js/test/*.test.ts` | | `node --test` (tsx) | `schedule.ts` (the shared vectors), `pagare.ts`, `hashes.ts`, `terms.ts` (the shared vector), `fx.ts`, `core.ts` |

After a deliberate change to `math.rs`, regenerate the vectors with `UPDATE_VECTORS=1 cargo test
--test vectors` in the same commit.

### 12.4 Fixtures

`programs/elote_pool/tests/fixtures/` (see its README), each pinned by sha256 and checked at load:

| file | program | source | sha256 |
|---|---|---|---|
| `mpl_core_program-0.15.3.so` | Metaplex Core `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d` | GitHub release `release/core@0.15.3` | `5b4dacda…dd56ce5` |
| `squads_v4-mainnet.so` | Squads v4 `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf` | the mainnet deployment, dumped 2026-11-04 (upgrade authority none) | `dec8d3e0…a01d34c` |

Core is loaded by every LiteSVM harness (`tests/common/mod.rs`); Squads only by
`tests/squads.rs`. The other tests sign as the council vault directly with signature verification
off.

## 13. Deployment

| cluster | program id | upgrade authority | build |
|---|---|---|---|
| devnet | `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM` | `GU7CfoGZYsfkx23aJoqFjfwyAmLCExqBBoBFrn2EvZKS` (single key) | `main` @ `ce0b1b2`, 2026-11-08; hash `d2b24a8e51648300d747fba7c9308713dd2fe5768bc6a4a8ca1c679bc37ab61a` matches the `--features devnet --arch v0 --tools-version v1.54` build of that commit |
| mainnet-beta | not deployed | | |

The devnet council is vault 0 (`6s6aBtkG6Wm9N6Gx1igKnc3Q3fcJt2r1WtyyLSQrnS6x`) of the Squads v4
multisig `C3sa3p6WRkEULZbVmYGDB3i6YaoDAWJXM9euUPJg8Vap` (2 of 3). The README records the smoke pool
and how to reproduce the hash. The program id is the same in `declare_id!` and `Anchor.toml` for
localnet and devnet. Before mainnet the upgrade authority moves to a Squads multisig, the IDL goes
on chain (`anchor idl init`) and the deployed build is verified with `solana-verify` against a build
of the source with its cluster's feature (no feature for mainnet, `devnet` for devnet: the asset
allow-lists differ, so the bytes do). The program embeds `security.txt` (SECURITY.md).

## 14. Retired

Kept here so older documents and the reserved codes can be read; none of this exists in the program.

- **v1** (before the council, decision 32): a single admin key with an attestor co-signing, and an
  `open_line` instruction that opened a line in one step. Replaced by the council, `request_line`
  and `approve_line`.
- **The guardian** (retired by decision 39): a key that could pause alone, as any council member
  could. `Pool.guardian`, `set_guardian` and `GuardianSet` are gone; the pause is the council's.
- **The cancellation route** (retired by decision 38): `cancel_pagare` burnt a paid pagaré by the
  council, the borrower after a cancel grace, or the procurador through a program-held
  `BurnDelegate`; a paid pagaré waited in a `PAID_PENDING_CANCEL` status (2) and a holder's
  cancellation request (registry kind 10) was the backstop. `set_cancel_grace`, `CancelGraceSet`,
  `PagareCancelled`, the cancel grace fields and flags bit 0 are gone; errors 6030, 6061 and 6067
  stay retired. The paid título is now delivered to its debtor (§9.4).
- **The judicial hold** (retired by decision 38-g): a court annotation briefly held the delivery
  (flags bit 4, error 6075). `court_annotation` now gates nothing.
- **The deposit gate** (retired by decision 40b): deposits refused while a line was impaired (error
  6076). Deposits now price at the unprovisioned NAV.
