# `elote_pool` architecture (draft rewrite)

> **Stale (2026-11-08):** this architecture draft predates decisions 38–41: the paid pagaré is now delivered to its debtor (cancel_pagare, the cancel grace and the procurador's burn are gone; deliver_pagare, burn_pagare and annul_pagare exist), the pause is the council's alone (no guardian), the mark provisions the NAV in full, and a line paid ahead reaches collection after its last due date. README.md, SECURITY.md and docs/interface.md are current; bring this file to them before relying on it.

> **Draft.** A section-by-section rewrite of [ARCHITECTURE.md](../ARCHITECTURE.md), which still
> describes v1 (admin, attestor, `open_line`). This file describes the code as it is on
> `council/pr4` at `9355049`, as merged into `integration/pr4-fixes` (`964bd3e`; the merge
> changes only documents and the KYC PoCs, not `programs/`). When it is reviewed it replaces
> ARCHITECTURE.md.

How the program is built, instruction by instruction, byte by byte. The [README](../README.md)
says what the pool does for investors and borrowers; this file says how the code does it, for
whoever has to change it, audit it or write a client against it. The source of truth is
`programs/elote_pool/src/`. Where this file and the code disagree, the code is right and this file
needs fixing. Citations of the form `file.rs:N` are relative to `programs/elote_pool/src/`;
`instructions/` is abbreviated `ix/`.

Program id `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`, Anchor 1.2.1, Solana 4.1.2, Rust
1.89.0. 34 instructions, 3 account types, 37 events, 66 error codes. 224 Rust tests in 20 binaries
and 21 JS tests (§11).

## 1. Overview and model

### 1.1 One program, eight modules

```
programs/elote_pool/src/
├── lib.rs        declare_id!, security.txt, the #[program] module: 34 thin entry points
├── state.rs      Pool, CreditLine and Pagare layouts, LineStatus, PDA seeds, constants, allow-list
├── council.rs    the Squads v4 council: fresh (valid_until), check, is_member, vault address
├── fx.rs         the FIX: dates, the feeder's band (5% a day, up to 15%), the bounds, MXN⇄USDC conversions and their rounding
├── terms.rs      LineTerms and the terms hash the council approves
├── math.rs       pure arithmetic: share pricing, tranche interest, installments, arrears
├── registry.rs   the pagaré registry: entry hashes and the record's hash chain
├── errors.rs     PoolError (66 codes, 6000..6065)
├── events.rs     37 events, all emitted through emit_cpi!
└── instructions/
    ├── pool.rs   init_pool, deposit, redeem, pause, resume, the council's pool setters,
    │             propose_council, accept_council, set_fx_feeder, post_fix, pool_signer!
    ├── lines.rs  request_line, approve_line, reject_line, withdraw_request, draw (mints the
    │             pagaré), repay, mark_in_collection, write_off, suspend_draws, resume_draws,
    │             propose_acceleration, withdraw_acceleration, accelerate_line, cancel_line
    └── pagare.rs cancel_pagare, endorse_pagare, revoke_procuracion, court_annotation,
                  annotate_payment
```

There is no separate loan program. The loan contract is the `CreditLine` account and
`ix/lines.rs`. The fund is the `Pool` account and `ix/pool.rs`. Both live in one program because
every loan event moves the pool's NAV (`Pool.outstanding_principal`) in the same atomic state
transition, and only the pool PDA signs the vault's outbound transfers (`pool_signer!`,
`ix/pool.rs:25`).

`math.rs` and `fx.rs` have no Anchor accounts and no I/O. Every function takes integers and
returns `Option` (`None` on overflow or division by zero), and the handlers turn `None` into
`PoolError::Overflow`. Every instruction struct carries `#[event_cpi]`, so it takes two extra
accounts (the event authority PDA and the program) and emits its events as self-CPIs.

### 1.2 The money model

```
NAV   = vault.amount + pool.outstanding_principal          (both in USDC base units)
price = NAV / share_mint.supply
```

- **Shares (ERC-4626).** `deposit` mints `assets * supply / NAV` shares, one to one while
  `supply == 0` (`math::shares_for_deposit`, `math.rs:13`). `redeem` pays `shares * NAV / supply`
  assets (`math::assets_for_redeem`, `math.rs:24`). Both products go through `u128` and both
  divisions round down, in the pool's favour.
- **Outstanding principal at USDC cost.** `Pool.outstanding_principal` is the USDC the pool
  delivered on draws and has not yet recovered or written off. `draw` adds the USDC it delivered.
  `repay` subtracts the cost of the principal repaid, valued at the FIX each pagaré was drawn at
  (`fx::cost_of`). `write_off` subtracts the line's remaining cost, `usdc_drawn -
  usdc_cost_repaid` (`ix/lines.rs:1155-1157`).
- **Debt in MXN, settlement in USDC at the FIX.** Every amount on a line and a pagaré is in
  centavos of MXN (`CreditLine.currency = CURRENCY_MXN = 484`, `state.rs:55`). The vault, deposits,
  redemptions and repayments are in USDC base units. A draw delivers
  `usdc_for_mxn(amount_mxn, fix)`, rounded down. A repayment credits `mxn_for_usdc(max_usdc, fix)`
  MXN, rounded down, and takes `usdc_ceil_for_mxn(applied, fix)` USDC, rounded up (§6).
- **Interest counts when collected.** Nothing accrues in the NAV. Interest, late interest and the
  FX result of the principal enter the NAV only when a repayment brings the USDC into the vault.

| event | vault | outstanding_principal | NAV | price |
|---|---|---|---|---|
| `deposit` | + assets | | + | unchanged (shares minted pro rata) |
| `redeem` | − assets | | − | unchanged (shares burnt pro rata) |
| `draw` | − USDC delivered | + USDC delivered | unchanged | unchanged |
| `repay`, principal part | + USDC that paid it | − cost at the pagarés' FIX | + `FxRealized.delta` (may be negative) | moves by the delta |
| `repay`, interest and late interest | + | | + | up |
| `write_off` | | − `usdc_drawn - usdc_cost_repaid` | − | down |

`Pool.interest_collected` (USDC, `usdc_for_mxn(interest_part)`), `Pool.written_off` (USDC) and
`Pool.fx_realized` (signed USDC) are informative running totals (`ix/lines.rs:887-890`,
`ix/lines.rs:1158`).

**Dead shares.** The first deposit must be at least `min_first_deposit(decimals)` =
`max(10^decimals, 1 000 × DEAD_SHARES)` (`state.rs:33`), and `DEAD_SHARES` (1 000) of the shares
it mints go to the pool-owned `dead_shares` account, which no instruction burns. The supply never
falls below 1 000 once anything was deposited.

**Slippage floors.** `deposit` takes `min_shares_out` and `redeem` takes `min_assets_out`
(`SlippageExceeded`).

**`Pool::check`** (`state.rs:205`) runs at the end of every money instruction on the balances
reloaded after the CPIs: `vault.amount + outstanding_principal` fits a `u64`, and a share supply,
when passed, is zero or at least `DEAD_SHARES` (`InvariantBroken`). `deposit`, `redeem`, `repay`
and `write_off` pass the supply; `draw` checks the NAV leg only.

## 2. Roles

| role | identified by | can |
|---|---|---|
| council | `Pool.council`: vault 0 of `Pool.multisig`, a Squads v4 multisig (`council_kind = COUNCIL_SQUADS`) | every council instruction (§4, marked **C**): approve, reject and limit lines, suspend, resume and accelerate draws, write off, publish the FIX and name the feeder, the pool parameters, rotate itself, and, as the holder of the pagarés in the pool, cancel, endorse en procuración, revoke, annotate court acts and receipts |
| council member | a key in the multisig's member list | `pause` alone; cannot be a borrower (`BorrowerIsCurator` at `approve_line`) or a procurador (`EndorseeInvalid`) |
| guardian | `Pool.guardian` (zero key for none), set at `init_pool` and by `set_guardian` | `pause` only |
| fx_feeder | `Pool.fx_feeder` (zero key for none), set by `set_fx_feeder` | `post_fix` for a later date within 5% a day (up to 15%) of the FIX in force, within the pool's FIX bounds |
| creator | `Pool.creator`: signer and payer of `init_pool`, part of the pool's seeds | nothing after `init_pool` |
| borrower | `CreditLine.borrower` | `request_line`, `withdraw_request`, `draw` its approved line (signing the FIX and the derived pagaré hash), `cancel_pagare` alone on its own paid pagaré once `cancel_grace_seconds` have run from the payment |
| payer (relayer) | any signer passed as `payer` | funds the rent of a request (gets it back on reject or withdraw), of the pagaré asset and its record at `draw`, and of a plugin at `endorse_pagare`; receives the asset's rent at `cancel_pagare`. Any signer can also `repay` (the `payer` of `repay` is the one whose USDC moves) |
| sponsor | `CreditLine.sponsor`, `sponsor_role` 0 promoter or 1 guarantor | co-signs `request_line` when named (`SponsorRequired`); the role is recorded, not enforced |
| procurador | `Pagare.procurador`, named by `endorse_pagare` | `cancel_pagare` alone on a paid pagaré of its procuración, burning through the program address `["procurador", pagare, procurador]` |
| investor | holder of shares | `deposit`, `redeem`. When `Pool.permissioned` (share mint `DefaultAccountState = Frozen`), the share mint's freeze authority (Token ACL's `MintConfig` in production) thaws each investor's share account first: that is the allow-list. The program records `permissioned`, not who holds the freeze authority |
| anyone | | `repay` any drawn line, `mark_in_collection` |
| upgrade authority | BPF loader authority of the program id | replace the program (§12) |

### 2.1 The council and `council::check`

The council is not a key the program trusts by address alone. Every council instruction except
the rotation pair (below) takes the stored multisig account (`address = pool.multisig`, else
`CouncilInvalid`) and, when the council is the signer (always, except the feeder's `post_fix` and
the procurador's and borrower's `cancel_pagare`), runs `council::check(multisig, pool.council,
pool.council_vault_bump)` (`council.rs:62`), which reads the Squads account at fixed offsets, with
no Squads crate and no CPI:

1. the account is owned by Squads v4 (`SQUADS_PROGRAM_ID`, `state.rs:57`);
2. `create_program_address(["multisig", multisig, "vault", 0, vault_bump])` under Squads equals
   `pool.council` (`council.rs:28`);
3. the 8-byte discriminator is `sha256("account:Multisig")[..8]` (`council.rs:10`);
4. **autonomous:** the `config_authority` bytes 40..72 are all zero, so only the members' votes
   change the multisig (`council.rs:67`);
5. the members list (after `threshold` at 72, `rent_collector: Option<Pubkey>` at 94 and the bump)
   has at most `MAX_COUNCIL_MEMBERS` = 10 entries of 33 bytes (`CouncilTooLarge`, `council.rs:53`);
6. **majority:** with `voters` the members whose permission mask has the Vote bit (`1 << 1`),
   `voters > 0` and `threshold > voters / 2` (`council.rs:69-70`).

Any failure is `CouncilInvalid` except the size, `CouncilTooLarge`. `init_pool` and
`accept_council` run the same check on the multisig they are given, with the canonical vault bump
(`council::canonical_vault`, `council.rs:34`), and store that bump in `Pool.council_vault_bump`.

The council signs only through Squads `vault_transaction_execute` (Squads' `invoke_signed`), so
the CPI depth of a council instruction that reaches Metaplex Core is Squads (1), `elote_pool` (2),
Core (3), system (4) (`tests/squads.rs`).

**`valid_until`.** Every council instruction takes `valid_until: i64` as its last argument and
starts with `council::fresh(valid_until)` (`council.rs:22`): `now > valid_until` is
`ProposalExpired`, `now == valid_until` is accepted. Squads proposals never expire, so the program
bounds them. `pause`, `init_pool` and the non-council instructions take no `valid_until`.

**Rotation** is two steps: `propose_council` (signed by the council, `has_one = council`, after
`council::check` on the current multisig; only `COUNCIL_SQUADS`) stores `pending_council` and its
kind; `accept_council` is signed by the proposed vault, accepts only `COUNCIL_SQUADS`, and runs
`council::check` on the new multisig before writing it. The zero key in `propose_council` withdraws a pending proposal.

`council::is_member` (`council.rs:75`) reads the same member list and is used by `pause`
(a member pauses alone), `approve_line` (`BorrowerIsCurator`) and `endorse_pagare`
(`EndorseeInvalid`).

## 3. Accounts and the PDA tree

```
Pool            PDA ["pool", creator, asset_mint]              program-owned, 527 bytes
├── share_mint  creator-supplied mint                          classic or Token-2022, decimals = asset's,
│                                                              mint authority = pool PDA
├── vault       PDA ["vault", pool]                            token account of asset_mint, authority = pool
├── dead_shares PDA ["dead", pool]                             token account of share_mint, authority = pool
├── collection  creator-supplied Core CollectionV1             update authority = pool PDA, bare at init
└── CreditLine  PDA ["line", pool, line_id u64 LE]             program-owned, 529 bytes
    ├── Pagare  PDA ["pagare", line, draw_index u16 LE]        program-owned, 399 bytes: the pagaré record
    │   └── procurador authority PDA ["procurador", pagare, procurador]   no account; the BurnDelegate's
    │                                                          authority during a procuración
    └── asset   PDA ["pagare_asset", line, draw_index u16 LE]  Metaplex Core AssetV1, owned by the Core
                                                               program, held by the pool PDA

council         Squads PDA ["multisig", multisig, "vault", [0]] under SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf
event authority PDA ["__event_authority"]                      Anchor #[event_cpi]
```

Seeds are in `state.rs:5-21`.

| PDA | bump | how it is verified |
|---|---|---|
| pool | `Pool.bump`, canonical (Anchor `init`) | every instruction re-derives `[POOL, pool.creator, pool.asset_mint, pool.bump]` |
| vault, dead_shares | canonical at `init`, not stored | later instructions compare the address with `Pool.vault` / `Pool.dead_shares` |
| line | `CreditLine.bump`, canonical (Anchor `init` in `request_line`) | re-derived with the stored bump in `draw`, `repay`, `mark_in_collection`, `write_off`, the council line instructions and the pagaré instructions; `approve_line` and `reject_line` use `has_one = pool`, `withdraw_request` uses `has_one = borrower` |
| pagare | client-passed `pagare_bump` at `draw`, then `Pagare.bump` | `draw` verifies the seeds with one `create_program_address`; the pagaré instructions re-derive with the stored bump |
| pagare_asset | client-passed `asset_bump` at `draw`, not stored | `draw` verifies the seeds; later instructions take `address = pagare.asset` |
| procurador authority | `find_program_address` each time | `endorse_pagare` (`ix/pagare.rs:277`) and `cancel_pagare` (`ix/pagare.rs:116`) |
| council vault | `Pool.council_vault_bump`, canonical at `init_pool` / `accept_council` | `council::vault_address` |

- The pool address is a function of `(creator, asset_mint)`.
- `line_id` must equal `pool.line_count` at the request, so line ids are dense `0, 1, 2…`. A
  rejected or withdrawn request closes its account; its id is not reused.
- `draw_index` is `CreditLine.draws` before the draw, so a line's pagarés are dense `0, 1, 2…`
  below `MAX_DRAWS` (64).
- The pool PDA signs every outbound CPI (share mints, vault transfers, Core create, thaw, burn
  and plugin calls) with `pool_signer!(pool)` = `["pool", creator, asset_mint, bump]`
  (`ix/pool.rs:25`).

All three accounts are Borsh-serialized with no padding, end in a zeroed `reserved` tail, and are
sized by `InitSpace` (`tests/layout.rs` pins 527, 529 and 399 bytes). Offsets include the 8-byte
Anchor discriminator, so they can be used directly in `memcmp` filters.

| account | discriminator (hex) |
|---|---|
| `Pool` | `f19a6d0411b16dbc` |
| `CreditLine` | `dce2cd18dc978168` |
| `Pagare` | `8c8e1bef938349f7` |

### 3.1 `Pool` (527 bytes, `state.rs:107`)

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 32 | `council` | vault 0 of `multisig`; signs every council instruction |
| 40 | 32 | `asset_mint` | part of the seeds; on this build's allow-list (§11.2) |
| 72 | 32 | `share_mint` | mint authority = pool PDA |
| 104 | 32 | `vault` | `["vault", pool]` |
| 136 | 32 | `dead_shares` | `["dead", pool]` |
| 168 | 32 | `collection` | the series' Core collection |
| 200 | 8 | `outstanding_principal` | USDC cost lent out (§1.2) |
| 208 | 8 | `interest_collected` | USDC, informative |
| 216 | 8 | `written_off` | USDC, informative |
| 224 | 8 | `line_count` | next `line_id` |
| 232 | 4 | `grace_period_seconds` | grace after a due date or the term before late interest, collection and write-off; fixed at `init_pool` |
| 236 | 4 | `collection_period_seconds` | from the mark to the earliest write-off; ≥ 30 days; `set_collection_period` |
| 240 | 2 | `max_line_bps` | largest line as a share of the NAV at approval, `1..=10 000`; `init_pool`, `set_max_line_bps` |
| 242 | 1 | `asset_controls` | bit 0 asset mint has a freeze authority, bit 1 Token-2022 `Pausable` |
| 243 | 1 | `permissioned` | share mint is `DefaultAccountState = Frozen` |
| 244 | 1 | `paused` | |
| 245 | 1 | `bump` | pool PDA bump |
| 246 | 32 | `creator` | part of the seeds |
| 278 | 1 | `council_kind` | `COUNCIL_SQUADS` (0); `COUNCIL_GENERIC` (1) is defined and refused by `accept_council` |
| 279 | 32 | `multisig` | the Squads multisig |
| 311 | 1 | `council_vault_bump` | |
| 312 | 32 | `pending_council` | set by `propose_council`; zero when none |
| 344 | 1 | `pending_council_kind` | |
| 345 | 32 | `guardian` | zero when none |
| 377 | 8 | `max_outstanding_principal` | cap on `outstanding_principal` at every draw, ≥ 1 |
| 385 | 4 | `acceleration_notice_seconds` | ≥ 7 days |
| 389 | 4 | `cancel_grace_seconds` | ≥ 30 days |
| 393 | 8 | `impaired_principal` | zero; no instruction writes it |
| 401 | 2 | `provision_bps` | zero; no instruction writes it |
| 403 | 32 | `fx_feeder` | zero when none |
| 435 | 8 | `fix_rate` | MXN per USD × 10⁴; zero before the first publication |
| 443 | 4 | `fix_date` | `yyyymmdd` Banxico determined it; never moves back |
| 447 | 8 | `fix_posted_at` | slot time of the last `post_fix` |
| 455 | 8 | `fx_realized` | `i64`, Σ `FxRealized.delta` |
| 463 | 64 | `reserved` | zero |

Helpers: `asset_freezable()`, `asset_pausable()`, `max_line_principal(nav) = nav * max_line_bps /
10 000` rounded down (`state.rs:195`), `check()` (§1.2).

### 3.2 `CreditLine` (529 bytes, `state.rs:235`)

`sponsor` is `Option<Pubkey>`. Borsh writes `Some(k)` as one tag byte plus 32 and `None` as the
tag byte alone, so every field after it sits 32 bytes earlier on a line without a sponsor (the
account keeps its size; the tail is zero). Both offsets are given.

| offset (sponsor `Some`) | offset (sponsor `None`) | size | field | notes |
|---|---|---|---|---|
| 0 | 0 | 8 | discriminator | |
| 8 | 8 | 32 | `pool` | |
| 40 | 40 | 8 | `line_id` | |
| 48 | 48 | 32 | `borrower` | |
| 80 | 80 | 1 + 32 / 1 | `sponsor` | `Option<Pubkey>` |
| 113 | 81 | 1 | `sponsor_role` | 0 promoter, 1 guarantor |
| 114 | 82 | 32 | `agreement_hash` | never zero |
| 146 | 114 | 32 | `expediente_root` | salted merkle root of the credit file; never zero |
| 178 | 146 | 1 | `status` | `LineStatus`: 0 `Requested`, 1 `Approved`, 2 `Active`, 3 `Repaid`, 4 `WrittenOff`, 5 `InCollection`, 6 `Cancelled` (`state.rs:217`) |
| 179 | 147 | 8 | `principal` | MXN centavos; the commitment |
| 187 | 155 | 2 | `rate_bps` | annual, ≤ 10 000 |
| 189 | 157 | 8 | `term_end` | unix seconds |
| 197 | 165 | 8 | `availability_end` | draws accepted while `now < availability_end`; 0 until approval |
| 205 | 173 | 8 | `drawn` | MXN centavos drawn; never decreases |
| 213 | 181 | 2 | `draws` | tranches so far, ≤ 64 |
| 215 | 183 | 8 | `interest_due` | MXN; Σ per-tranche interest fixed at each draw, cut at an acceleration |
| 223 | 191 | 8 | `principal_repaid` | MXN |
| 231 | 199 | 8 | `interest_repaid` | MXN |
| 239 | 207 | 2 | `pagares_paid` | payment cursor: oldest pagaré not yet marked paid |
| 241 | 209 | 8 | `first_due_at` | 0 on a bullet line |
| 249 | 217 | 8 | `interval_seconds` | 0 on a bullet line |
| 257 | 225 | 2 | `installments` | `1..=120`; 0 on a bullet line |
| 259 | 227 | 2 | `late_bps` | annual late rate, ≤ 10 000, scheduled or bullet |
| 261 | 229 | 1 | `max_missed` | `3..=installments`; 0 on a bullet line |
| 262 | 230 | 8 | `late_collected` | MXN |
| 270 | 238 | 8 | `late_paid_until` | late interest settled to this instant |
| 278 | 246 | 8 | `drawn_at` | first draw |
| 286 | 254 | 8 | `closed_at` | `Repaid`, `WrittenOff` or `Cancelled` |
| 294 | 262 | 8 | `accelerated_at` | the mark; 0 before |
| 302 | 270 | 1 | `bump` | |
| 303 | 271 | 2 | `currency` | 484 |
| 305 | 273 | 8 | `requested_at` | |
| 313 | 281 | 8 | `approved_at` | |
| 321 | 289 | 32 | `terms_hash` | `terms::terms_hash` (§4.3) |
| 353 | 321 | 32 | `pagare_template_hash` | hash of the approved pagaré text |
| 385 | 353 | 32 | `payer` | funded the request; gets the rent on reject or withdraw |
| 417 | 385 | 16 | `draw_time_weight` | `u128`, Σ `amount_mxn × drawn_at` |
| 433 | 401 | 8 | `availability_end_saved` | the window's end before a suspension; non-zero ⇔ suspended |
| 441 | 409 | 8 | `acceleration_due_at` | when `accelerate_line` may fire; 0 when none pending |
| 449 | 417 | 8 | `usdc_drawn` | USDC delivered over the draws |
| 457 | 425 | 8 | `usdc_cost_repaid` | USDC cost of the principal repaid |
| 465 | 433 | 64 | `reserved` | zero |

`CreditLine::schedule()` returns `None` when `installments == 0` (a bullet line)
(`state.rs:437`); `anchor()` = `first_due_at - interval` (`state.rs:449`); `last_due()` =
`first_due_at + (installments - 1) × interval`, or `term_end` on a bullet line (`state.rs:456`);
`available()` = `principal - drawn` (`state.rs:467`).

### 3.3 `Pagare` (399 bytes, `state.rs:358`)

The program's record of one pagaré, written by `draw` next to the Core asset. It holds no folio
and nothing personal and is never closed: after `cancel_pagare` it stays as the PAGADO mark.

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 32 | `line` | |
| 40 | 2 | `draw_index` | |
| 42 | 32 | `asset` | `["pagare_asset", line, draw_index]` |
| 74 | 32 | `pagare_hash` | derived on chain at `draw` (§4.4) |
| 106 | 8 | `amount_mxn` | the sum the pagaré promises, MXN centavos |
| 114 | 8 | `issued_at` | |
| 122 | 8 | `due_at` | the line's `last_due()` at issuance |
| 130 | 8 | `drawn_through` | the line's `drawn` after this tranche |
| 138 | 32 | `cert_fingerprint` | `sha256(salt ‖ fingerprint)` of the PSC or e.firma certificate; zero only with `signature_method` 0 |
| 170 | 1 | `signature_method` | 0 wallet, 1 PSC + wallet, 2 SAT e.firma + wallet |
| 171 | 1 | `status` | 0 `PAGARE_OUTSTANDING`, 2 `PAGARE_PAID_PENDING_CANCEL`, 1 `PAGARE_PAID` |
| 172 | 8 | `paid_at` | when the covering repayment landed |
| 180 | 32 | `receipt_hash` | the covering repayment's receipt hash; replaced by `annotate_payment` until the burn |
| 212 | 32 | `cancel_receipt_hash` | the receipt `cancel_pagare` was given |
| 244 | 32 | `procurador` | procurador in force; zero when none (`PAGARE_PROCURADOR_OFFSET = 244` in `clients/js/src/pagare.ts`) |
| 276 | 1 | `flags` | bit 0 the asset carries the `BurnDelegate` plugin, bit 1 a procuración is in force |
| 277 | 32 | `log_root` | registry hash chain (§8.3) |
| 309 | 4 | `entry_count` | `u32`; 1 after the draw |
| 313 | 1 | `last_kind` | `KIND_*` of the last entry |
| 314 | 1 | `bump` | |
| 315 | 8 | `fx_rate` | the FIX the borrower signed for this draw |
| 323 | 4 | `fx_date` | its date |
| 327 | 8 | `usdc_delivered` | `usdc_for_mxn(amount_mxn, fx_rate)` |
| 335 | 32 | `delivery_hash` | `sha256(pagare_hash ‖ fx_rate ‖ fx_date ‖ usdc_delivered)` |
| 367 | 32 | `reserved` | zero |

### 3.4 Constants

| constant | value | where | role |
|---|---|---|---|
| `DEAD_SHARES` | 1 000 | `state.rs:25` | shares locked by the first deposit |
| `MIN_FIRST_DEPOSIT_DEAD_RATIO` | 1 000 | `state.rs:28` | first deposit ≥ 1 000 × `DEAD_SHARES` base units |
| `BPS` | 10 000 | `state.rs:37` | |
| `MAX_DRAWS` | 64 | `state.rs:39` | tranches per line |
| `MAX_INSTALLMENTS` | 120 | `state.rs:47` | bounds the arrears loop |
| `MIN_MAX_MISSED` | 3 | `state.rs:49` | |
| `MIN_COLLECTION_PERIOD_SECONDS` | 2 592 000 (30 d) | `state.rs:51` | |
| `SECONDS_PER_YEAR` | 31 536 000 (365 d) | `state.rs:53` | |
| `CURRENCY_MXN` | 484 | `state.rs:55` | |
| `MAX_COUNCIL_MEMBERS` | 10 | `state.rs:65` | |
| `REQUEST_TTL_SECONDS` | 1 296 000 (15 d) | `state.rs:68` | approval window from the request |
| `MIN_ACCELERATION_NOTICE_SECONDS` | 604 800 (7 d) | `state.rs:70` | |
| `MIN_CANCEL_GRACE_SECONDS` | 2 592 000 (30 d) | `state.rs:73` | |
| `FIX_SCALE` | 10 000 | `fx.rs:10` | FIX × 10⁴ |
| `MAX_FIX_AGE_DAYS` | 5 | `fx.rs:14` | oldest FIX a draw accepts |
| `MAX_FIX_JUMP_BPS_PER_DAY`, `MAX_FIX_JUMP_DAYS` | 500, 3 | `fx.rs` | the feeder's band: 5% a day between the FIX dates, one to three days |
| `TERMS_DOMAIN` | `"elote_pool:terms:v1"` | `terms.rs:10` | |

## 4. Instructions

34 instructions in `lib.rs:44-273`. For each: accounts (the count includes the two `#[event_cpi]`
accounts), arguments, checks in the order the handler makes them, the errors it returns, what it
writes and the events it emits. Anchor's own constraint errors (`ConstraintSeeds`,
`ConstraintHasOne` mapped to the codes shown, `AccountNotInitialized`, …) are raised before the
handler. **C** marks the instructions that run `council::check`; **V** the ones that take
`valid_until`.

### 4.1 Summary

| instruction | `lib.rs` | handler | signers | **V** | **C** | paused |
|---|---|---|---|---|---|---|
| `init_pool` | 53 | `ix/pool.rs:256` | creator | | on the given multisig | n/a |
| `deposit` | 105 | `ix/pool.rs:408` | depositor | | | blocked |
| `redeem` | 111 | `ix/pool.rs:531` | holder | | | blocked |
| `pause` | 246 | `ix/pool.rs:612` | member, guardian or council | | (reads members) | n/a |
| `resume` | 251 | `ix/pool.rs:650` | council | V | C | allowed |
| `set_guardian` | 256 | `ix/pool.rs:660` | council | V | C | allowed |
| `set_max_line_bps` | 261 | `ix/pool.rs:671` | council | V | C | allowed |
| `set_outstanding_cap` | 266 | `ix/pool.rs:683` | council | V | C | allowed |
| `set_acceleration_notice` | 271 | `ix/pool.rs:695` | council | V | C | allowed |
| `set_collection_period` | 59 | `ix/pool.rs:352` | council | V | C | allowed |
| `set_cancel_grace` | 65 | `ix/pool.rs:725` | council | V | C | allowed |
| `propose_council` | 224 | `ix/pool.rs:756` | council | V | yes | allowed |
| `accept_council` | 229 | `ix/pool.rs:783` | proposed vault | V | on the new multisig | allowed |
| `set_fx_feeder` | 234 | `ix/pool.rs:824` | council | V | C | allowed |
| `post_fix` | 240 | `ix/pool.rs:855` | feeder or council | V | council branch | allowed |
| `request_line` | 118 | `ix/lines.rs:66` | borrower, payer, sponsor if named | | | blocked |
| `approve_line` | 124 | `ix/lines.rs:188` | council | V | C | blocked |
| `reject_line` | 129 | `ix/lines.rs:248` | council | V | C | allowed |
| `withdraw_request` | 134 | `ix/lines.rs:277` | borrower | | | allowed |
| `draw` | 145 | `ix/lines.rs:567` | borrower, payer | | | blocked |
| `repay` | 197 | `ix/lines.rs:785` | payer | | | allowed |
| `mark_in_collection` | 210 | `ix/lines.rs:1042` | anyone (fee payer) | | | allowed |
| `write_off` | 217 | `ix/lines.rs:1122` | council | V | C | allowed |
| `suspend_draws` | 71 | `ix/lines.rs:1223` | council | V | C | allowed |
| `resume_draws` | 82 | `ix/lines.rs:1238` | council | V | C | allowed |
| `propose_acceleration` | 88 | `ix/lines.rs:1256` | council | V | C | allowed |
| `withdraw_acceleration` | 93 | `ix/lines.rs:1273` | council | V | C | allowed |
| `accelerate_line` | 99 | `ix/lines.rs:1309` | council | V | C | allowed |
| `cancel_line` | 76 | `ix/lines.rs:1333` | council | V | C | allowed |
| `cancel_pagare` | 163 | `ix/pagare.rs:90` | council, procurador or borrower; payer | V | council branch | allowed |
| `endorse_pagare` | 172 | `ix/pagare.rs:251` | council (holder), payer | V | C | allowed |
| `revoke_procuracion` | 179 | `ix/pagare.rs:385` | council (holder), payer | V | C | allowed |
| `court_annotation` | 186 | `ix/pagare.rs:467` | council (holder) | V | C | allowed |
| `annotate_payment` | 203 | `ix/pagare.rs:534` | council (holder) | V | C | allowed |

The pause blocks exactly `deposit`, `redeem`, `request_line`, `approve_line` and `draw`. Money can
always come back in and facts can always be recorded.

Council instructions of the shape `CouncilPool` (`ix/pool.rs:627`: `council` signer, `multisig`,
`pool` mut, `has_one = council` → `Unauthorized`) and `CouncilLine` (`ix/lines.rs:1178`: the same
plus `line` mut, seeds with the stored bump, `has_one = pool` → `WrongAccount`) share their
accounts. `SetCollectionPeriod`, `SetCancelGrace` and `SetFxFeeder` have the `CouncilPool` shape
under their own names.

### 4.2 Pool and council

#### `init_pool(params: InitPoolParams)` (`ix/pool.rs:256`)

- **Accounts (13):** `creator` (signer, mut, pays); `asset_mint`; `pool` (`init` at
  `["pool", creator, asset_mint]`); `share_mint` (existing, validated); `vault` (`init` at
  `["vault", pool]`, mint `asset_mint`, authority pool, `token_program`); `dead_shares` (`init` at
  `["dead", pool]`, mint `share_mint`, authority pool, `share_token_program`); `collection`;
  `multisig`; `token_program`; `share_token_program`; `system_program`; + 2.
- **Args** (`InitPoolParams`, `ix/pool.rs:236`): `grace_period_seconds: u32`, `max_line_bps: u16`,
  `collection_period_seconds: u32`, `guardian: Pubkey`, `max_outstanding_principal: u64`,
  `acceleration_notice_seconds: u32`, `cancel_grace_seconds: u32`.
- **Checks, in order:** `1 <= max_line_bps <= 10 000` (`WrongAmount`); `max_outstanding_principal
  >= 1` (`WrongAmount`); collection period ≥ 30 d (`CollectionPeriodTooShort`); acceleration
  notice ≥ 7 d (`AccelerationNoticeTooShort`); cancel grace ≥ 30 d (`CancelGraceTooShort`);
  `council::check` on `multisig` with its canonical vault (`CouncilInvalid`, `CouncilTooLarge`);
  `asset_listed(asset_mint)` (`AssetNotListed`, §11.2); asset mint ≠ share mint
  (`AssetIsShareMint`); `10^decimals` fits a `u64` (`WrongAccount`); the asset-mint policy
  `asset_controls` (`ix/pool.rs:116`): refuses `TransferFeeConfig`, `PermanentDelegate`,
  `TransferHook`, `NonTransferable`, the three confidential extensions, `MintCloseAuthority`, and
  `DefaultAccountState` other than `Initialized` (`AssetMintNotAllowed`); records a freeze
  authority and `Pausable` as bits. The share-mint policy `validate_share_mint`
  (`ix/pool.rs:162`): owned by `share_token_program`, mint authority = pool, supply 0, decimals =
  asset's; refuses `TransferFeeConfig`, `TransferHook`, `MintCloseAuthority`, `PermanentDelegate`,
  `InterestBearingConfig`, `ScaledUiAmount`, `Pausable` and the confidential extensions;
  `DefaultAccountState = Frozen` sets `permissioned` and requires a freeze authority
  (`ShareMintNotAllowed`). The collection `validate_collection` (`ix/pool.rs:211`): owned by Core,
  key `CollectionV1`, update authority = pool, exactly the base record with no plugin header,
  `num_minted == 0`, `current_size == 0`, every read at a fixed offset or bounded by a length read
  first (`CollectionNotAllowed`).
- **Writes:** every `Pool` field; `council` = canonical vault 0 of `multisig`, `council_kind =
  COUNCIL_SQUADS`, `council_vault_bump`; FX fields, pending council, impairment fields zero;
  `reserved` zero. On a permissioned share mint the dead account is born frozen and is thawed by
  the freeze authority before the first deposit.
- **Events:** `PoolInitialized`.

#### `deposit(assets: u64, min_shares_out: u64)` (`ix/pool.rs:408`)

- **Accounts (12):** `depositor` (signer); `pool`; `asset_mint` (= `pool.asset_mint`);
  `share_mint` (mut, = `pool.share_mint`); `vault` (mut); `dead_shares` (mut);
  `depositor_asset_account` (mut, mint and owner checked); `depositor_share_account` (mut, mint
  and owner checked); `token_program`; `share_token_program`; + 2. Address mismatches are
  `WrongAccount`.
- **Checks:** not paused (`Paused`); `assets > 0` (`WrongAmount`); supply and NAV read before any
  transfer; on the first deposit `assets >= min_first_deposit(decimals)`
  (`FirstDepositTooSmall`); `minted = shares_for_deposit(...)` (`Overflow`, which also covers
  `NAV == 0` with `supply > 0`); `shares = minted - DEAD_SHARES` on the first deposit
  (`Overflow`); `shares > 0` (`ZeroShares`); `shares >= min_shares_out` (`SlippageExceeded`).
- **CPIs:** `transfer_checked` depositor → vault; `mint_to` shares → depositor (pool signs); on the
  first deposit `mint_to` 1 000 → `dead_shares`.
- **Writes:** no `Pool` field. `Pool::check` on the reloaded vault and supply (`InvariantBroken`).
- **Events:** `Deposited`, `NavUpdated`.

#### `redeem(shares: u64, min_assets_out: u64)` (`ix/pool.rs:531`)

- **Accounts (11):** `holder` (signer); `pool`; `asset_mint`; `share_mint` (mut); `vault` (mut);
  `holder_share_account` (mut); `holder_asset_account` (mut); `token_program`;
  `share_token_program`; + 2.
- **Checks:** not paused; `shares > 0` (`WrongAmount`); `assets = assets_for_redeem(shares,
  supply, nav)` on the pre-burn supply (`Overflow`); `assets > 0` (`WrongAmount`); `assets >=
  min_assets_out` (`SlippageExceeded`); `assets <= vault.amount` (`InsufficientLiquidity`).
- **CPIs:** `burn` (holder signs); `transfer_checked` vault → holder (pool signs).
- **Writes:** no `Pool` field; `Pool::check`.
- **Events:** `Redeemed`, `NavUpdated`.

#### `pause()` (`ix/pool.rs:612`)

- **Accounts (5):** `signer`; `multisig` (= `pool.multisig`, `CouncilInvalid`); `pool` (mut); + 2.
- **Checks:** the signer is the council, the guardian (non-zero), or, under `COUNCIL_SQUADS`, a key
  in the multisig's member list (`council::is_member`) (`Unauthorized`).
- **Writes:** `paused = true`. **Events:** `PoolPaused { by }`.

#### `resume(valid_until)` — **V C** (`ix/pool.rs:650`)

- **Accounts (5):** `CouncilPool`. **Checks:** `ProposalExpired`; `council::check`.
- **Writes:** `paused = false`. **Events:** `PoolResumed`.

#### `set_guardian(guardian: Pubkey, valid_until)` — **V C** (`ix/pool.rs:660`)

- **Accounts (5):** `CouncilPool`. **Checks:** `ProposalExpired`; `council::check`. Any key,
  zero for none.
- **Writes:** `guardian`. **Events:** `GuardianSet`.

#### `set_max_line_bps(max_line_bps: u16, valid_until)` — **V C** (`ix/pool.rs:671`)

- **Accounts (5):** `CouncilPool`. **Checks:** `ProposalExpired`; `1..=10 000` (`WrongAmount`);
  `council::check`. Lines already approved keep their principal.
- **Writes:** `max_line_bps`. **Events:** `MaxLineBpsSet`.

#### `set_outstanding_cap(max_outstanding_principal: u64, valid_until)` — **V C** (`ix/pool.rs:683`)

- **Accounts (5):** `CouncilPool`. **Checks:** `ProposalExpired`; `>= 1` (`WrongAmount`);
  `council::check`. A cap below the current outstanding stops new draws only.
- **Writes:** `max_outstanding_principal`. **Events:** `OutstandingCapSet`.

#### `set_acceleration_notice(acceleration_notice_seconds: u32, valid_until)` — **V C** (`ix/pool.rs:695`)

- **Accounts (5):** `CouncilPool`. **Checks:** `ProposalExpired`; ≥ 7 d
  (`AccelerationNoticeTooShort`); `council::check`. A pending proposal keeps its own deadline.
- **Writes:** `acceleration_notice_seconds`. **Events:** `AccelerationNoticeSet`.

#### `set_collection_period(collection_period_seconds: u32, valid_until)` — **V C** (`ix/pool.rs:352`)

- **Accounts (5):** `council`, `multisig`, `pool` (mut). **Checks:** `ProposalExpired`; ≥ 30 d
  (`CollectionPeriodTooShort`); `council::check`. Applies from every line's next `write_off`.
- **Writes:** `collection_period_seconds`. **Events:** `CollectionPeriodSet`.

#### `set_cancel_grace(cancel_grace_seconds: u32, valid_until)` — **V C** (`ix/pool.rs:725`)

- **Accounts (5):** `council`, `multisig`, `pool` (mut). **Checks:** `ProposalExpired`; ≥ 30 d
  (`CancelGraceTooShort`); `council::check`. `cancel_pagare` reads the value in force at the
  cancellation.
- **Writes:** `cancel_grace_seconds`. **Events:** `CancelGraceSet`.

#### `propose_council(new_council: Pubkey, kind: u8, valid_until)` — **V** (`ix/pool.rs:752`)

- **Accounts (5):** `council` (signer); `multisig` (`address = pool.multisig` → `CouncilInvalid`);
  `pool` (mut, `has_one = council` → `Unauthorized`); + 2.
- **Checks:** `ProposalExpired`; `kind == COUNCIL_SQUADS` (`CouncilKindInvalid`); `council::check`
  on `multisig` (`CouncilInvalid`).
- **Writes:** `pending_council = new_council`, `pending_council_kind = kind`; a new proposal
  replaces a pending one and the zero key withdraws it.
- **Events:** `CouncilProposed`.

#### `accept_council(valid_until)` — **V**, check on the new multisig (`ix/pool.rs:783`)

- **Accounts (5):** `new_council` (signer); `multisig` (the new one); `pool` (mut); + 2.
- **Checks:** `ProposalExpired`; `pending_council` non-zero and equal to the signer
  (`NoPendingCouncil`); `pending_council_kind == COUNCIL_SQUADS` (`CouncilKindInvalid`); canonical
  vault 0 of `multisig` equals the signer (`CouncilInvalid`); `council::check` on `multisig`.
- **Writes:** `council`, `council_kind`, `multisig`, `council_vault_bump`; pending fields cleared.
- **Events:** `CouncilAccepted { old, new, kind }`.

#### `set_fx_feeder(fx_feeder: Pubkey, valid_until)` — **V C** (`ix/pool.rs:824`)

- **Accounts (5):** `council`, `multisig`, `pool` (mut). **Checks:** `ProposalExpired`;
  `council::check`.
- **Writes:** `fx_feeder` (zero for none). **Events:** `FxFeederSet`.

#### `post_fix(fix_rate: u64, fix_date: u32, valid_until)` — **V**, **C** on the council branch (`ix/pool.rs:855`)

See §6.

### 4.3 Lines

#### `request_line(line_id: u64, terms: LineTerms)` (`ix/lines.rs:66`)

- **Accounts (8):** `borrower` (signer); `payer` (signer, mut, pays the line's rent); `sponsor`
  (optional signer); `pool` (mut); `line` (`init` at `["line", pool, line_id]`, payer `payer`);
  `system_program`; + 2.
- **Args** (`LineTerms`, `terms.rs:14`): `principal` (MXN centavos), `rate_bps`, `late_bps`,
  `term_end`, `agreement_hash`, `sponsor: Option<Pubkey>`, `sponsor_role`,
  `schedule: Option<ScheduleParams>` (`first_due_at`, `interval_seconds`, `installments`,
  `max_missed`; `state.rs:416`), `expediente_root`, `pagare_template_hash`.
- **Checks, in order:** not paused (`Paused`); `line_id == pool.line_count` (`WrongAccount`);
  borrower ≠ council (`BorrowerIsCouncil`); `principal > 0` (`WrongAmount`); `rate_bps` and
  `late_bps` ≤ 10 000 (`WrongAmount`); `term_end > now` (`TermEnded`); non-zero
  `agreement_hash` (`AgreementHashRequired`), `expediente_root` (`ExpedienteRootRequired`),
  `pagare_template_hash` (`PagareTemplateHashRequired`); `sponsor_role <= 1`
  (`SponsorRoleInvalid`); a named sponsor signed (`SponsorRequired`); a schedule passes
  `validate_schedule` (`ix/lines.rs:289`): `1 <= installments <= 120` and `interval > 0`
  (`InvalidSchedule`), `max_missed >= 3` (`MaxMissedTooLow`), `max_missed <= installments` and
  `first_due_at > now` (`InvalidSchedule`), last due date ≤ `term_end` (`ScheduleEndsAfterTerm`),
  `principal + tranche_interest(principal, rate, last_due - now) >= installments`
  (`InvalidSchedule`).
- **Writes:** every `CreditLine` field: `status = Requested`, `currency = 484`, `requested_at =
  now`, `terms_hash = terms::terms_hash(pool, line_id, borrower, 484, terms)` (`terms.rs:34`),
  `payer`, the schedule fields (zero on a bullet line), everything else zero; `pool.line_count
  += 1`.
- **Events:** `LineRequested` (every term and the terms hash).

The terms hash is `sha256("elote_pool:terms:v1" ‖ pool ‖ line_id u64 ‖ borrower ‖ currency u16 ‖
principal u64 ‖ rate_bps u16 ‖ term_end i64 ‖ first_due_at i64 ‖ interval_seconds i64 ‖
installments u16 ‖ late_bps u16 ‖ max_missed u8 ‖ sponsor_present u8 ‖ sponsor (zero key when
none) ‖ sponsor_role u8 ‖ agreement_hash ‖ pagare_template_hash ‖ expediente_root)`, integers
little endian. `clients/js/src/terms.ts` reproduces it against a shared vector
(`terms.rs:65`, `terms.rs:87`).

#### `approve_line(expediente_root: [u8; 32], terms_hash: [u8; 32], valid_until)` — **V C** (`ix/lines.rs:188`)

- **Accounts (7):** `council`; `multisig`; `pool` (`has_one = council`); `line` (mut, `has_one =
  pool` → `WrongAccount`); `vault` (= `pool.vault`, for the NAV); + 2.
- **Checks, in order:** `ProposalExpired`; not paused (`Paused`); `council::check`; `status ==
  Requested` (`LineStatus`); `now <= requested_at + 15 d` (`RequestExpired`); both hashes equal the
  stored ones (`ExpedienteMismatch`); borrower not a multisig member (`BorrowerIsCurator`);
  `fix_rate != 0` (`FixStale`); `principal <= mxn_for_usdc(nav × max_line_bps / 10 000,
  fix_rate)` (`LineTooLarge`, `ix/lines.rs:204-206`); the window end (`first_due_at`, or
  `term_end` on a bullet line) is after now (`TermEnded`).
- **Writes:** `status = Approved`, `approved_at = now`, `availability_end`.
- **Events:** `LineApproved { expediente_root, terms_hash, availability_end }`.

#### `reject_line(valid_until)` — **V C** (`ix/lines.rs:248`)

- **Accounts (7):** `council`; `multisig`; `pool` (`has_one = council`); `line` (mut, `has_one =
  pool`, `has_one = payer` → `WrongAccount`, `status == Requested` → `LineStatus`, `close =
  payer`); `payer` (mut); + 2.
- **Checks:** `ProposalExpired`; `council::check`.
- **Writes:** the line account is closed; its rent goes to `payer`. The id stays taken.
- **Events:** `LineRejected`.

#### `withdraw_request()` (`ix/lines.rs:277`)

- **Accounts (5):** `borrower` (signer); `line` (mut, `has_one = borrower` → `Unauthorized`,
  `has_one = payer`, `status == Requested`, `close = payer`); `payer` (mut); + 2.
- Expired or not. **Writes:** the account is closed, rent to `payer`. **Events:**
  `RequestWithdrawn`.

#### `draw(amount_mxn, fx_rate, fx_date, asset_bump, pagare_bump, cert_fingerprint, signature_method)` (`ix/lines.rs:567`)

- **Accounts (15):** `borrower` (signer); `pool` (mut); `line` (mut, seeds, `has_one = pool` →
  `WrongAccount`, `has_one = borrower` → `Unauthorized`); `asset_mint`; `vault` (mut);
  `borrower_asset_account` (mut, mint and owner); `token_program`; `payer` (signer, mut, rent of
  the asset and the record); `asset` (mut, `["pagare_asset", line, line.draws]` with
  `asset_bump`); `pagare` (mut, `["pagare", line, line.draws]` with `pagare_bump`); `collection`
  (mut, = `pool.collection`); `mpl_core_program` (= Core id); `system_program`; + 2. With the bump
  given, each seeds constraint is one `create_program_address`, so the compute of a draw does not
  depend on the line's key.
- **Checks, in order (all before any write):** not paused; status `Approved` or `Active`
  (`LineStatus`); `0 < amount_mxn <= principal - drawn` (`WrongAmount`); `now <
  availability_end` (`AvailabilityEnded`); `fix_rate != 0` and the signed `fx_rate`, `fx_date`
  equal the pool's (`FixMismatch`); the FIX is at most 5 days old in Mexico City dates
  (`FixStale`); on a scheduled line `drawn + interest_due >= installments` after the tranche
  (`InvalidSchedule`); `draws < 64` (`TooManyDraws`); `usdc = usdc_for_mxn(amount_mxn, fx_rate)
  > 0` (`WrongAmount`); `usdc <= vault.amount` (`InsufficientLiquidity`); `outstanding + usdc <=
  max_outstanding_principal` (`OutstandingCapExceeded`); `signature_method <= 2` and a non-zero
  `cert_fingerprint` unless the method is 0 (`SignatureMethodInvalid`).
- **Derived on chain:** `pagare_hash = sha256(pagare_template_hash ‖ terms_hash ‖ line ‖
  draw_index u16 ‖ amount_mxn u64 ‖ now i64)` (`ix/lines.rs:539`); `delivery_hash =
  sha256(pagare_hash ‖ fx_rate u64 ‖ fx_date u32 ‖ usdc u64)` (`ix/lines.rs:553`); the tranche's
  interest `tranche_interest(amount_mxn, rate_bps, last_due - now)` (`math.rs:33`).
- **CPIs:** `transfer_checked` `usdc` vault → borrower (pool signs); any lamports already on the
  asset and record addresses are moved to `payer` (signed by each PDA), because both creations
  require an unfunded address (`ix/lines.rs:496`); Core `CreateV2` of the asset (§8.1), pool PDA
  as authority and owner, asset PDA signing for its address; `create_account` of the record.
- **Writes:** `pool.outstanding_principal += usdc`; `line.drawn += amount_mxn`, `usdc_drawn +=
  usdc`, `draw_time_weight += amount_mxn × now`, `draws += 1`, `interest_due +=` the tranche's
  interest; on the first tranche `status = Active`, `drawn_at = now`; the `Pagare` record with the
  issuance entry (`entry_count = 1`, `last_kind = Emision`). `Pool::check` on the reloaded vault.
- **Events:** `LineDrawn`, `PagareIssued`, `PagareEntry` (Emision).

#### `repay(max_usdc: u64, fx_rate: u64, fx_date: u32, receipt_hash: [u8; 32])` (`ix/lines.rs:785`)

- **Accounts (10 + remaining):** `payer` (signer); `pool` (mut); `line` (mut, seeds, `has_one =
  pool`); `asset_mint`; `share_mint` (for the supply); `vault` (mut); `payer_asset_account` (mut,
  mint and owner); `token_program`; + 2. **Remaining:** exactly the records
  `[pagares_paid ..= last_touched]`, writable, oldest first, where `last_touched` is the first
  record whose `drawn_through` exceeds `principal_repaid` after the payment, or the last draw;
  none once the cursor has passed the last draw. Each is loaded as a program-owned `Pagare` with
  its discriminator and checked for this line, the expected index and `PAGARE_OUTSTANDING`; an
  uncovered record must end the set and a covered one may end it only if it is the last draw
  (`PagareAccountsMismatch`, `ix/lines.rs:828-854`). `clients/js/src/pagare.ts` computes the set.
- **Checks and arithmetic, in order:** status `Active` or `InCollection` (`LineStatus`); `fix_rate
  != 0` and the signed FIX equals the pool's, whatever its age (`FixMismatch`); `amount =
  mxn_for_usdc(max_usdc, fx_rate)`; `remaining = (drawn - principal_repaid) + (interest_due -
  interest_repaid)`; `amount > 0` (`WrongAmount`); `late_part = line_arrears(...).late_due` on a
  scheduled line or an accelerated bullet line, else 0; `amount >= late_part`
  (`LateInterestUnpaid`); while the line can still draw (`Active`, `now < availability_end`,
  something undrawn), a payment with nothing due is refused (`WrongAmount`); `rest = min(amount -
  late_part, remaining)`, `interest_part = min(rest, interest_left)`, `principal_part = rest -
  interest_part`; `usdc_in = usdc_ceil_for_mxn(late + interest + principal, fx_rate)`, never more
  than `max_usdc`; `usdc_principal = usdc_in - usdc_for_mxn(late + interest, fx_rate)`; the
  remaining accounts; `cost` = Σ over the records of `cost_of(usdc_delivered, amount_mxn, applied
  after) - cost_of(…, applied before)`; `delta = usdc_principal - cost`.
- **CPIs:** one `transfer_checked` payer → vault of `usdc_in`, when non-zero.
- **Writes:** `pool.outstanding_principal -= cost`; `pool.interest_collected +=
  usdc_for_mxn(interest_part)`; `pool.fx_realized += delta`; `line.principal_repaid`,
  `interest_repaid`, `usdc_cost_repaid += cost`; when `late_part > 0`, `late_collected +=
  late_part` and `late_paid_until = now`; `status = Repaid`, `closed_at = now` when nothing
  remains and the line cannot draw (`ix/lines.rs:906`). Per record, oldest first, the principal
  applied to its range: zero → no entry; covered (`drawn_through <= principal_repaid` after) →
  `Pagado` entry, `status = PAGARE_PAID_PENDING_CANCEL`, `paid_at = now`, `receipt_hash`, cursor
  past it; reached but not covered → `Abono` entry. `Pool::check` with the supply.
- **Events, in order:** `PagareEntry` per entry; `PagarePayment` when records were passed;
  `LateInterestCollected` when `late_part > 0`; `FxRealized` when `principal_part > 0`;
  `LineRepaid`; `NavUpdated`. A payment on a zero debt of a line that cannot draw is the closing
  repayment: `LineRepaid` with zero parts, `usdc_received = 0` and `remaining = 0`.

#### `mark_in_collection()` (`ix/lines.rs:1042`)

- **Accounts (4):** `pool`; `line` (mut, seeds, `has_one = pool`); + 2. No signer beyond the fee
  payer.
- **Checks:** scheduled (`NotScheduled`); `status == Active` (`LineStatus`); with `a =
  line_arrears(now)` and `k = a.covered + max_missed`, `k <= installments` and `now > anchor + k ×
  interval + grace` (`NotEnoughMissed`).
- **Writes:** `status = InCollection`, `accelerated_at = now`, `interest_due = kept` with `kept`
  from `accelerated_balance(line, max(a.due, k), now)` (`ix/lines.rs:1287`, §7.4);
  `acceleration_due_at = 0`.
- **Events:** `LineInCollection { missed, remaining, interest_forgiven }`; `AccelerationWithdrawn`
  when it superseded a pending council proposal.

#### `write_off(valid_until)` — **V C** (`ix/lines.rs:1122`)

- **Accounts (8):** `council`; `multisig`; `pool` (mut, `has_one = council`); `line` (mut, seeds,
  `has_one = pool`); `share_mint`; `vault`; + 2.
- **Checks:** `ProposalExpired`; `council::check`; with `after_term = now > term_end + grace`:
  `Active` requires `after_term` (`TermNotEnded`); `InCollection` requires (`after_term`, or on a
  scheduled line `now > first_missed_due + grace`, or a bullet line) and `now > accelerated_at +
  collection_period_seconds` (`WriteOffTooEarly`); any other status `LineStatus`.
- **Writes:** `loss = drawn - principal_repaid` (MXN), `loss_usdc = usdc_drawn -
  usdc_cost_repaid`; `pool.outstanding_principal -= loss_usdc`; `pool.written_off += loss_usdc`;
  `status = WrittenOff`, `closed_at = now`. No CPI.
- **Events:** `LineWrittenOff { loss, loss_usdc, nav_after }`, `NavUpdated`.

#### `suspend_draws(valid_until)` — **V C** (`ix/lines.rs:1223`)

- **Accounts (6):** `CouncilLine`. **Checks:** `ProposalExpired`; `council::check`; status
  `Approved` or `Active` (`LineStatus`); `now < availability_end` (`WindowClosed`).
- **Writes:** `availability_end_saved = availability_end`, `availability_end = now`.
- **Events:** `LineDrawsSuspended { available_forfeited = principal - drawn }`.

#### `resume_draws(valid_until)` — **V C** (`ix/lines.rs:1238`)

- **Accounts (6):** `CouncilLine`. **Checks:** `ProposalExpired`; `council::check`; `Approved` or
  `Active` (`LineStatus`); suspended (`DrawsNotSuspended`); no acceleration pending
  (`AccelerationState`).
- **Writes:** `availability_end = availability_end_saved`, `availability_end_saved = 0`.
- **Events:** `LineDrawsResumed { availability_end }`.

#### `propose_acceleration(valid_until)` — **V C** (`ix/lines.rs:1256`)

- **Accounts (6):** `CouncilLine`. **Checks:** `ProposalExpired`; `council::check`; `Active` and
  `drawn > 0` (`LineStatus`); none pending (`AccelerationState`).
- **Writes:** `acceleration_due_at = now + acceleration_notice_seconds`; an open draw window is
  suspended as in `suspend_draws` (a closed one is left alone).
- **Events:** `AccelerationProposed { due_at }`.

#### `withdraw_acceleration(valid_until)` — **V C** (`ix/lines.rs:1273`)

- **Accounts (6):** `CouncilLine`. **Checks:** `ProposalExpired`; `council::check`; `Active` with a
  pending proposal (`AccelerationState`).
- **Writes:** `acceleration_due_at = 0`. Draws stay suspended until `resume_draws`.
- **Events:** `AccelerationWithdrawn`.

#### `accelerate_line(valid_until)` — **V C** (`ix/lines.rs:1309`)

- **Accounts (6):** `CouncilLine`. **Checks:** `ProposalExpired`; `council::check`; `Active` with a
  pending proposal (`AccelerationState`); `now >= acceleration_due_at` (`AccelerationNotDue`).
- **Writes:** `status = InCollection`, `accelerated_at = now`, `interest_due = kept`
  (`accelerated_balance` with `due_count` = installments due now on a scheduled line, 0 on a bullet
  line). `acceleration_due_at` stays set: together with `accelerated_at` it records the council's
  route.
- **Events:** `LineAccelerated { remaining, interest_forgiven }`.

#### `cancel_line(valid_until)` — **V C** (`ix/lines.rs:1333`)

- **Accounts (6):** `CouncilLine`. **Checks:** `ProposalExpired`; `council::check`; `Approved` and
  `drawn == 0` (`LineStatus`).
- **Writes:** `status = Cancelled`, `closed_at = now`. **Events:** `LineCancelled`.

### 4.4 Pagarés

The five instructions take `pagare` at `["pagare", line, draw_index]` with its stored bump and
`has_one = line` (`WrongAccount`), and `line` with its seeds and `has_one = pool`. The ones that
touch the asset take `asset` (= `pagare.asset`), `collection` (= `pool.collection`),
`mpl_core_program` and `system_program`. Every one appends one registry entry (§8.3) and emits its
`PagareEntry`. None is blocked by the pause.

#### `cancel_pagare(draw_index: u16, receipt_hash: [u8; 32], valid_until)` — **V**, **C** on the council branch (`ix/pagare.rs:90`)

- **Accounts (13):** `holder` (signer); `multisig`; `payer` (signer, mut, receives the asset's
  rent); `pool`; `line`; `pagare` (mut); `asset` (mut); `collection` (mut);
  `procurador_authority` (optional); `mpl_core_program`; `system_program`; + 2.
- **Branch** (§8.4): the council if `holder == pool.council` (`council::check`); else the borrower
  if `holder == line.borrower`; else the procurador (`holder == pagare.procurador`, `NotHolder`;
  procuración in force, `ProcuracionState`).
- **Checks, in order:** `ProposalExpired`; the branch; not burnt (`PagareAlreadyPaid`); non-zero
  `receipt_hash` (`DocumentHashRequired`); `status == PAGARE_PAID_PENDING_CANCEL`
  (`PagareNotCovered`); borrower branch: `now > paid_at + cancel_grace_seconds`
  (`CancelGraceNotElapsed`); procurador branch: `procurador_authority` is
  `["procurador", pagare, holder]` (`WrongAccount`).
- **Writes:** the `Terminacion` entry (from the pool on the council branch, from the signer
  otherwise, to the pool, over `receipt_hash`); `status = PAGARE_PAID`; `cancel_receipt_hash`;
  flags bit 1 and `procurador` cleared (bit 0 stays). `paid_at` and `receipt_hash` stay.
- **Events:** `PagareEntry`, `PagareCancelled { holder, by_borrower }`.
- **CPIs, last:** Core `UpdatePluginV1` thawing the `PermanentFreezeDelegate` (pool PDA as the
  collection's authority); `BurnV1` with the pool PDA as owner, or on the procurador branch with
  `procurador_authority` signed by the program. Core leaves a one-byte tombstone and sends the rest
  of the rent to `payer`.

#### `endorse_pagare(draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32], valid_until)` — **V C** (`ix/pagare.rs:251`)

- **Accounts (12):** `holder` (signer, `== pool.council` → `NotHolder`); `multisig`; `payer`
  (signer, mut); `pool`; `line`; `pagare` (mut); `asset` (mut); `collection` (mut);
  `mpl_core_program`; `system_program`; + 2.
- **Checks, in order:** `ProposalExpired`; `kind == KIND_PROCURACION` (`EndosoKindInvalid`);
  `council::check`; `to` is not the zero key, the pool, the council, the guardian, the borrower or
  a multisig member (`EndorseeInvalid`); non-zero `endoso_hash` (`DocumentHashRequired`); line not
  `WrittenOff` (`LineStatus`); `status == PAGARE_OUTSTANDING` (`PagareAlreadyPaid`); no procuración
  in force (`ProcuracionState`).
- **Writes:** the `Procuracion` entry (pool → `to`, over `endoso_hash`); flags bits 0 and 1;
  `procurador = to`.
- **CPI:** Core `AddPluginV1` of an owner-managed `BurnDelegate` with authority
  `Address(["procurador", pagare, to])`, the pool PDA signing as owner; `ApprovePluginAuthorityV1`
  to that address instead when flags bit 0 says the plugin exists.
- **Events:** `PagareEntry`.

#### `revoke_procuracion(draw_index: u16, entry_hash: [u8; 32], valid_until)` — **V C** (`ix/pagare.rs:385`)

- **Accounts (12):** as `endorse_pagare`.
- **Checks:** `ProposalExpired`; `council::check`; non-zero `entry_hash` (`DocumentHashRequired`);
  a procuración in force (`ProcuracionState`).
- **Writes:** the `Revocacion` entry (pool → recorded procurador, over `entry_hash`); flags bit 1
  and `procurador` cleared.
- **CPI:** Core `RevokePluginAuthorityV1` on the `BurnDelegate` (authority back to `Owner`; the
  plugin stays). **Events:** `PagareEntry`.

#### `court_annotation(draw_index: u16, kind: u8, document_hash: [u8; 32], valid_until)` — **V C** (`ix/pagare.rs:467`)

- **Accounts (7):** `holder` (`== pool.council` → `NotHolder`); `multisig`; `pool`; `line`;
  `pagare` (mut); + 2. No Core account.
- **Checks:** `ProposalExpired`; `council::check`; `kind` is `KIND_ANOTACION_JUDICIAL` (6) or
  `KIND_CONSTANCIA_JUDICIAL` (7) (`AnnotationKindInvalid`); non-zero `document_hash`
  (`DocumentHashRequired`); not burnt (`PagareAlreadyPaid`). Any line status, beside a procuración.
- **Writes:** the entry (pool → pool, over `document_hash`). **Events:** `PagareEntry`.

#### `annotate_payment(draw_index: u16, entry_count: u32, receipt_hash: [u8; 32], valid_until)` — **V C** (`ix/pagare.rs:534`)

- **Accounts (7):** `annotator` (`== pool.council` → `Unauthorized`); `multisig`; `pool`; `line`;
  `pagare` (mut); + 2.
- **Checks:** `ProposalExpired`; `council::check`; non-zero `receipt_hash`
  (`DocumentHashRequired`); `1 <= entry_count <= pagare.entry_count` (`EntryNotFound`). Any status,
  burnt included.
- **Writes:** the `Recibo` entry (annotator → pool, over `receipt_hash`); while `status ==
  PAGARE_PAID_PENDING_CANCEL`, `pagare.receipt_hash = receipt_hash`.
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
            repay: nothing due and nothing drawable ◄─────────────────────────────────┘ │ │
                      │                                                                 │ │
                      ▼          write_off (C), now > term_end + grace ◄────────────────┘ │
                   Repaid                     │                                           │
                      ▲                       ▼                                           │
                      │                  WrittenOff ◄── write_off (C) ──┐                 │
                      │                                                 │                 │
                      └──────── repay (last unit) ─────────────── InCollection ◄──────────┘
                                                                        mark_in_collection (anyone,
                                                                        scheduled) or
                                                                        accelerate_line (C)
```

- `Repaid`, `WrittenOff` and `Cancelled` are terminal; `reject_line` and `withdraw_request` close
  the account. A `Requested` line past its 15-day window stays `Requested` until withdrawn or
  rejected; `approve_line` refuses it.
- `Active → Repaid` needs nothing due and nothing drawable (window closed, fully drawn, or in
  collection). While the line can draw, a payment that clears the debt leaves it `Active`.
- After the mark there is no return to `Active`: only full repayment or the write-off.

**Draw window** (sub-state of `Approved` and `Active`, fields `availability_end`,
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
                draw                      repay covers it (Pagado)                    cancel_pagare
   (none) ──────────────► OUTSTANDING (0) ─────────────────────────► PAID_PENDING_CANCEL (2) ──────────► PAID (1)
                              │  ▲                                     │                                   asset burnt,
       endorse_pagare (C)     │  │ revoke_procuracion (C)              │ court_annotation (C)              record stays
                              ▼  │                                     │ annotate_payment (C)
                     OUTSTANDING + procuración (flags bit 1)           │   (replaces receipt_hash)
                              │                                        │
                              └── repay covers it ──► PAID_PENDING_CANCEL + procuración
                                                       (cancel_pagare by the procurador clears it)

   Abono entries: a repay that reaches an OUTSTANDING pagaré without covering it. Status unchanged.
   court_annotation: any status but PAID. annotate_payment: any status.
```

The status never reverts. `cancel_pagare` requires `PAID_PENDING_CANCEL` whatever the line's
status, so a covered pagaré cancels on a line in collection or written off as well.

## 6. The FIX (`fx.rs`, `post_fix`)

The FIX is Banxico's USD/MXN rate, stored as `Pool.fix_rate` = MXN per USD × 10⁴ (18.3740 is
183 740) and `Pool.fix_date` = the date Banxico determined it, `yyyymmdd`.

### 6.1 `post_fix(fix_rate, fix_date, valid_until)` (`ix/pool.rs:855`)

- **Accounts (5):** `poster` (signer); `multisig` (= `pool.multisig`); `pool` (mut); + 2.
- **Checks, in order:** `ProposalExpired`; `fix_rate > 0` (`WrongAmount`); `fix_date` is a real
  date in 2000–2199 and not after today in Mexico City (`FixDateInvalid`; `fx::fix_day`,
  `fx.rs:34`; `fx::mexico_city_day`, UTC−6, `fx.rs:49`). Then:
  - **council** (`poster == pool.council`): `council::check`; `fix_date >= pool.fix_date`
    (`FixDateInvalid`). The council publishes the first FIX, moves of any size and same-day
    corrections.
  - **feeder**: `poster == pool.fx_feeder`, non-zero (`Unauthorized`); once per Mexico City day,
    `mexico_city_day(now) > mexico_city_day(pool.fix_posted_at)` (`FixAlreadyPostedToday`); `fix_date > pool.fix_date`
    (`FixDateInvalid`); a FIX already published and the new one within the band (`FixJump`).
- **Writes:** `fix_rate`, `fix_date`, `fix_posted_at = now`.
- **Events:** `FixPosted { fix_rate, fix_date, previous_rate, by, slot, posted_at }`.

**Band** (`fx::within_band`): `|new − old| × 10 000 <= old × 500 × days`, with `days` the calendar days between the two FIX dates clamped to `1..=3`: 5% a day either way, 15% at most.

### 6.2 Where the FIX is used

| instruction | rule |
|---|---|
| `approve_line` | the cap `nav × max_line_bps / 10 000` (USDC) is converted to MXN with `mxn_for_usdc` at the FIX in force; no FIX → `FixStale` |
| `draw` | the borrower signs `fx_rate` and `fx_date`; both must equal the pool's (`FixMismatch`); `mexico_city_day(now) − fix_day(fix_date) <= MAX_FIX_AGE_DAYS` (5) (`FixStale`) |
| `repay` | the payer signs `fx_rate` and `fx_date`; both must equal the pool's (`FixMismatch`); no age limit |

### 6.3 Conversions and rounding

With `MXN_USDC = 10⁸` (centavos per peso × USDC base units per dollar ÷ `FIX_SCALE`, `fx.rs:12`):

| function | formula | rounding | used for |
|---|---|---|---|
| `usdc_for_mxn(mxn, fix)` (`fx.rs:54`) | `mxn × 10⁸ / fix` | down | USDC a draw delivers; USDC value of interest charges |
| `mxn_for_usdc(usdc, fix)` (`fx.rs:59`) | `usdc × fix / 10⁸` | down | MXN credited for a repayment's `max_usdc`; the line cap at approval |
| `usdc_ceil_for_mxn(mxn, fix)` (`fx.rs:65`) | `⌈mxn × 10⁸ / fix⌉` | up | USDC a repayment takes for the MXN applied |
| `cost_of(delivered, amount_mxn, applied)` (`fx.rs:72`) | `delivered × min(applied, amount_mxn) / amount_mxn` | down | USDC cost of the principal applied to one pagaré; the whole pagaré costs exactly `usdc_delivered` |

Direction: the pool delivers rounded down and takes rounded up, and credits MXN rounded down, so
a repayment never credits more MXN than the USDC it received buys at the FIX. In `repay` the late
and ordinary interest are valued rounded down and the principal takes the rounding remainder
(`usdc_principal = usdc_in − usdc_for_mxn(charges)`). The cost of a repayment's principal is the
difference of two `cost_of` values per record, so partial payments of one pagaré add up to its
delivery exactly.

## 7. Calendar, arrears and acceleration (`math.rs`, `line_arrears`)

A scheduled line stores its terms and its repayments; arrears are recomputed on every call by
`math::arrears(terms, paid, now)` (`math.rs:218`) through `line_arrears` (`ix/lines.rs:990`).
Nothing about arrears is stored.

### 7.1 Schedule

- **Anchor** = `first_due_at − interval`. Installment `k` (1-based) is due at `anchor + k ×
  interval`, and counts as due at that very second. `installments_due(now)` = `min(installments,
  (now − anchor) / interval)`, zero before the first due date (`math.rs:60`).
- **Total** = `drawn + interest_due` (MXN): what was drawn plus the interest fixed at each draw.
  Each tranche's interest runs from its draw to `last_due()`.
- **Installment** = `total / installments` rounded down; the last carries the remainder
  (`installment_amount`, `cumulative_due`, `math.rs:75-86`).
- **Covered** = `paid / installment`, capped at `installments − 1` unless `paid >= total`, with
  `paid = principal_repaid + interest_repaid` (late interest never pays an installment)
  (`math.rs:90`).
- **Missed** = `due − covered` when positive. The grace period delays late interest, collection
  and the write-off; it does not change the count.

### 7.2 Late interest (moratorio)

Late interest accrues at `late_bps` a year (365 days) on the **principal share** of each overdue
part, `p_k = part_k × drawn / total` rounded down (no interest on overdue interest), from
`start_k = max(due_k + grace, late_paid_until)` to `now`:

```
late_due = Σ_k p_k × min(now − start_k, cap) × late_bps / (10 000 × SECONDS_PER_YEAR)
cap      = ⌈10 000 × SECONDS_PER_YEAR / late_bps⌉
```

The weighted sum is accumulated in `u128` and divided once (`math.rs:127`). Caps: per settlement
`late_due <= overdue_principal`; over the life of the line `late_due <= drawn − late_collected`.
`repay` collects it first and moves `late_paid_until` only when `late_part > 0`.

### 7.3 Bullet lines

A bullet line (`installments == 0`) has one maturity, `term_end`, and pays interest and principal
in any order up to it. It accrues no late interest unless accelerated: after `accelerate_line`,
`line_arrears` treats it as one installment due at `term_end` (anchor `term_end − 1`, interval 1),
accelerated at the mark, so late interest at `late_bps` runs on its principal from
`min(term_end + grace, accelerated_at)` (`ix/lines.rs:991-996`). `mark_in_collection` refuses a
bullet line (`NotScheduled`); `write_off` takes an `Active` bullet line after `term_end + grace`.

### 7.4 Acceleration

Two routes set `accelerated_at` and move the line to `InCollection`:

| route | instruction | trigger | interest kept |
|---|---|---|---|
| objective | `mark_in_collection` (anyone) | scheduled; `covered + max_missed <= installments` and `now > anchor + (covered + max_missed) × interval + grace` | `interest_due × due_count / installments`, `due_count = max(due, covered + max_missed)` |
| council | `propose_acceleration` → notice → `accelerate_line` | `Active`, `drawn > 0`; `now >= acceleration_due_at` | scheduled: `interest_due × due / installments` with `due` the installments due now; bullet: `bullet_accrued_interest(drawn, draw_time_weight, rate, now)` = `rate × (drawn × now − Σ amount_k × drawn_at_k) / (10 000 × SECONDS_PER_YEAR)`, at most `interest_due` (`math.rs:45`) |

In both, `kept` is at least `interest_repaid`, the rest is forgiven (`accelerated_balance`,
`ix/lines.rs:1287`; `math::accelerated_interest`, `math.rs:182`), and the NAV does not move.

After the mark, `arrears` counts every installment as due, so `overdue` is the whole unpaid
balance. Installment `k` is exigible at `min(due_k, accelerated_at)` and accrues late interest from
`max(min(due_k + grace, accelerated_at), late_paid_until)` (`math.rs:243-244`): one already overdue
keeps its own start, one not yet due starts at the mark. `first_missed_due` follows the exigible
instant. The loop runs over every remaining installment, at most 120.

### 7.5 What reads the arrears

| reader | field |
|---|---|
| `repay` | `late_due` |
| `mark_in_collection` | `covered`, `due`, `missed` |
| `accelerate_line` | `due` |
| `write_off` | `first_missed_due` |

`clients/js/src/schedule.ts` mirrors the arithmetic (`lineArrears`, `lineLateDue`,
`bulletAccruedInterest`, `maxLinePrincipal`); `tests/vectors.rs` and
`clients/js/test/schedule.test.ts` replay the same 427 vectors (50 accelerated) against both.

## 8. The pagaré token

### 8.1 The Metaplex Core asset

`draw` creates the asset with `CreateV2` (`ix/lines.rs:439`):

- **owner** the pool PDA; **update authority** the series' collection (whose own update authority
  is the pool PDA); **name** `PAGARE <line_id>-<draw_index>`; **URI** empty.
- **plugins at creation:**
  - `PermanentFreezeDelegate { frozen: true }`, authority `UpdateAuthority` (the collection, so the
    pool PDA): the asset cannot move; only the program thaws it, in `cancel_pagare`;
  - `ImmutableMetadata`, authority `None`;
  - `Attributes`, authority `None`, eight facts: `pagare_hash` (lower-case hex), `amount_mxn`,
    `currency` = `MXN`, `issued_at`, `due_at`, `line` (the line id), `draw_index`,
    `signature_method` (decimals). The certificate commitment stays in the record only;
  - `AddBlocker`, authority `None`: no authority-managed plugin can be added later.
- **delegates:** none at creation. The only plugin added later is the owner-managed
  `BurnDelegate` of a procuración, whose authority is the program address
  `["procurador", pagare, procurador]` (`endorse_pagare`). `init_pool` admits only a bare
  collection, so no collection-level plugin reaches the asset.

The program does not read the asset back: later instructions take `address = pagare.asset` and
rely on Core's own checks in the CPIs. `tests/core_capabilities.rs` pins what Core 0.15.3 allows
after creation (permanent-delegate hand-off and `None` as terminal, who can thaw, the update
authority bound to the collection, `AddBlocker`, owner-managed delegates and a burn delegate held
by a program address).

### 8.2 The record

The `Pagare` account (§3.3) is created by hand in `draw` (`create_sidecar`, `ix/lines.rs:513`)
so its bump is the client's and the address is verified with one `create_program_address`. It
records the promise in MXN (`amount_mxn`, `pagare_hash`) apart from the delivery in USDC
(`fx_rate`, `fx_date`, `usdc_delivered`, `delivery_hash`). `drawn_through` is the line's `drawn`
after the tranche: pagarés are covered oldest first, so pagaré `k` is covered once
`principal_repaid >= drawn_through_k`.

### 8.3 The registry: a hash chain in the record (`registry.rs`)

```
entry_hash = sha256(asset ‖ kind ‖ from ‖ to ‖ hash ‖ stated_at ‖ entry_count)
             32 B     1 B    32 B   32 B  32 B   i64 LE      u32 LE         (141 bytes)
log_root   = sha256(log_root ‖ entry_hash)            starting from 32 zero bytes
```

`registry::append` (`registry.rs:19`) is the only writer: it increments `entry_count` (the entry's
own position, 1 for the issuance; `Overflow` if it would wrap), updates `log_root` and `last_kind`.
The full entry goes in `PagareEntry { pool, line_id, draw_index, asset, kind, from, to,
entry_hash, entry_count, recorded_at, stated_at }`, so anyone can replay the chain from the
events and compare it with `log_root`. Every entry the program writes has `stated_at =
recorded_at = now`.

| kind | code | written by | from | to | hash |
|---|---|---|---|---|---|
| `Emision` | 0 | `draw` | pool | pool | `pagare_hash` |
| `Procuracion` | 2 | `endorse_pagare` | pool | procurador | `endoso_hash` |
| `Revocacion` | 3 | `revoke_procuracion` | pool | recorded procurador | `entry_hash` (the signed revocation) |
| `Terminacion` | 4 | `cancel_pagare` | pool (council branch), procurador or borrower | pool | `receipt_hash` |
| `Abono` | 5 | `repay` (principal reaches, does not cover) | payer | pool | `receipt_hash` |
| `AnotacionJudicial` | 6 | `court_annotation` | pool | pool | `document_hash` |
| `ConstanciaJudicial` | 7 | `court_annotation` | pool | pool | `document_hash` |
| `Pagado` | 9 | `repay` (covers it) | payer | pool | `receipt_hash` |
| `Recibo` | 11 | `annotate_payment` | council | pool | `receipt_hash` |

Codes 1 and 8 are reserved for the endoso en propiedad and the late endoso; 10 was the holder's
cancellation request and is retired. None is reused (`state.rs:341-352`). A payment writes an
entry on a pagaré only if it applies principal to it; an interest-only payment emits
`PagarePayment` with no entries.

### 8.4 Procuración and cancellation

**Procuración** (endoso en procuración, the only endoso the pool makes): `endorse_pagare` lends
the power to extinguish the pagaré through the owner-managed `BurnDelegate` whose authority is
`["procurador", pagare, to]`. Nobody holds a key for that address, so the burn happens only inside
`cancel_pagare`, where the program signs for it. The asset stays owned by the pool.
`revoke_procuracion` returns the delegate's authority to the owner; the plugin stays (flags bit
0), so the next procuración approves it instead of adding it.

**Cancellation, three ways** (`cancel_pagare`), all on a pagaré in `PAGARE_PAID_PENDING_CANCEL`
and with a non-zero receipt hash:

| branch | signer | extra condition | burn authority | `Terminacion.from` |
|---|---|---|---|---|
| council | `pool.council` | `council::check` | pool PDA (owner) | pool |
| procurador | `pagare.procurador` | procuración in force; `procurador_authority` = `["procurador", pagare, signer]` | procurador PDA, program-signed | procurador |
| borrower | `line.borrower` | `now > paid_at + pool.cancel_grace_seconds` (≥ 30 d) | pool PDA (owner) | borrower |

`PagareCancelled.by_borrower` marks the third branch. Every branch ends the procuración in force
with the title and leaves the record as the PAGADO mark.

### 8.5 Annotations

- **Court** (`court_annotation`): an `AnotacionJudicial` (embargo, suspension, cancellation) or
  `ConstanciaJudicial` over the court document's hash, on any line status and beside a
  procuración, while the title exists.
- **Receipt** (`annotate_payment`): a `Recibo` entry attached to payment entry `entry_count` of the
  chain. While the pagaré awaits its burn it also replaces the payer's provisional
  `Pagare.receipt_hash`. The chain stores no per-entry kinds, so the engine checks that the
  referenced entry is a payment entry.

## 9. Events and errors

### 9.1 Events (37, `events.rs`, all via `emit_cpi!`)

| event | `events.rs` | emitted by | fields |
|---|---|---|---|
| `PoolInitialized` | 8 | `init_pool` | pool, council, creator, multisig, guardian, asset_mint, share_mint, collection, grace_period_seconds, collection_period_seconds, max_line_bps, max_outstanding_principal, acceleration_notice_seconds, cancel_grace_seconds, asset_controls, permissioned |
| `CollectionPeriodSet` | 30 | `set_collection_period` | pool, collection_period_seconds |
| `NavUpdated` | 38 | `deposit`, `redeem`, `repay`, `write_off` | pool, total_assets, share_supply |
| `Deposited` | 45 | `deposit` | pool, depositor, assets, shares, nav_after, supply_after |
| `Redeemed` | 55 | `redeem` | pool, holder, shares, assets, nav_after, supply_after |
| `PagareIssued` | 67 | `draw` | pool, line_id, draw_index, asset, pagare_hash, amount_mxn, delivery_hash, due_at, holder |
| `PagareEntry` | 83 | `draw`, `repay`, `cancel_pagare`, `endorse_pagare`, `revoke_procuracion`, `court_annotation`, `annotate_payment` | pool, line_id, draw_index, asset, kind, from, to, entry_hash, entry_count, recorded_at, stated_at |
| `PagareCancelled` | 100 | `cancel_pagare` | pool, line_id, draw_index, asset, receipt_hash, holder, by_borrower |
| `LineDrawsSuspended` | 113 | `suspend_draws` | pool, line_id, available_forfeited |
| `LineCancelled` | 120 | `cancel_line` | pool, line_id |
| `LineDrawn` | 128 | `draw` | pool, line_id, draw_index, amount_mxn, fx_rate, fx_date, usdc_delivered, drawn, available, interest_due, availability_end, outstanding_after |
| `LineRepaid` | 147 | `repay` | pool, line_id, payer, receipt_hash, usdc_received, fx_rate, principal_part, interest_part, remaining, nav_after |
| `LateInterestCollected` | 163 | `repay` | pool, line_id, amount |
| `LineInCollection` | 174 | `mark_in_collection` | pool, line_id, missed, remaining, interest_forgiven |
| `LineWrittenOff` | 184 | `write_off` | pool, line_id, loss, loss_usdc, nav_after |
| `PoolPaused` | 193 | `pause` | pool, by |
| `PagarePayment` | 213 | `repay` | pool, line_id, payer, receipt_hash, late_part, interest_part, principal_part, entries: `Vec<PaymentEntry { draw_index, kind, entry_count, principal_applied }>` |
| `PagareReceiptAnnotated` | 227 | `annotate_payment` | pool, line_id, draw_index, entry_count, payment_entry_count, receipt_hash |
| `LineRequested` | 239 | `request_line` | pool, line_id, borrower, payer, sponsor, sponsor_role, agreement_hash, expediente_root, pagare_template_hash, terms_hash, currency, principal, rate_bps, term_end, first_due_at, interval_seconds, installments, late_bps, max_missed |
| `LineApproved` | 263 | `approve_line` | pool, line_id, expediente_root, terms_hash, availability_end |
| `LineRejected` | 272 | `reject_line` | pool, line_id |
| `RequestWithdrawn` | 278 | `withdraw_request` | pool, line_id |
| `AccelerationProposed` | 286 | `propose_acceleration` | pool, line_id, due_at |
| `AccelerationWithdrawn` | 294 | `withdraw_acceleration`, `mark_in_collection` (superseding) | pool, line_id |
| `LineAccelerated` | 302 | `accelerate_line` | pool, line_id, remaining, interest_forgiven |
| `LineDrawsResumed` | 311 | `resume_draws` | pool, line_id, availability_end |
| `CouncilProposed` | 319 | `propose_council` | pool, council, kind |
| `CouncilAccepted` | 326 | `accept_council` | pool, old, new, kind |
| `PoolResumed` | 334 | `resume` | pool |
| `GuardianSet` | 340 | `set_guardian` | pool, guardian |
| `MaxLineBpsSet` | 346 | `set_max_line_bps` | pool, max_line_bps |
| `OutstandingCapSet` | 352 | `set_outstanding_cap` | pool, max_outstanding_principal |
| `AccelerationNoticeSet` | 358 | `set_acceleration_notice` | pool, acceleration_notice_seconds |
| `CancelGraceSet` | 364 | `set_cancel_grace` | pool, cancel_grace_seconds |
| `FixPosted` | 372 | `post_fix` | pool, fix_rate, fix_date, previous_rate, by, slot, posted_at |
| `FxFeederSet` | 384 | `set_fx_feeder` | pool, fx_feeder |
| `FxRealized` | 393 | `repay` (principal part > 0) | pool, line_id, receipt_hash, cost, received, delta, nav_after |

Order within one instruction: `draw` emits `LineDrawn`, `PagareIssued`, `PagareEntry`; `repay`
emits the `PagareEntry`s, `PagarePayment`, `LateInterestCollected`, `FxRealized`, `LineRepaid`,
`NavUpdated`; `cancel_pagare` emits `PagareEntry` then `PagareCancelled`, both before the burn.

### 9.2 Errors (66, `errors.rs`, numbered from 6000 in declaration order)

Squads v4 also numbers its errors from 6000. Decode an error by the program that raised it (the
invoke stack in the logs), not by the number alone; `scripts/lib/cli.ts` does this.

| code | name | group | message | source |
|---|---|---|---|---|
| 6000 | `Unauthorized` | Signers | This signer may not do this | `errors.rs:9` |
| 6001 | `SponsorRequired` | Signers | The sponsor named on this line must sign | `errors.rs:11` |
| 6002 | `NotHolder` | Signers | The pagaré's holder must sign | `errors.rs:13` |
| 6003 | `Paused` | Signers | The pool is paused | `errors.rs:15` |
| 6004 | `CouncilInvalid` | The council | The council is not the vault of an autonomous Squads v4 multisig with a majority threshold | `errors.rs:19` |
| 6005 | `CouncilTooLarge` | The council | The council multisig has more members than the program checks | `errors.rs:21` |
| 6006 | `CouncilKindInvalid` | The council | This council kind is not accepted | `errors.rs:23` |
| 6007 | `NoPendingCouncil` | The council | No council rotation is pending, or this signer is not the one proposed | `errors.rs:25` |
| 6008 | `ProposalExpired` | The council | The council's decision is past its valid_until | `errors.rs:27` |
| 6009 | `WrongAccount` | Accounts | Wrong mint or token account | `errors.rs:31` |
| 6010 | `AssetMintNotAllowed` | Accounts | The asset mint carries a Token-2022 extension the pool does not accept | `errors.rs:33` |
| 6011 | `ShareMintNotAllowed` | Accounts | The share mint is not one this pool can own and price: wrong authority, supply, decimals or extensions | `errors.rs:35` |
| 6012 | `AssetNotListed` | Accounts | The asset mint is not on this cluster's allow-list | `errors.rs:37` |
| 6013 | `AssetIsShareMint` | Accounts | The asset mint and the share mint are the same | `errors.rs:39` |
| 6014 | `CollectionNotAllowed` | Accounts | The collection is not a bare Metaplex Core collection whose update authority is the pool | `errors.rs:41` |
| 6015 | `PagareAccountsMismatch` | Accounts | The pagaré records passed are not the ones this payment touches, oldest first | `errors.rs:43` |
| 6016 | `BorrowerIsCouncil` | Roles | The council cannot be the borrower | `errors.rs:47` |
| 6017 | `BorrowerIsCurator` | Roles | A member of the council cannot be the borrower | `errors.rs:49` |
| 6018 | `EndorseeInvalid` | Roles | The endorsee is missing, or is the pool, the council, a curator, the guardian or the borrower | `errors.rs:51` |
| 6019 | `WrongAmount` | Amounts and invariants | Amount is zero or exceeds what is due | `errors.rs:55` |
| 6020 | `Overflow` | Amounts and invariants | Arithmetic overflow | `errors.rs:57` |
| 6021 | `InvariantBroken` | Amounts and invariants | A pool invariant does not hold after this instruction; the transaction is refused | `errors.rs:59` |
| 6022 | `InsufficientLiquidity` | Amounts and invariants | Not enough liquidity in the vault | `errors.rs:61` |
| 6023 | `ZeroShares` | Amounts and invariants | The deposit would mint zero shares | `errors.rs:63` |
| 6024 | `FirstDepositTooSmall` | Amounts and invariants | First deposit below the minimum | `errors.rs:65` |
| 6025 | `SlippageExceeded` | Amounts and invariants | The result is below the minimum the caller accepts | `errors.rs:67` |
| 6026 | `LineTooLarge` | Amounts and invariants | The principal exceeds the pool's maximum line, max_line_bps of the NAV | `errors.rs:69` |
| 6027 | `OutstandingCapExceeded` | Amounts and invariants | The draw would take the pool's outstanding principal above its cap | `errors.rs:71` |
| 6028 | `CollectionPeriodTooShort` | Pool parameters | The collection period is shorter than the minimum | `errors.rs:75` |
| 6029 | `AccelerationNoticeTooShort` | Pool parameters | The acceleration notice is below the seven-day floor | `errors.rs:77` |
| 6030 | `CancelGraceTooShort` | Pool parameters | The borrower's cancel grace is below the thirty-day floor | `errors.rs:79` |
| 6031 | `LineStatus` | Lines | The line is not in the required status | `errors.rs:83` |
| 6032 | `AgreementHashRequired` | Lines | The signed agreement's hash is required | `errors.rs:85` |
| 6033 | `ExpedienteRootRequired` | Lines | The expediente root is required | `errors.rs:87` |
| 6034 | `PagareTemplateHashRequired` | Lines | The pagaré template's hash is required | `errors.rs:89` |
| 6035 | `ExpedienteMismatch` | Lines | The expediente root or the terms hash is not the one the borrower requested | `errors.rs:91` |
| 6036 | `RequestExpired` | Lines | The request is older than the council's review window | `errors.rs:93` |
| 6037 | `SponsorRoleInvalid` | Lines | sponsor_role is unknown | `errors.rs:95` |
| 6038 | `TermEnded` | Lines | The term has ended, or would end before the opening | `errors.rs:97` |
| 6039 | `InvalidSchedule` | Lines | The installment schedule is not valid | `errors.rs:99` |
| 6040 | `MaxMissedTooLow` | Lines | max_missed is below the floor: at least one missed installment must come before the collection trigger | `errors.rs:101` |
| 6041 | `ScheduleEndsAfterTerm` | Lines | The last installment would fall due after the term | `errors.rs:103` |
| 6042 | `NotScheduled` | Lines | The line has no installment schedule | `errors.rs:105` |
| 6043 | `AvailabilityEnded` | Lines | The draw window of this line has closed | `errors.rs:107` |
| 6044 | `TooManyDraws` | Lines | The line has reached its maximum number of draws | `errors.rs:109` |
| 6045 | `DrawsNotSuspended` | Lines | The line's draws are not suspended | `errors.rs:111` |
| 6046 | `WindowClosed` | Lines | The line's draw window has already closed | `errors.rs:113` |
| 6047 | `AccelerationState` | Lines | No acceleration is pending on this line, or one already is | `errors.rs:115` |
| 6048 | `AccelerationNotDue` | Lines | The acceleration notice has not run | `errors.rs:117` |
| 6049 | `LateInterestUnpaid` | Lines | The repayment must cover the late interest due | `errors.rs:119` |
| 6050 | `NotEnoughMissed` | Lines | Fewer installments missed than the line allows before collection | `errors.rs:121` |
| 6051 | `TermNotEnded` | Lines | The term has not ended | `errors.rs:123` |
| 6052 | `WriteOffTooEarly` | Lines | A line in collection is written off only after its collection period, and after its oldest missed due date or its term, plus grace | `errors.rs:125` |
| 6053 | `FixDateInvalid` | The FIX | The FIX date is not a valid date, is in the future, or is older than the published one | `errors.rs:129` |
| 6054 | `FixJump` | The FIX | The FIX moves more than the feeder may publish; the council must | `errors.rs:131` |
| 6055 | `FixMismatch` | The FIX | The FIX signed is not the one published now | `errors.rs:133` |
| 6056 | `FixStale` | The FIX | No FIX is published, or it is too old for a draw | `errors.rs:135` |
| 6057 | `DocumentHashRequired` | Pagarés | The signed document's hash is required | `errors.rs:139` |
| 6058 | `SignatureMethodInvalid` | Pagarés | signature_method is unknown, or lacks its certificate fingerprint | `errors.rs:141` |
| 6059 | `PagareNotCovered` | Pagarés | No repayment has marked this pagaré paid | `errors.rs:143` |
| 6060 | `PagareAlreadyPaid` | Pagarés | This pagaré is already extinguished | `errors.rs:145` |
| 6061 | `CancelGraceNotElapsed` | Pagarés | The borrower may cancel alone only after the cancel grace from the payment | `errors.rs:147` |
| 6062 | `EndosoKindInvalid` | Pagarés | This kind of endoso is not one the program records | `errors.rs:149` |
| 6063 | `ProcuracionState` | Pagarés | The pagaré has no procuración in force, or already has one | `errors.rs:151` |
| 6064 | `AnnotationKindInvalid` | Pagarés | This kind is not a court annotation | `errors.rs:153` |
| 6065 | `EntryNotFound` | Pagarés | The referenced registry entry does not exist | `errors.rs:155` |

## 10. Compute

Measured on 2026-11-05 from this tree (`NO_DNA=1 anchor build --ignore-keys --arch v0
--tools-version v1.54 -- --features localnet`, then `cargo test -p elote_pool --test compute --
--nocapture`, three runs; the council-through-Squads figures from `tests/squads.rs`). The test
ceiling is `CEILING = 100 000` CU (`tests/compute.rs:17`), half the 200 000 CU a transaction gets
per instruction without a compute-budget request. Margin = 100 000 − CU.

| instruction | shape | CU | margin |
|---|---|---|---|
| `init_pool` | classic asset, plain or permissioned shares | 36 920 – 48 454 | 51 546 – 63 080 |
| `request_line` | 120 installments | 23 798 | 76 202 |
| `approve_line` | council signing directly | 14 744 | 85 256 |
| `approve_line` | through Squads `vault_transaction_execute` | 40 884 | 59 116 |
| `draw` | first tranche, 120 installments | 82 026 | 17 974 |
| `draw` | second tranche, 120 installments | 82 200 | 17 800 |
| `repay` | 18 installments, all missed | 39 398 | 60 602 |
| `repay` | 120 installments, all missed | 68 282 | 31 718 |
| `repay` | 120 installments, after an early mark (3 missed) | 57 909 | 42 091 |
| `repay` | covering four records, 120 installments, all missed | 96 578 | 3 422 |
| `mark_in_collection` | 18 installments, all missed | 16 727 | 83 273 |
| `mark_in_collection` | 120 installments, all missed | 36 737 | 63 263 |
| `mark_in_collection` | 120 installments, 3 missed | 13 720 | 86 280 |
| `write_off` | 18 installments, all missed | 26 573 | 73 427 |
| `write_off` | 120 installments, all missed | 55 337 | 44 663 |
| `write_off` | 120 installments, after an early mark | 55 338 | 44 662 |
| `suspend_draws` | | 14 563 | 85 437 |
| `resume_draws` | | 13 880 | 86 120 |
| `propose_acceleration` | | 14 577 | 85 423 |
| `accelerate_line` | 120 installments, all due | 49 244 | 50 756 |
| `pause` | by a member | 9 364 | 90 636 |
| `annotate_payment` | 120 installments | 20 305 | 79 695 |
| `cancel_pagare` | council signing directly | 51 746 | 48 254 |
| `cancel_pagare` | council through Squads | 83 865 | 16 135 |
| `cancel_pagare` | procurador | 53 457 | 46 543 |
| `cancel_pagare` | borrower after the cancel grace, under a procuración | 51 339 | 48 661 |
| `endorse_pagare` | plugin added | 38 290 – 45 790 | 54 210 – 61 710 |
| `endorse_pagare` | plugin approved (after a revocation) | 44 875 – 52 375 | 47 625 – 55 125 |
| `endorse_pagare` | through Squads | 71 585 | 28 415 |
| `revoke_procuracion` | 120 installments | 40 029 | 59 971 |
| `court_annotation` | 120 installments | 17 627 | 82 373 |

- **Key-dependent figures.** `init_pool` (Anchor's `init` bump searches for pool, vault and dead
  shares, plus `council::canonical_vault`) and `endorse_pagare` (`find_program_address` for the
  procurador address) cost about 1 500 CU per extra bump attempt, so they vary with the keys.
  `init_pool_cost_follows_the_pda_bump_searches` shows the whole spread is the bump tries:
  30 920 + 1 500 × tries (plain shares) and 31 954 + 1 500 × tries (permissioned). The
  procurador's `cancel_pagare` also searches; it measured 53 457 in all three runs.
- **Deterministic figures.** `draw` takes both PDA bumps from the client, so it does not search.
  Of its cost, the Core `CreateV2` CPI with the four plugins is about 37 000 CU.
- **The arrears loop** (`math::arrears`) is the only loop over the schedule, at most 120
  iterations; with every installment missed each iteration takes the accrual branch. `repay` also
  loops over the records it is given (each loaded and written) and costs per record.
- **Budget rules in the tests.** Every instruction stays below `CEILING`.
  `a_payment_covering_four_sidecars_stays_under_the_ceiling` also requires a 5 000 CU margin
  for the four-record `repay`; `tests/squads.rs` requires the same 5 000 CU margin for the council
  instructions through Squads. The client passes at most four records per `repay`
  (`MAX_SIDECARS_PER_REPAY = 4`, `clients/js/src/pagare.ts:75`) and splits a larger payment.
- **Not driven by `tests/compute.rs`:** `deposit`, `redeem`, `reject_line`, `withdraw_request`,
  `withdraw_acceleration`, `cancel_line`, `resume`, the council setters, `propose_council`,
  `accept_council`, `set_fx_feeder` and `post_fix`.

## 11. Toolchain, build and tests

### 11.1 Toolchain

| component | version | where pinned |
|---|---|---|
| Rust | 1.89.0 | `rust-toolchain.toml` |
| Anchor (`anchor-lang`, `anchor-spl`, CLI) | 1.2.1 | `Anchor.toml`, `programs/elote_pool/Cargo.toml` (`=1.2.1`) |
| Solana CLI | 4.1.2 | `Anchor.toml` |
| Platform tools | v1.54 | build flag `--tools-version v1.54` |
| SBPF | v0 | build flag `--arch v0` |
| `mpl-core` crate | 0.12.1 | `programs/elote_pool/Cargo.toml` (CPI builders and types) |
| LiteSVM | 0.11.0 | dev-dependency |
| Node | ≥ 22.12 | `package.json` |

### 11.2 Build

```sh
NO_DNA=1 anchor build --ignore-keys --arch v0 --tools-version v1.54 -- --features localnet
```

- `--tools-version v1.54`: `cargo-build-sbf` 4.1.0 fails to build the program with its default
  platform tools; v1.54 builds it.
- `--arch v0`: the SBPF v0 format, which the Solana 3.x runtime inside LiteSVM 0.11 loads.
- `--ignore-keys`: the program id in `declare_id!` and `Anchor.toml` is used as is; the
  `target/deploy/elote_pool-keypair.json` a fresh build writes is not the program's key.
- **Cluster features** select the asset allow-list (`state.rs:74-98`): no feature = mainnet (USDC
  `EPjF…Dt1v`, MEXe `MEXe…r4y`); `devnet` = Etherfuse sandbox MEXe and USDC and Circle's devnet
  USDC; `localnet` = any mint the asset policy accepts (tests and the local e2e). `devnet` and
  `localnet` together is a compile error.

Output: `target/deploy/elote_pool.so` and `target/idl/elote_pool.json`. `npm run idl:sync`
copies the IDL to `idl/`, `node codama.mjs` regenerates `clients/js/src/generated/`, and
`npm run check:client` fails if the committed client differs.

### 11.3 Tests

`cargo test -p elote_pool --no-fail-fast` runs 224 Rust tests in 20 binaries (plus an empty
doc-test binary); the LiteSVM binaries load `target/deploy/elote_pool.so`, so the build runs
first. `npm run test:client` runs 21 JS tests. Counts measured on 2026-11-05 from this tree.

| binary | tests | harness | what it checks |
|---|---|---|---|
| `src/` unit tests | 39 | plain | `math` (26), `fx` (5), `state` (3), `terms` (2), `instructions::lines` (2), `council` (1) |
| `tests/acceleration.rs` | 11 | LiteSVM | suspension and resumption, acceleration with notice on bullet and scheduled lines, the objective mark superseding a proposal, pause by a member or the guardian, pool limits and floors, the pause on approvals |
| `tests/audit.rs` | 4 | LiteSVM | repayment closing rules, the bare collection, the collection period before write-off |
| `tests/compute.rs` | 10 | LiteSVM | the CU figures of §10 under the ceiling |
| `tests/core_capabilities.rs` | 8 | LiteSVM + Core 0.15.3 | what Core allows after an asset's creation (§8.1) |
| `tests/council.rs` | 14 | LiteSVM | the council constants, `init_pool` against multisig shapes (majority, config authority, size, owner), parameter floors, the two-step rotation |
| `tests/fx.rs` | 12 | LiteSVM | `post_fix` by council and feeder, the band, Mexico City dates, draws and repayments at the FIX, the outstanding and line caps at the FIX, FX gains and losses, write-off at USDC cost |
| `tests/known_limits.rs` | 12 | LiteSVM | documentation tests that pin current behaviour (pricing around repayments and write-offs, the first-deposit floor, late interest settled at execution, the client contract of `repay`) |
| `tests/layout.rs` | 1 | LiteSVM | account sizes 527 / 529 / 399 and the zeroed `reserved` tails |
| `tests/lines.rs` | 11 | LiteSVM | the line cycle end to end, sponsor, overpayment, liquidity, write-off, the bullet window, the line cap |
| `tests/math_props.rs` | 7 | proptest | the pure functions over random inputs |
| `tests/pagare.rs` | 22 | LiteSVM + Core 0.15.3 | the asset and record per draw, plugins read back through `mpl-core`, the registry entries, payment entries and the cursor, the three cancellation branches, procuración and revocation, court and receipt annotations |
| `tests/pool.rs` | 22 | LiteSVM | `init_pool` and the mint and collection policies, deposits and redemptions, dead shares, rounding and slippage, a Token-2022 asset cycle, `NavUpdated` |
| `tests/reaudit.rs` | 4 | LiteSVM | interest-only payments and the registry, the entry counter width, who can be procurador, receipt annotation until the burn |
| `tests/requests.rs` | 7 | LiteSVM | request, approval of exact terms, the 15-day window, members as borrowers, reject and withdraw returning rent, the pause on requests |
| `tests/schedule.rs` | 20 | LiteSVM, clock warped | schedule validation, installments, missed count, late interest and its caps, repayment order, collection and acceleration, write-off timing |
| `tests/squads.rs` | 4 | LiteSVM + Squads v4 | council instructions through the real `vault_transaction_execute` (CPI depth 4), stale proposals, a multisig lowered below a majority |
| `tests/tranches.rs` | 12 | LiteSVM, clock warped | tranched draws, the strict window, the payability floor, per-tranche interest, `MAX_DRAWS`, calendars, `suspend_draws` and `cancel_line` |
| `tests/valid_until.rs` | 3 | LiteSVM | `valid_until` on approvals, setters, rotation, the FIX, write-off and the council's cancellation |
| `tests/vectors.rs` | 1 | plain | 427 arrears vectors (50 accelerated) shared with `clients/js/test/arrears-vectors.json` |
| `clients/js/test/*.test.ts` | 21 | `tsx --test` | `pagare.ts` (8), `schedule.ts` (7), `hashes.ts` (3), `core.ts` (1), `fx.ts` (1), `terms.ts` (1) |

On this tree 223 of the 224 Rust tests pass. `a_payment_covering_four_sidecars_stays_under_the_ceiling`
measures 96 578 CU against the 95 000 CU its 5 000 CU margin allows (§10).

### 11.4 Fixtures

`programs/elote_pool/tests/fixtures/` (see its README), each pinned by sha256 and checked at load:

| file | program | source | sha256 |
|---|---|---|---|
| `mpl_core_program-0.15.3.so` | Metaplex Core `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d` | GitHub release `release/core@0.15.3` | `5b4dacda…dd56ce5` |
| `squads_v4-mainnet.so` | Squads v4 `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf` | mainnet dump of 2026-11-04 (upgrade authority none) | `dec8d3e0…d01d34c` |

Core is loaded by every LiteSVM harness (`tests/common/mod.rs`); Squads only by
`tests/squads.rs`. The other tests sign as the council vault directly with signature verification
off.

## 12. Deployment

| cluster | program id | upgrade authority | build |
|---|---|---|---|
| devnet | `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM` | `GU7CfoGZYsfkx23aJoqFjfwyAmLCExqBBoBFrn2EvZKS` (single key) | an earlier build: Anchor 1.1.2 and Solana CLI 3.1.10, last commit on that toolchain tag `pre-audit-2026-10-19`. It is not `council/pr4` and has none of the council, FIX or request instructions |
| mainnet-beta | not deployed | | |

The program id is the same in `declare_id!` (`lib.rs:28`) and `Anchor.toml` for localnet and
devnet. The upgrade authority moves to a Squads multisig before mainnet; the IDL goes on chain
(`anchor idl init`) and the deployed build is verified with `solana-verify` against a build of
the source with its cluster's feature (no feature for mainnet, `devnet` for devnet). The program
embeds `security.txt` (`lib.rs:33`).
