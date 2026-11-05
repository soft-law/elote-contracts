# `elote_pool` architecture

How the program is built, instruction by instruction, byte by byte. The [README](README.md) says
what the pool does for investors and borrowers; this file says how the code does it, for whoever
has to change it, audit it or write a client against it. Source of truth is
`programs/elote_pool/src/`; where this file and the code disagree, the code wins and this file is
the bug.

Program id `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`, Anchor 1.2.1, Solana 4.1.2; the asset
is a classic SPL Token or a Token-2022 mint; 163 Rust tests and 11 JS tests.

## 0. One program, not two: where the loan contract lives

There is no separate "loan program". The programmable loan contract the README talks about is
the `CreditLine` account, and the code that executes it is `instructions/lines.rs`. The fund is
the `Pool` account plus `instructions/pool.rs`. Both halves live in `elote_pool` because they
share one thing that cannot be split: the money.

```
 off chain                                   on chain (elote_pool)
 ─────────                                   ─────────────────────
 agreements engine (softlaw-backend):        open_line(agreement_hash, terms, schedule)
   credit agreement template 0.1                │  admin + borrower + attestor (+ sponsor) sign
   (apertura de crédito, LGTOC 291-301)         ▼
   parties sign the JSON document          CreditLine PDA  ◄── the programmable loan contract
   keccak256(utf8(JSON)) = agreement_hash       │
                                                │ draw / repay / mark_in_collection / write_off
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
  (`F7omfV66XSjsYm1VVVCPyhnnnW72ZYUtqwZPLJWdS82G`, repo `soft-law/softlaw-solana`) makes the same
  call: its IP-backed credit facilities sit in the same program as the marketplace because the
  repayment waterfall needs the sale proceeds. `elote_pool` does not call that program, and that
  program does not know about `elote_pool`.
- **When a split would earn its place:** a second product borrowing from the pool (the pool would
  then expose a lending interface), loan servicing that must upgrade independently of the money
  logic, or one loan funded by several pools (the loan becomes an account of its own program and
  pools hold positions). None of these exists today.

## 1. One program, seven modules

```
programs/elote_pool/src/
├── lib.rs            declare_id!, security.txt, the #[program] module: twenty thin entry points
├── state.rs          Pool, CreditLine and Pagare layouts, PDA seeds, constants
├── errors.rs         PoolError (46 codes, 6000..6045, grouped by domain)
├── events.rs         22 events, all emitted through emit_cpi!
├── math.rs           pure arithmetic: share pricing, installments, late interest, arrears
├── registry.rs       the pagaré registry: entry hashes and the sidecar's hash chain
└── instructions/
    ├── pool.rs       init_pool, deposit, redeem, set_paused, set_attestor, set_collection_period,
    │                 set_cancel_delay, and the pool_signer! macro
    ├── lines.rs      open_line, draw (which mints the pagaré), repay, mark_in_collection, write_off,
    │                 suspend_draws, cancel_line
    └── pagare.rs     cancel_pagare, endorse_pagare, revoke_procuracion, court_annotation,
                      request_cancellation, annotate_payment
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
the admin key, the asset mint and the share mint (created by the admin and validated at
`init_pool`).

```
Pool            PDA ["pool", admin, asset_mint]          data account, program-owned
├── share_mint  admin-supplied mint (any address)        classic or Token-2022, decimals = asset decimals,
│                                                        mint authority = pool, freeze authority = the
│                                                        admin's choice (Token ACL MintConfig in production)
├── vault       PDA ["vault", pool]                      token account of asset_mint, owner = pool
├── dead_shares PDA ["dead", pool]                       token account of share_mint, owner = pool
└── CreditLine  PDA ["line", pool, line_id as u64 LE]    one per approved line, program-owned
    ├── Pagare  PDA ["pagare", line, draw_index as u16 LE]        one per draw: the program's record of
    │   │                                                        the pagaré, program-owned
    │   └── procurador authority  PDA ["procurador", pagare, procurador]  no account: the authority of
    │                                                        the pagaré's BurnDelegate during a
    │                                                        procuración; only the program signs for it
    └── asset   PDA ["pagare_asset", line, draw_index as u16 LE]  the pagaré itself: a Metaplex Core
                                                                 asset owned by the Core program, held
                                                                 by the pool PDA, in the series'
                                                                 collection (`Pool.collection`)
```

- The pool address is a function of `(admin, asset_mint)`: one admin can run one pool per asset,
  and the admin can never change (it is part of the address).
- The vault and the dead-shares account are PDAs, not associated token accounts, so nobody can
  create them ahead of `init_pool` and block the pool.
- `line_id` must equal `pool.line_count` at opening, so lines are a dense sequence `0, 1, 2…`
  and a client can enumerate them without an indexer.
- The pool PDA signs every outbound CPI (minting shares, paying a draw, paying a redemption,
  creating, thawing and burning a pagaré) with `pool_signer!(pool)`, which rebuilds the seeds
  `["pool", admin, asset_mint, bump]` from the account's own fields.
- `draw_index` is `CreditLine.draws` before the draw, so a line's pagarés are a dense sequence
  `0, 1, 2…` below `MAX_DRAWS`, and the asset of any draw is derivable from the line address
  alone. The asset is a PDA of this program (not a client keypair), which the pool PDA and the
  asset PDA both sign for in the Core `CreateV2` CPI; the Core program then owns the account.
  The client passes both bumps to `draw`, so the program verifies the addresses without a search.

Every account is Borsh with no padding: every parameter is a typed field, and each account ends
in `reserved` bytes, zero at creation, for fields added after mainnet (zero there always means
"no value"). `tests/layout.rs` pins the sizes. Offsets include the 8-byte Anchor discriminator,
so they can be used directly in `memcmp` filters.

### 2.1 `Pool` (346 bytes)

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 32 | `admin` | signer of `init_pool`; part of the PDA seeds |
| 40 | 32 | `attestor` | the agreements engine's key; co-signs openings, draws, endosos, court annotations and cancellations; never the admin; rotated by `set_attestor` |
| 72 | 32 | `asset_mint` | part of the PDA seeds |
| 104 | 32 | `share_mint` | mint authority = the pool PDA |
| 136 | 32 | `vault` | |
| 168 | 32 | `dead_shares` | |
| 200 | 32 | `collection` | the series' Metaplex Core collection the pagaré assets are minted into; admin-created, validated at `init_pool` (owner, `CollectionV1`, update authority = pool, bare) |
| 232 | 8 | `outstanding_principal` | principal drawn and neither repaid nor written off |
| 240 | 8 | `interest_collected` | informative running total (ordinary interest only) |
| 248 | 8 | `written_off` | informative running total |
| 256 | 8 | `line_count` | next `line_id` |
| 264 | 4 | `grace_period_seconds` | seconds after a due date before late interest and a write-off; zero means none; fixed at `init_pool` |
| 268 | 4 | `collection_period_seconds` | seconds from the mark before a line in collection can be written off; at least 30 days; `set_collection_period` |
| 272 | 4 | `cancel_delay_seconds` | the backstop's wait from a cancellation request; at least seven days; `set_cancel_delay` |
| 276 | 2 | `max_line_bps` | the largest line as a share of the NAV at the opening, `1..=10 000`; fixed at `init_pool` |
| 278 | 1 | `asset_controls` | `ASSET_CONTROL_*` bits of the asset mint: 0 freeze authority, 1 `Pausable`; recorded at `init_pool` |
| 279 | 1 | `permissioned` | the share mint is `DefaultAccountState = Frozen`; recorded at `init_pool` |
| 280 | 1 | `paused` | |
| 281 | 1 | `bump` | |
| 282 | 64 | `reserved` | zero |

`Pool::max_line_principal(nav)` applies the cap (`nav * max_line_bps / 10 000`, rounded down),
and the JS client mirrors it in `clients/js/src/schedule.ts` (`maxLinePrincipal`).
`asset_freezable()` and `asset_pausable()` read the two `asset_controls` bits.

### 2.2 `CreditLine` (375 bytes)

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 32 | `pool` | |
| 40 | 8 | `line_id` | |
| 48 | 32 | `borrower` | co-signer of `open_line`; the only key allowed to `draw` |
| 80 | 33 | `sponsor` | `Option<Pubkey>`: 1 tag byte + 32 |
| 113 | 1 | `sponsor_role` | 0 promoter, 1 guarantor; meaningful only when `sponsor` is named; recorded, not enforced |
| 114 | 32 | `agreement_hash` | hash of the signed credit agreement; never zero |
| 146 | 32 | `expediente_root` | salted merkle root of the credit file; never zero |
| 178 | 1 | `status` | `LineStatus` |
| 179 | 8 | `principal` | the approved commitment; `available = principal - drawn` |
| 187 | 2 | `rate_bps` | annual, ≤ 10 000; charged per tranche from its draw to the last due date |
| 189 | 8 | `term_end` | unix seconds; the maturity of a bullet line, the bound of a schedule |
| 197 | 8 | `availability_end` | last instant a draw is accepted (strict): `first_due_at` or `term_end`; `suspend_draws` brings it to now |
| 205 | 8 | `drawn` | principal transferred so far, `<= principal`, never decreases |
| 213 | 2 | `draws` | tranches so far, `<= MAX_DRAWS` (64) |
| 215 | 8 | `interest_due` | Σ over tranches of `amount * rate_bps * (last_due - drawn) / (10 000 * SECONDS_PER_YEAR)`, each fixed at its draw; `mark_in_collection` removes the forgiven interest |
| 223 | 8 | `principal_repaid` | |
| 231 | 8 | `interest_repaid` | |
| 239 | 2 | `pagares_paid` | the payment cursor: the index of the oldest pagaré no repayment has marked paid |
| 241 | 8 | `first_due_at` | the first due date from the signed contract; 0 on a bullet line |
| 249 | 8 | `interval_seconds` | seconds between due dates; 0 on a bullet line |
| 257 | 2 | `installments` | `1..=120`; 0 on a bullet line |
| 259 | 2 | `late_bps` | annual late rate on overdue principal, ≤ 10 000 |
| 261 | 1 | `max_missed` | missed installments that allow `mark_in_collection`, `3..=installments` |
| 262 | 8 | `late_collected` | late interest collected so far |
| 270 | 8 | `late_paid_until` | late interest is settled up to this instant; 0 until a repayment collects some |
| 278 | 8 | `opened_at` | |
| 286 | 8 | `drawn_at` | time of the first tranche; informational (the calendar is `first_due_at`) |
| 294 | 8 | `closed_at` | set by the repayment that closes the line, by `write_off` or by `cancel_line` |
| 302 | 8 | `accelerated_at` | when `mark_in_collection` accelerated the balance; 0 before. From then on every installment is due and late interest runs on all remaining principal; `write_off` waits `collection_period_seconds` from it |
| 310 | 1 | `bump` | |
| 311 | 64 | `reserved` | zero |

A bullet line has `installments == 0`, and so zero `first_due_at`, `interval_seconds`,
`late_bps` and `max_missed`. The schedule terms (`first_due_at` through `max_missed`) never change
after `open_line`; `late_collected` and `late_paid_until` are the only schedule fields that move,
and only `repay` writes them. `CreditLine::schedule()` returns `None` when `installments` is zero,
which is how every handler tells the two kinds apart (the JS client's `lineSchedule(line)` returns
the schedule or `null` the same way). `CreditLine::anchor()` (`first_due_at - interval`) is the
single place the calendar is anchored, and `last_due()` (the last due date, or `term_end` on a
bullet line) is the instant every tranche's interest runs to.

### 2.3 `Pagare` (387 bytes) and the pagaré asset

The sidecar `draw` writes next to each Core asset. The signed data message is the pagaré; the
asset is its registration and circulation record and the sidecar is the program's half of that
record. It holds no folio (the engine maps `draw_index` to the folio off chain) and nothing
personal, and it is never closed: after `cancel_pagare` it stays as the PAGADO mark (status,
`paid_at`, the receipts' hashes, the registry), so its rent is a cost of every draw.

| offset | size | field | notes |
|---|---|---|---|
| 0 | 8 | discriminator | |
| 8 | 32 | `line` | the `CreditLine` |
| 40 | 2 | `draw_index` | `CreditLine.draws` at issuance |
| 42 | 32 | `asset` | the Core asset's address (`["pagare_asset", line, draw_index]`) |
| 74 | 32 | `pagare_hash` | the `document_hash` of the draw: the signed pagaré's hash |
| 106 | 8 | `amount` | the tranche, in base units of the asset |
| 114 | 8 | `issued_at` | the draw's timestamp |
| 122 | 8 | `due_at` | the line's last due date at issuance (`last_due()`) |
| 130 | 8 | `drawn_through` | the line's `drawn` after this tranche: Σ amounts of pagarés `0..=draw_index`; the coverage rule reads it instead of earlier sidecars (`tests/pagare.rs` pins the invariant) |
| 138 | 32 | `cert_fingerprint` | hash of the PSC certificate that signed the pagaré text (C6a); zero only with `signature_method` 0 |
| 170 | 1 | `signature_method` | 0 wallet only, 1 PSC + wallet, 2 SAT e.firma + wallet (`SignatureMethodInvalid` otherwise) |
| 171 | 1 | `status` | 0 `PAGARE_OUTSTANDING`, 2 `PAGARE_PAID_PENDING_CANCEL` (a repayment covered it; the burn is owed), 1 `PAGARE_PAID` (burnt); never reverts |
| 172 | 8 | `paid_at` | when the repayment that covered it landed; zero while outstanding |
| 180 | 32 | `receipt_hash` | that repayment's receipt hash: the payer's, which the holder's `annotate_payment` replaces until the burn; zero while outstanding |
| 212 | 32 | `cancel_receipt_hash` | the final receipt `cancel_pagare` was given; zero until the burn |
| 244 | 8 | `cancel_after` | the backstop's deadline set by `request_cancellation`; zero when no request is pending (any cancellation clears it) |
| 252 | 32 | `requested_receipt_hash` | the receipt hash of that request |
| 284 | 32 | `procurador` | the procurador of the procuración in force (`flags` bit 1); the zero key when none. Set by `endorse_pagare`, cleared by `revoke_procuracion` and by the extinction |
| 316 | 1 | `flags` | bit 0: the asset carries an owner-managed `BurnDelegate` (a later procuración approves instead of adds); bit 1: an endoso en procuración is in force |
| 317 | 32 | `log_root` | the registry's hash chain (below) |
| 349 | 4 | `entry_count` | entries in the chain (u32); 1 after the draw |
| 353 | 1 | `last_kind` | `KIND_*` of the last entry |
| 354 | 1 | `bump` | |
| 355 | 32 | `reserved` | zero |

**The asset.** `draw` creates it in the same instruction with: owner = pool PDA (the vehicle's
custody); update authority = the series' collection, whose own update authority is the pool PDA
(Core expresses the authority of an asset in a collection through the collection); and four
plugins:

- `PermanentFreezeDelegate { frozen: true }` whose authority is `UpdateAuthority` (the
  collection, so the pool PDA, for as long as the asset is in the collection, which is always):
  the asset cannot move, and only the program can thaw it, which `cancel_pagare` does right
  before the burn;
- `ImmutableMetadata`, authority `None`;
- `Attributes`, authority `None`, carrying the nine facts `pagare_hash`, `amount`, `currency`
  (the asset mint), `issued_at`, `due_at`, `line`, `draw_index`, `cert_fingerprint` and
  `signature_method`, the 32-byte values as lower-case hex (a base58 encoding on chain costs tens
  of thousands of CU);
- `AddBlocker`, authority `None`, so no authority-managed plugin is added later.

No transfer or burn delegate at creation: the pool is the owner while the asset is in the pool
and only the program signs for it. The only plugin added later is the owner-managed
`BurnDelegate` of a procuración (`endorse_pagare`). No external adapter and no AppData: the
registry lives in the sidecar, and the asset carries nothing but immutable attributes.
`init_pool` admits only a bare collection, so no collection plugin reaches the asset. The name is
`PAGARE <line_id>-<draw_index>`: no series label, none of the words a court read as an offering
in 2016609 (C29); the URI is empty. The program never reads the asset back: `cancel_pagare`
trusts the sidecar (`asset` must be `Pagare.asset`) and the Core program's own checks in its CPIs
(thaw as the collection's authority, then `BurnV1` as the owner or through the procurador's
address).

What Core lets the parties do with such an asset after its creation, pinned by
`tests/core_capabilities.rs` against the 0.15.3 binary (condition C9):

- A permanent delegate's authority cannot be reassigned directly: `ApprovePluginAuthorityV1` on a
  delegated permanent plugin fails with `CannotRedelegate` for everyone, the delegate and the
  collection's update authority included. `RevokePluginAuthorityV1` by the delegate or by the
  collection's authority resets it to `UpdateAuthority` (the collection, so the pool PDA); from
  there only the collection's authority can approve a key or `None`, and `None` is terminal (no
  approve, no revoke after it). A handoff is two acts of the pool; an extinction is irreversible.
- Only the permanent freeze delegate's authority can thaw or re-freeze (`UpdatePluginV1`); the
  collection's authority, the owner and strangers get `NoApprovals`. A frozen asset whose freeze
  authority became `None` is immovable forever (the owner's transfer is refused; Core reports
  `InvalidAuthority`); thawed first, it becomes an ordinary asset its owner transfers freely.
- An asset's update authority cannot leave its collection: `UpdateV1` to an address or `None` and
  `UpdateV2` to an address fail with `InvalidAuthority` for everyone, the collection's authority
  included. That authority can still add authority-managed plugins (`UpdateDelegate` went
  through), not permanent or owner-managed ones; the `AddBlocker` set at creation closes that door
  (at about 4 200 CU per creation).
- AppData adapters: the data authority is fixed at creation and alone writes the data; adding or
  removing adapters is the collection's authority alone, and `AddBlocker` stops neither. The
  asset carries no adapter, so that residual power has nothing to act on.
- Owner-managed delegates are the owner's to add (`AddBlocker` or not), approve and revoke, and
  reset to `Owner` on transfer; an approved `BurnDelegate` burns a thawed asset. One approved to a
  program address refuses the procurador's own key.

**Endosos.** The pool makes only the endoso en procuración (LGTOC 35; the vehicle makes no endoso
en propiedad, decision 29): collection runs through procuradores and the asset stays with the
pool. The procuración lends the power to extinguish the pagaré through an owner-managed
`BurnDelegate` whose authority is the program address `["procurador", pagare, procurador]`, never
the procurador's key: an approved owner-managed delegate can call `BurnV1` directly, which would
extinguish the title without payment, receipt or PAGADO mark. Only the program signs for that
address, so the burn happens only inside `cancel_pagare` (§5.13–§5.15).

**The registry: a hash chain in the sidecar** (`registry.rs`). The registry is program-owned so
that no holder can block or erase an entry: a payment entry must not depend on a third-party
holder's cooperation (LGTOC 130), and rewriting or removing entries would alter the title (arts. 8
fr. VI and 13). The pagaré text carries the mandate that lets the system write the payment entries
for the holder ("el tenedor autoriza al sistema de información a asentar los abonos").

- Each entry is `(asset, kind, from, to, hash, stated_at, entry_count)`, where `entry_count` is
  the entry's own position (1 for the issuance entry). Its hash is

  ```
  entry_hash = sha256(asset || kind || from || to || hash || stated_at || entry_count)
               32 B     1 B    32 B   32 B  32 B   i64 LE   u32 LE
  ```

  141 bytes of preimage, the asset first so two pagarés with the same facts never share a chain.
- The sidecar keeps `log_root = sha256(log_root || entry_hash)`, starting from 32 zero bytes, so
  the issuance entry is the genesis; `entry_count` (u32) and `last_kind` are updated with it.
  `registry::append` is the only writer and fails (`Overflow`) if the count would overflow.
- The entry's full content goes in the event `PagareEntry { pool, line_id, draw_index, asset,
  kind, from, to, entry_hash, entry_count, recorded_at, stated_at }`, so anyone can replay the
  chain from the events and compare it with `log_root`. `stated_at` is the date the entry states
  and `recorded_at` the slot's time; every entry the program writes has both at `now`.
- Kinds are fixed codes, and an instruction refuses a kind it does not write (art. 30 presumes
  propiedad when the class is missing, so it is never left blank): `Emision` (0), reserved (1,
  for the endoso en propiedad), `Procuracion` (2), `Revocacion` (3), `Terminacion` (4), `Abono`
  (5), `AnotacionJudicial` (6), `ConstanciaJudicial` (7), reserved (8, for the late endoso),
  `Pagado` (9), `SolicitudCancelacion` (10), `Recibo` (11). The reserved codes are never reused.

| instruction | kind | from | to | hash |
|---|---|---|---|---|
| `draw` | `Emision` | pool | pool | `pagare_hash` |
| `endorse_pagare` | `Procuracion` | pool | the procurador | `endoso_hash` |
| `revoke_procuracion` | `Revocacion` | pool | the recorded procurador | `entry_hash` (the signed revocation) |
| `court_annotation` | `AnotacionJudicial` or `ConstanciaJudicial` | pool | pool | `document_hash` |
| `repay` | `Abono` or `Pagado` | the payer | pool | `receipt_hash` |
| `request_cancellation` | `SolicitudCancelacion` | pool | pool | `receipt_hash` |
| `cancel_pagare` | `Terminacion` | pool, or the procurador on its branch | pool | `receipt_hash` |
| `annotate_payment` | `Recibo` | the annotator (admin or attestor) | pool | `receipt_hash` |

A payment writes an entry on a pagaré only if it applies principal to it: `Pagado` on each pagaré
it covers, `Abono` on the one it reaches without covering. An interest-only payment is recorded in
`PagarePayment` with no entries, so nobody can grow a chain one unit at a time (§5.6). The
registry is the sidecar's chain together with `PagareIssued` and Core's ownership transactions. A
Core transfer made outside the program leaves no entry; the verifier shows it as "transferencia
sin asiento", never described in the title text as "not an endoso" (art. 25 risk; template side),
and shows the class of the last entry.

### 2.4 Constants (`state.rs`)

| constant | value | role |
|---|---|---|
| `MAX_DRAWS` | 64 | tranches per line (`TooManyDraws`); bounds a line's pagaré index space |
| `MAX_SIDECARS_PER_REPAY` | 3 | pagaré records one `repay` takes (`TooManySidecars`); bounds its compute |
| `min_first_deposit(decimals)` | `max(10^decimals, 1 000 × DEAD_SHARES)` | floor on the first deposit: one whole unit of the asset, never below 1 000 000 base units |
| `DEAD_SHARES` | 1 000 | shares locked forever by the first deposit |
| `MIN_FIRST_DEPOSIT_DEAD_RATIO` | 1 000 | the first deposit is at least this many dead shares, whatever the decimals |
| `BPS` | 10 000 | basis points |
| `MAX_INSTALLMENTS` | 120 | bounds the arrears loop and its compute |
| `MIN_MAX_MISSED` | 3 | policy: three missed installments before collection |
| `MIN_COLLECTION_PERIOD_SECONDS` | 2 592 000 | 30 days: the shortest collection period a pool may set |
| `MIN_CANCEL_GRACE_SECONDS` | 2 592 000 | 30 days: the shortest wait from a pagaré's payment before its borrower may cancel it alone |
| `MAX_CANCEL_GRACE_SECONDS` | 31 536 000 | 365 days: the longest such wait (`CancelGraceTooLong`); each repayment fixes the pool's value on the pagarés it marks paid (`Pagare.cancel_grace_seconds`) |
| `MIN_CANCEL_DELAY_SECONDS` | 604 800 | seven days: the shortest wait from a cancellation request to the holder's cancellation alone |
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
                     (scheduled lines, anyone)     ▼  │
                                              InCollection ─── repay (last unit) ──► Repaid
                                                   │
                     write_off (admin) ◄───────────┘        write_off (admin) ◄── Active
                           │
                           ▼
                       WrittenOff
```

- `Approved` lines hold no money. `draw` keeps the status `Active` on later tranches. An
  undrawn line past its draw window can never be drawn; the admin clears it with `cancel_line`
  (`Approved` and `drawn == 0` → `Cancelled`, terminal) and can close any line's draw window
  early with `suspend_draws` (`availability_end = now`, status unchanged).
- `Repaid`, `WrittenOff` and `Cancelled` are terminal. No instruction closes or reopens an
  account.
- After the mark there is no cure: the balance is accelerated and only full repayment moves the
  line to `Repaid`; `write_off` waits for the pool's collection period from the mark
  (`tests/schedule.rs::after_the_mark_there_is_no_cure_and_the_write_off_waits_for_the_collection_period_from_the_mark`,
  `tests/audit.rs::f03_write_off_waits_for_the_pools_collection_period_from_the_mark`).
- A line with nothing due stays `Active` while it can still draw; `Repaid` needs both nothing due
  and nothing drawable (the window closed, the commitment fully drawn, or the line in
  collection), so a third party cannot close an open line by paying its debt
  (`tests/audit.rs`).

**Pause matrix.** `set_paused(true)` blocks `deposit`, `redeem`, `open_line` and `draw`. It never
blocks `repay`, `mark_in_collection`, `write_off`, the pagaré instructions (`cancel_pagare`,
`endorse_pagare`, `revoke_procuracion`, `court_annotation`, `request_cancellation`,
`annotate_payment`) or `set_paused(false)`: money can always come back in and facts can always
be recorded, but nothing leaves the vault while paused.

## 5. Instructions in detail

For each instruction: who signs, what the account constraints already guarantee before the
handler runs, the checks in the order the handler makes them, what it writes, which CPIs it
makes and which events it emits. Error codes are `PoolError` variants, numbered from 6000 in
the order of `errors.rs` (46 codes, 6000..6045, grouped by domain: signers, accounts, roles,
amounts and invariants, pool parameters, lines, pagarés).

**Rule for foreign accounts.** On chain, the program reads an account of another program (a
Metaplex Core collection or asset) by fixed offsets and bounded lengths, never with that crate's
full decoder: `validate_collection` checks the owner, the data length, the `Key` byte at offset 0
and the update authority at bytes 1..33, and that is the only Core account the program decodes at
all. A full decoder run against an attacker-chosen account of the right owner can allocate until
the program aborts (an asset passed as the collection does exactly that); a fixed
read fails closed. For the pagaré assets the program trusts its own sidecar and the CPIs.

### 5.1 `init_pool(grace_period_seconds: u32, max_line_bps: u16, attestor: Pubkey, collection_period_seconds: u32, cancel_delay_seconds: u32)`

- **Signers:** `admin` (also pays rent).
- **Constraints:** `pool` is `init` at `["pool", admin, asset_mint]`; `share_mint` is an
  existing `InterfaceAccount<Mint>` the admin created (validated below, not `init`); `vault` is
  `init` at `["vault", pool]` for `asset_mint` with `authority = pool` under `token_program`
  (the asset's program); `dead_shares` is `init` at `["dead", pool]` for `share_mint` with
  `authority = pool` under `share_token_program` (the share mint's program). Both token
  programs are `Interface<TokenInterface>`: classic SPL Token or Token-2022, independently.
- **Checks, in order:** `1 <= max_line_bps <= 10 000` (`WrongAmount`);
  `collection_period_seconds >= 30 days` (`CollectionPeriodTooShort`); `cancel_delay_seconds >=
  7 days` (`CancelDelayTooShort`); `attestor != Pubkey::default()` (`WrongAccount`) and
  `attestor != admin` (`AttestorIsAdmin`: the two-signature model must not collapse into one
  key); `asset_mint.decimals <= 19`, so that `min_first_deposit` exists (`WrongAccount`). Any
  value of `grace_period_seconds` is accepted, including zero. Then the asset-mint policy
  (`asset_controls` in `pool.rs`): the mint is unpacked with its extensions and refused
  (`AssetMintNotAllowed`) if it carries `TransferFeeConfig`, `PermanentDelegate`,
  `TransferHook`, `NonTransferable`, `ConfidentialTransferMint`,
  `ConfidentialTransferFeeConfig`, `ConfidentialMintBurn`, `MintCloseAuthority`, or
  `DefaultAccountState` other than `Initialized`; a freeze authority and `Pausable` are accepted
  and recorded as bits of `asset_controls`; other extensions are accepted and ignored. Then the
  share-mint policy (`validate_share_mint`): owned by `share_token_program`, `mint_authority ==
  pool`, `supply == 0`, `decimals == asset_mint.decimals`; refused extensions
  `TransferFeeConfig`, `TransferHook`, `MintCloseAuthority`, `PermanentDelegate`,
  `InterestBearingConfig`, `ScaledUiAmount` (a display rate or multiplier on a NAV-priced share
  misleads wallets), `Pausable` (a pause of every deposit and redemption outside `set_paused` and
  its event) and the three confidential ones; `DefaultAccountState == Frozen` sets
  `permissioned` and then requires a freeze authority; `NonTransferable` is allowed (deposit
  mints, redeem burns); all failures are `ShareMintNotAllowed`. Who the freeze authority is
  (Token ACL's `MintConfig` or a key) is not recorded. Last, the `collection` account must be a
  bare Metaplex Core collection the pool controls (`validate_collection`: owned by the Core
  program, key `CollectionV1`, update authority = the pool PDA, the account exactly the base
  record with no plugin header after it, `num_minted == 0` and `current_size == 0`;
  `CollectionNotAllowed`). Core applies a collection's permanent plugins to every asset in it, so
  a collection-level delegate would reach every pagaré. Every read is at a fixed offset or
  bounded by a length read first; the strings are never decoded.
- **Writes:** every `Pool` field, `reserved` zeroed. On a permissioned share mint the dead
  account is born frozen: the freeze authority thaws it in this transaction or before the first
  deposit, or the first deposit fails inside Token-2022 (`AccountFrozen`).
- **CPIs:** the three `init`s (system program + token programs) done by Anchor.
- **Events:** `PoolInitialized`, which carries every parameter the pool recorded: `admin`,
  `attestor`, `asset_mint`, `share_mint`, `collection`, `grace_period_seconds`,
  `collection_period_seconds`, `cancel_delay_seconds`, `max_line_bps`, `asset_controls`,
  `permissioned`.
- **Compute:** 34 543 to 42 043 CU (`tests/compute.rs`; plain and permissioned share mints). The
  `init`s search for their bumps, so the figure varies with the keys by 1 500 CU per extra
  attempt.

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
- **CPIs:** `transfer_checked` assets depositor → vault (depositor signs); `mint_to` shares → depositor
  (pool signs); on a first deposit, `mint_to` `DEAD_SHARES` → dead account (pool signs).
- **Writes:** none to `Pool`. The state is in the token accounts. `Pool::check` runs on the
  reloaded vault and share supply.
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
- **CPIs:** `burn` shares (holder signs); `transfer_checked` vault → holder (pool signs).
  `Pool::check` runs on the reloaded vault and share supply.
- **Events:** `Redeemed`, then `NavUpdated`.

A redemption while principal is out and the vault is thin fails rather than paying a partial
amount; the holder retries with fewer shares.

### 5.4 `open_line(line_id, principal, rate_bps, term_end, agreement_hash, sponsor, sponsor_role, schedule, expediente_root)`

- **Signers:** `admin` (pays rent for the line), `borrower`, `attestor` (must equal
  `pool.attestor`, `AttestorRequired`), and `sponsor` when one is named in the arguments
  (`Option<Signer>`).
- **Constraints:** `pool` has `has_one = admin` (`Unauthorized`); `line` is `init` at
  `["line", pool, line_id LE]`, so an existing `line_id` fails at account creation; `vault`
  is read-only and must be `pool.vault` (`WrongAccount`), for the NAV the cap is measured
  against.
- **Checks, in order:** not paused; `line_id == pool.line_count` (`WrongAccount`);
  `borrower != pool.admin` (`BorrowerIsAdmin`); `borrower != pool.attestor`
  (`BorrowerIsAttestor`); `principal > 0` (`WrongAmount`);
  `principal <= (vault.amount + outstanding_principal) * max_line_bps / 10 000`
  (`LineTooLarge`; the NAV counts principal lent out, and an empty pool opens no line);
  `rate_bps <= 10 000` (`WrongAmount`); `term_end > now` (`TermEnded`); `agreement_hash !=
  [0; 32]` (`AgreementHashRequired`); `expediente_root != [0; 32]` (`ExpedienteRootRequired`);
  `sponsor_role <= 1` (`SponsorRoleInvalid`); if `sponsor` is
  `Some(k)`, the `sponsor` account must be present and be `k` (`SponsorRequired`); if
  `schedule` is `Some`, `validate_schedule`: `1 <= installments <= 120`,
  `interval_seconds > 0`, `late_bps <= 10 000`, `3 <= max_missed <= installments`,
  `first_due_at > now` (all `InvalidSchedule`);
  `last_due = first_due_at + (installments - 1) * interval <= term_end`
  (`ScheduleEndsAfterTerm`); and `principal + tranche_interest(principal, rate_bps, last_due -
  now) >= installments`, one unit per installment if the whole commitment were drawn now
  (`InvalidSchedule`). Nothing about the dates is re-checked at the draw: the calendar is the
  contract's.
- **Writes:** every `CreditLine` field; `status = Approved`; `interest_due = 0`; `drawn = 0`,
  `draws = 0`, `pagares_paid = 0`; the schedule terms (`first_due_at`, `interval_seconds`,
  `installments`, `late_bps`, `max_missed`; all zero on a bullet line), `late_collected = 0`,
  `late_paid_until = 0`; `availability_end = first_due_at` on a scheduled line, `term_end` on a
  bullet line; `sponsor_role`, `expediente_root`; `reserved` zeroed; `pool.line_count += 1`.
- **CPIs:** none that move tokens.
- **Events:** `LineOpened`, with every term: `borrower`, `sponsor`, `sponsor_role`,
  `agreement_hash`, `expediente_root`, `principal`, `rate_bps`, `term_end`, `availability_end`,
  `first_due_at`, `interval_seconds`, `installments`, `late_bps`, `max_missed` (the schedule
  fields zero on a bullet line).

The borrower's signature is what binds the on-chain line to a person who agreed to the
`agreement_hash`; the attestor's signature is the agreements engine stating that the signed
agreement exists and matches the arguments; the sponsor's signature records which kind of
promise it made (`sponsor_role`) and grants no authority afterwards (a sponsor repays through
`repay` like anyone else).

### 5.5 `draw(amount, document_hash, asset_bump, pagare_bump, cert_fingerprint, signature_method)`

- **Signers:** `borrower`, `attestor` (the engine attests the pagaré behind `document_hash` was
  signed and is conserved; no money moves without it) and `payer`, any signer that funds the
  rent of the pagaré asset and its sidecar (the app's relayer, since SME wallets rarely hold
  SOL).
- **Constraints:** `line` has `has_one = pool` and `has_one = borrower` (`Unauthorized`);
  `asset_mint` (read-only) and `vault` match the pool; `borrower_asset_account` has the pool's
  asset mint and is owned by the borrower; `collection` is `pool.collection` (`WrongAccount`);
  `mpl_core_program` is the Core program id; `asset` is at `["pagare_asset", line, line.draws]`
  and `pagare` at `["pagare", line, line.draws]` with the bumps the client passed (Anchor's
  `ConstraintSeeds` on a wrong address or bump). With the bump given, each constraint runs one
  `create_program_address` and never searches: the compute of a draw does not depend on the
  line's key. The price is that a valid non-canonical bump is accepted too (§9).
- **Checks, in order (all before any write):** not paused; `status` is `Approved` or
  `Active` (`LineStatus`); `0 < amount <= principal - drawn` (`WrongAmount`);
  `now < availability_end`, strictly (`AvailabilityEnded`: an installment counts as due at the
  second it falls due); on a scheduled line `drawn + interest_due >= installments` after the
  tranche (`InvalidSchedule`), so every installment keeps at least one unit; `draws <
  MAX_DRAWS` (`TooManyDraws`); `amount <= vault.amount` (`InsufficientLiquidity`);
  `document_hash != [0; 32]` (`DocumentHashRequired`); `signature_method <= 2` and
  `cert_fingerprint != [0; 32]` unless the method is 0 (`SignatureMethodInvalid`, C6a).
- **CPIs:** `transfer_checked` the tranche vault → borrower (pool signs); then, for the asset
  and for the sidecar alike, a system `transfer` to `payer` of whatever lamports sit on the
  address, signed by the PDA (anyone can fund a predictable address ahead of the draw and both
  `create_account`s refuse a funded one, which would otherwise let a stranger block every draw
  of a line for the price of rent); Metaplex Core `CreateV2` of the pagaré asset into the
  collection with the plugins of §2.3 (the pool PDA signs as authority and owner, the asset PDA
  for its own address, `payer` funds it); `create_account` of the sidecar by hand (Anchor's
  `init` always searches for the bump).
- **Writes:** `pool.outstanding_principal += amount`; `line.drawn += amount`; `line.draws += 1`;
  `line.interest_due += amount * rate_bps * (last_due - now) / (10 000 * SECONDS_PER_YEAR)`
  (fixed at the draw, rounded down once per tranche); on the first tranche `status = Active` and
  `drawn_at = now`; the `Pagare` sidecar (`line`, `draw_index`, `asset`, `pagare_hash =
  document_hash`, `amount`, `issued_at = now`, `due_at = last_due`, `drawn_through = drawn`
  after the tranche, `cert_fingerprint`, `signature_method`, status outstanding, `reserved`
  zero, and the registry opened with the issuance entry: `log_root = sha256(0 || entry_hash)`,
  `entry_count = 1`, `last_kind = Emision`). `Pool::check` runs on the reloaded vault.
- **Events:** `LineDrawn` (`draw_index`, the tranche as `amount`, and the line after it:
  `drawn`, `available`, `interest_due`, `availability_end`; and the pool's
  `outstanding_after`), then `PagareIssued` (`draw_index`, `asset`, `pagare_hash`, `amount`,
  `due_at`, `holder` = the pool PDA), then `PagareEntry` (the issuance entry: kind `Emision`,
  from and to the pool, the pagaré's hash, `entry_count` 1, both dates `now`).
- **Compute:** 80 627 CU on the first tranche and 80 802 on the second at 120 installments,
  deterministic, of which the Core CPI is about 37 000 (`tests/compute.rs`; the ceiling is
  100 000, so some 19 000 CU of room remain before clients would need a compute-budget request;
  the attributes are the lever if it comes to that).

### 5.6 `repay(amount: u64, receipt_hash: [u8; 32])`

- **Signers:** `payer`, anyone. The event records who paid.
- **Constraints:** `line` has `has_one = pool`; `asset_mint` and `share_mint` (read-only) and
  `vault` match the pool; `payer_asset_account` has the pool's asset mint and is owned by the
  payer. **Remaining accounts:** the sidecars `[pagares_paid ..= last_touched]`, writable, oldest
  first, where `pagares_paid` is the line's cursor and `last_touched` the first pagaré whose
  `drawn_through` exceeds `principal_repaid` after this payment, or the last draw when none
  does; each is loaded as a `Pagare` (program-owned, with the discriminator) and its `line`,
  `draw_index` and outstanding status checked: only `draw` writes such an account, at the seeds
  address, so those facts identify the sidecar without recomputing its address (1 500 CU each).
  Anything else (none when one is due, one when none is due, a gap, a repeat, another line's,
  too many or too few) is `PagareAccountsMismatch`, before any write. A line whose cursor has
  passed its last draw, or that never drew (the closing repayment), passes none. The client
  computes the set as `clients/js/src/pagare.ts::repaySidecars` does.
- **Checks, in order:** `status` is `Active` or `InCollection` (`LineStatus`); `remaining =
  (drawn - principal_repaid) + (interest_due - interest_repaid)`; `amount > 0`
  (`WrongAmount`); `late = arrears(now).late_due`, on a bullet line as one installment due at
  `term_end` (from `term_end + grace`, or from the mark if accelerated before);
  `amount >= late` (`LateInterestUnpaid`); while the line can still draw (`Active`, `now <
  availability_end` and something undrawn), a payment on a zero debt is refused
  (`WrongAmount`); `rest = min(amount - late, remaining)`; `interest_part = min(rest,
  interest_left)`; `principal_part = rest - interest_part`; then the remaining accounts. One order
  for every line: late interest, then interest, then principal. **The transfer is `late +
  interest_part + principal_part`, which can be less than `amount`:** no line takes more than
  is due and the surplus stays in the payer's account
  (`tests/schedule.rs::late_interest_is_capped_and_overpayment_stays_with_the_payer`,
  `tests/lines.rs::repay_clamps_overpayment_and_rejects_wrong_status`). The line closes
  (`Repaid`) only when nothing is due and nothing can be drawn: the window has closed, the
  commitment is fully drawn, or the line is in collection. While it can still draw, a payment
  that clears the debt leaves it `Active` and the undrawn remainder stays drawable; once it
  cannot, a payment on a zero debt closes the line and moves nothing: the **closing repayment**,
  a `LineRepaid` with zero parts and `remaining = 0`, which indexers and the app treat as the
  close it is, not as an error (otherwise a third party could pay one unit and forfeit the
  borrower's commitment).
- **CPIs:** one `transfer_checked` payer → vault (payer signs) for the computed total, when it is
  not zero.
- **Writes:** `pool.outstanding_principal -= principal_part`;
  `pool.interest_collected += interest_part` (late interest is *not* counted here);
  `line.principal_repaid`, `line.interest_repaid`; on a scheduled line, when `late > 0`,
  `late_collected += late` and `late_paid_until = now`, which stops the same seconds being
  charged twice. **When `late` is zero the watermark does not move:** the accrual that rounded
  down to zero keeps running, so a borrower cannot erase it by paying one unit every few
  seconds. If `remaining` reaches zero and the line cannot draw any more, `status = Repaid` and
  `closed_at = now`. **The payment entries (LGTOC 130, "en un solo acto"):** for each sidecar
  passed, oldest first, `principal_applied` is the part of this payment's principal that falls
  on that pagaré's range (up to its `drawn_through`). A sidecar with `principal_applied == 0`
  gets no entry. Otherwise the entry goes from the payer to the pool over `receipt_hash` (the
  payer's, unverified; zero means "receipt to follow", repaired by `annotate_payment`): `Pagado`
  where `drawn_through <= principal_repaid` after the payment, which also sets `status =
  PAGARE_PAID_PENDING_CANCEL`, `paid_at = now`, `receipt_hash` and `cancel_grace_seconds` (the
  pool's, fixed for the borrower's exit) (the PAGADO mark at the moment of payment; only the
  burn waits for `cancel_pagare`) and moves the cursor `pagares_paid` past
  it; `Abono` on the one the payment reaches without covering. So a covered pagaré is never left
  without its mark, and an interest-only payment writes no entry. `Pool::check` runs on the
  reloaded vault and the share supply.
- **Events, in order:** one `PagareEntry` per entry; `PagarePayment` whenever any sidecar was
  passed (`payer`, `receipt_hash`, the split `late_part`, `interest_part`, `principal_part`, and
  `entries`: one `(draw_index, kind, entry_count, principal_applied)` per entry written, empty
  for an interest-only payment); `LateInterestCollected` (only when `late > 0`); `LineRepaid`
  with `remaining` after this payment (without late interest) and `nav_after`; `NavUpdated`.
- **Compute:** at 120 installments with every installment missed, 62 015 CU; after an early
  mark, 53 169; a payment covering five sidecars with every installment missed, 91 455; at 18
  installments, 34 967 (`tests/compute.rs`). At most three sidecars per `repay`
  (`state::MAX_SIDECARS_PER_REPAY`, mirrored in `clients/js/src/pagare.ts`); the program refuses
  more with `TooManySidecars`, and a payment that would touch more is split into several `repay`
  calls (§9). Three sidecars with every installment missed stay at least 5 000 CU under the
  ceiling.

Because `late_due` is computed at execution time, a client that quotes it a few seconds
earlier can be short by a few units and hit `LateInterestUnpaid`; clients pad the quote
(`tests/known_limits.rs::limit_late_interest_is_settled_at_execution_time_so_clients_must_pad`,
`clients/js/src/schedule.ts` computes it the same way as the program). The engine's own `repay`
calls carry the right receipt hash: the receipt (amount, date, pagaré, payer) is prepared before
the transaction and its hash goes in the argument; its NOM-151 constancia is obtained after
confirmation, and a failed transaction voids the receipt. Third-party payments get a `Recibo`
entry afterwards (§5.20).

### 5.7 `mark_in_collection()`

- **Signers:** none required beyond the fee payer. Anyone, also while paused.
- **Constraints:** `line` has `has_one = pool`.
- **Checks, in order:** the line has a schedule (`NotScheduled`); `status == Active`
  (`LineStatus`); `arrears(now)`; `k = covered + max_missed <= installments`
  (`NotEnoughMissed`: with `covered` installments paid, the `max_missed`-th missed one is
  installment `k`); `now > anchor + k * interval + grace`, with `anchor = first_due_at -
  interval` (`NotEnoughMissed`: the grace of the last missed installment has run).
- **Writes:** `status = InCollection`, `accelerated_at = now` (vencimiento anticipado,
  decision 28): from the mark every installment is due, `overdue` is the whole unpaid balance and
  late interest runs on all remaining principal (§6). There is no cure after the mark: paying the
  installments that were missed before it leaves the others missed, exigible since the mark, and
  the line leaves collection only by full repayment or write-off. The ordinary interest of the
  installments not yet due is forgiven: `interest_due` becomes
  `interest_due * due_count / installments` rounded down, with `due_count` the installments due
  at the mark under the calendar and at least `covered + max_missed`, and never less than the
  interest already repaid (`math::accelerated_interest`, one function, so a different ruling
  changes one place). No NAV effect: interest counts when it is collected. Example: 6.0 at 12% in
  six installments, marked with three due, keeps 0.36 of the 0.72 and forgives 0.36.
- **Events:** `LineInCollection` with `missed`, `interest_forgiven` and `remaining`
  (principal and interest after the forgiveness, no late interest).
- **Compute:** 34 769 CU at 120 installments with every installment missed, 12 336 with three
  missed (`tests/compute.rs`).

### 5.8 `write_off()`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`). Not blocked by the pause.
- **Constraints:** `line` has `has_one = pool`; `vault` is read only for the NAV in the event.
- **Checks:** `after_term = now > term_end + grace`. If `status == Active`: require
  `after_term` (`TermNotEnded`). If `status == InCollection`: require `after_term` **or**
  (`arrears(now).first_missed_due` is `Some(due)` and `now > due + grace`), **and** the
  collection period has run, `now > accelerated_at + collection_period_seconds`
  (`WriteOffTooEarly` for either: the grace of the third missed installment implies the grace
  of the first, so without the period the write-off would be available the instant the line is
  marked). Any other status: `LineStatus`.
  After the mark `first_missed_due` is the instant the oldest unpaid installment became exigible:
  its due date, or the mark for an installment that fell due only by the acceleration, so once
  the pre-mark arrears are paid the write-off waits for the grace counted from the mark.
- **Writes:** `loss = drawn - principal_repaid` (an approved line never drawn has nothing to
  book and is not written off: `LineStatus`); `pool.outstanding_principal -= loss`;
  `pool.written_off += loss`; `status = WrittenOff`; `closed_at = now`.
- **CPIs:** none. No tokens move; the NAV drops by `loss` because `outstanding_principal` does.
- **Events:** `LineWrittenOff` with `loss` and `nav_after`, then `NavUpdated`.
- **Compute:** 48 614 CU at 120 installments with every installment missed and the collection
  period run (`tests/compute.rs`).

Unpaid interest is never part of the loss: it was never in the NAV.

### 5.9 `set_paused(paused: bool)`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`).
- **Writes:** `pool.paused`. **Events:** `PoolPaused`.

### 5.10 `set_attestor()`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`) and `new_attestor`, the key that
  becomes the attestor: it signs, which proves the engine controls it (C14; the holder-facing
  and court-facing parts of a rotation stay off chain). A zero or uncontrolled key can therefore
  never be named.
- **Checks:** `new_attestor != admin` (`AttestorIsAdmin`).
- **Writes:** `pool.attestor = new_attestor`. **Events:** `AttestorSet`.

The attestor can block every opening, draw, endoso, court annotation and attested cancellation
by withholding its signature and can move nothing; this instruction is the recovery when its
key rotates or is lost.

### 5.11 `suspend_draws()`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`).
- **Constraints:** `line` has `has_one = pool`.
- **Checks:** `status` is `Approved` or `Active` (`LineStatus`).
- **Writes:** `availability_end = now`. Nothing else: repayment, collection and
  write-off are untouched, and the pool is not paused.
- **Events:** `LineDrawsSuspended` with `available_forfeited = principal - drawn`.

### 5.12 `cancel_line()`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`).
- **Constraints:** `line` has `has_one = pool`.
- **Checks:** `status == Approved` and `drawn == 0` (`LineStatus`): once a tranche exists
  something is owed and the line follows repayment, collection or write-off.
- **Writes:** `status = Cancelled` (terminal), `closed_at = now`. The line id stays taken.
- **Events:** `LineCancelled`.

### 5.13 `cancel_pagare(draw_index: u16, receipt_hash: [u8; 32])`

Condition C22 (LGTOC 17 and 129, CCF 2088): the extinction of a pagaré is an act of its holder
against a receipt, not a permissionless burn, and the record of the payment must outlive the
título.

- **Signers:** `holder`, who extinguishes the title: the pagaré's holder, which while the asset
  is in the pool is `pool.admin`, the vehicle's signer, or the procurador of record,
  `Pagare.procurador` with the procuración in force (any other signer: `NotHolder`); `attestor`,
  optional (`AttestorRequired` when absent unless the backstop below applies: the engine attests
  that the FEA-signed receipt behind `receipt_hash` exists and is conserved, and that it archived
  the pagaré's registry under NOM-151 before asking for the burn); `payer`, any signer, which
  receives the asset's rent from the Core program (the relayer that funded it at the draw, as a
  rule).
- **Constraints:** `line` has `has_one = pool`; `pagare` is at `["pagare", line, draw_index]`
  with its stored bump (`ConstraintSeeds`) and has `has_one = line` (`WrongAccount`); `asset` is
  `pagare.asset` (`WrongAccount`: the sidecar, not a derivation, names the asset, which is why a
  non-canonical bump at the draw is tolerable, §9); `collection` is `pool.collection`;
  `mpl_core_program` is the Core program id; `procurador_authority`, optional, is on the
  procurador's branch the program address `["procurador", pagare, holder]` the burn delegate
  was approved to, derived and compared in the handler (`WrongAccount` when missing or another),
  and left out on the owner's. A line that never drew has no sidecar, so a cancellation on a
  `Cancelled` line fails on the account (`AccountNotInitialized`).
- **Checks, in order (all before any write):** the branch, when the signer is not the admin
  (`NotHolder`, then `ProcuracionState` if the recorded procurador had no procuración in force,
  unreachable while the two are cleared together); the title is not burnt (`PagareAlreadyPaid`:
  paid is terminal and the asset is gone); `receipt_hash != [0; 32]` (`DocumentHashRequired`);
  the PAGADO mark a repayment wrote, `status == PAGARE_PAID_PENDING_CANCEL`, whatever the line's
  status (`PagareNotCovered` while outstanding); the attestor, or the backstop; on the
  procurador's branch, `procurador_authority`. The mark is the coverage rule: `repay` marks the
  pagarés oldest first as its principal reaches each `drawn_through` (`drawn_through` is the
  line's drawn after the tranche), so a later pagaré is never marked while an earlier one is
  uncovered, and among marked ones the cancellation order is free. A pagaré a payment covered
  cancels in collection or after a write-off like any other; the uncovered ones stay in force for
  the collection. Interest covers nothing by itself: `repay` settles interest before principal,
  so when the principal of pagarés `0..=k` is repaid every unit of interest fixed up to that
  payment is too, and a título is never extinguished while any of its principal is owed. The
  program does not check that the receipt differs from other receipts: the engine issues one per
  pagaré and the attestor's signature is the guard. Not blocked by the pause: it records a
  payment and moves no funds.
- **Backstop (no attestor account):** accepted only on the owner's branch when a request is
  pending (`cancel_after != 0`), its deadline has passed (`now > cancel_after`) and
  `receipt_hash` equals `requested_receipt_hash`; otherwise `AttestorRequired`. The same checks
  as above apply. See `request_cancellation` (§5.18).
- **Writes:** the `Terminacion` entry written to the sidecar's chain (from the signer's role,
  the pool or the procurador, to the pool, over `receipt_hash`); `status = PAGARE_PAID` (the
  payment's `paid_at` and `receipt_hash` stay as the repayment or the holder's annotation left
  them), `cancel_receipt_hash = receipt_hash`; a pending request cleared (`cancel_after = 0`,
  `requested_receipt_hash` zeroed); a procuración in force ends with the title (`flags` bit 1
  cleared, `procurador` zeroed; bit 0 stays as the plugin's history). Nothing else changes and
  the account is never closed (its rent stays locked, a cost of every draw).
- **Events:** `PagareEntry` with the `Terminacion` entry, then `PagareCancelled` (`pool`,
  `line_id`, `draw_index`, `asset`, `receipt_hash`, `holder`: the key that extinguished it, the
  admin or the procurador, as `PagareIssued` names the first holder; `backstop`: true when the
  holder cancelled alone after a request's deadline, without the attestor). The engine reads and
  archives the registry off chain, from the events, before the burn.
- **CPIs, last:** Metaplex Core `UpdatePluginV1` thawing the asset (the pool PDA as the
  collection's authority, which holds the freeze delegate), then `BurnV1` with the pool PDA as
  the owner or, on the procurador's branch, with `procurador_authority` as the authority, the
  program signing for it (it holds the owner-managed `BurnDelegate` the endoso approved; nobody
  has its key, so the burn happens only here); the collection, `payer`, the system program. Core
  leaves a one-byte `Uninitialized` tombstone at the address (so it cannot be re-created as an
  asset) and sends the rest of the rent to `payer`; the collection's `current_size` drops by one
  and `num_minted` stays.
- **Compute:** on a 120-installment line, 48 935 CU as the owner with the attestor, 48 659 by the
  holder alone after a request's deadline, and 53 500 to 58 400 through the procurador's address
  (`tests/compute.rs`; the thaw is about 17 000 of it); no arrears loop runs. The procurador's
  branch derives its address with `find_program_address`, so its cost varies with the
  procurador's key by 1 500 CU per extra bump attempt.

### 5.14 `endorse_pagare(draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32])`

The endoso en procuración (LGTOC 35), the only kind the pool makes (decision 29).

- **Signers:** `holder`, the pagaré's holder, which is `pool.admin` while the asset is in the pool
  (`NotHolder`); `attestor` (`AttestorRequired`: it attests the signed endoso behind
  `endoso_hash` and the procurador's verification, KYB done off chain by the engine; no SAS
  account is read); `payer`, any signer, for the rent of the plugin Core adds.
- **Constraints:** `line` has `has_one = pool`; `pagare` is at `["pagare", line, draw_index]` and
  has `has_one = line`; `asset` is `pagare.asset`; `collection` is `pool.collection`;
  `mpl_core_program` is the Core program id.
- **Checks, in order (all before any write):** `kind == KIND_PROCURACION` (`EndosoKindInvalid`);
  `to` is not the zero key, the pool, the attestor or the admin (`EndorseeInvalid`: the
  procurador is a third party, and naming the attestor would collapse the two signatures its
  cancellation needs into one key); `endoso_hash != 0` (`DocumentHashRequired`); the line is not
  `WrittenOff` (`LineStatus`); the pagaré is outstanding (`PagareAlreadyPaid`); no procuración is
  in force (`ProcuracionState`, `flags` bit 1).
- **Writes:** the `Procuracion` entry written to the sidecar's chain (from the pool, to `to`,
  over `endoso_hash`, `now`), `flags` bits 0 and 1 set, `procurador = to`.
- **CPI:** `AddPluginV1` of an owner-managed `BurnDelegate` whose authority is the program PDA
  `["procurador", pagare, to]`, the pool PDA signing as the owner; when `flags` bit 0 says the
  plugin exists (a procuración was revoked before), `ApprovePluginAuthorityV1` to that PDA instead
  (Core refuses a second `AddPluginV1` with `PluginAlreadyExists`). Nobody holds a key for the
  PDA, so the procurador's own signature cannot burn (pinned in `tests/core_capabilities.rs`);
  only `cancel_pagare` signs for it, with the receipt and the PAGADO mark. The owner stays the
  owner; nothing moves.
- **Events:** `PagareEntry` with the entry's content.
- **Compute:** at 120 installments, 33 540 to 38 040 CU on the add path and 40 712 to 45 212 on
  the approve path (`tests/compute.rs`). The handler derives the delegate's address with
  `find_program_address`, so the figure varies with the keys by 1 500 CU per extra bump attempt.

### 5.15 `revoke_procuracion(draw_index: u16, entry_hash: [u8; 32])`

- **Signers:** `holder` (`pool.admin` while the asset is in the pool, `NotHolder`), alone: the
  revocation is the owner's act (LGTOC 41); `payer`, any signer.
- **Constraints:** as `endorse_pagare`.
- **Checks:** `entry_hash != 0` (`DocumentHashRequired`, the signed revocation's hash); a
  procuración is in force (`flags` bit 1, else `ProcuracionState`).
- **Writes:** the `Revocacion` entry (from the pool, to the recorded `procurador`); `flags` bit 1
  cleared; `procurador` zeroed.
- **CPI:** `RevokePluginAuthorityV1` on the `BurnDelegate`, the pool PDA signing as the owner:
  the plugin stays with authority `Owner` (`flags` bit 0 stays), so the next procuración approves
  it instead of adding it.
- **Events:** `PagareEntry`.
- **Compute:** 37 144 CU at 120 installments (`tests/compute.rs`).

### 5.16 `court_annotation(draw_index: u16, kind: u8, document_hash: [u8; 32])`

A court's act on the title, recorded by the holder with the attestor's co-signature.

- **Signers:** `holder` (`pool.admin` while the asset is in the pool, `NotHolder`); `attestor`
  (`AttestorRequired`: it attests the court document behind `document_hash`). No payer: nothing
  is created.
- **Constraints:** `line` has `has_one = pool`; `pagare` is at `["pagare", line, draw_index]` and
  has `has_one = line`. No asset or Core account: nothing moves.
- **Checks:** `kind` is `AnotacionJudicial` (6: embargo, suspension, cancellation) or
  `ConstanciaJudicial` (7, art. 28), else `AnnotationKindInvalid`; `document_hash != 0`
  (`DocumentHashRequired`); the title is not burnt (`status != PAGARE_PAID`, else
  `PagareAlreadyPaid`). The line's status is not checked: a court acts on a written-off title and
  on a paid one awaiting its burn alike, and a procuración in force is untouched.
- **Writes:** the entry written to the sidecar's chain (`kind`, from = to = the pool, since a
  court's act has no counterparty, `document_hash`, `now`). Nothing else.
- **Events:** `PagareEntry`.
- **Compute:** 14 776 CU at 120 installments (`tests/compute.rs`).

### 5.17 `set_collection_period(collection_period_seconds: u32)`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`).
- **Checks:** `collection_period_seconds >= MIN_COLLECTION_PERIOD_SECONDS` (30 days;
  `CollectionPeriodTooShort`).
- **Writes:** `pool.collection_period_seconds`. Applies to every line from its next `write_off`
  check, marked or not. **Events:** `CollectionPeriodSet`.

### 5.18 `request_cancellation(draw_index: u16, receipt_hash: [u8; 32])`

The backstop on the attestor (LGTOC 17 and 129): the debtor is never left without receipt or
title if the attestor fails. Rotating the attestor (`set_attestor`) stays the primary recovery;
the backstop does not depend on it.

- **Signers:** `holder` (`pool.admin` while the asset is in the pool, `NotHolder`), alone.
- **Constraints:** as `court_annotation`: `pool`, `line` (`has_one = pool`), `pagare` (seeds,
  `has_one = line`); no asset or Core account.
- **Checks:** the same as `cancel_pagare` (not burnt, `receipt_hash != 0`, the PAGADO mark
  `PAGARE_PAID_PENDING_CANCEL` whatever the line's status), so a request cannot be filed before
  the title is paid. A new request replaces a pending one, so a wrong receipt cannot block the
  backstop: it takes the new receipt and restarts the wait, but never brings the deadline
  earlier than the one it replaces.
- **Writes:** the `SolicitudCancelacion` entry (from = to = the pool, over `receipt_hash`);
  `cancel_after = max(now + pool.cancel_delay_seconds, cancel_after)` (the delay in force at the
  request: a later `set_cancel_delay` never moves a pending deadline);
  `requested_receipt_hash = receipt_hash`. Any cancellation clears both. In this vehicle the
  holder is the pool, so the backstop is the admin cancelling alone after the delay.
- **Events:** `PagareEntry`, then `CancellationRequested` (`pool`, `line_id`, `draw_index`,
  `asset`, `receipt_hash`, `cancel_after`), so an indexer reads the deadline instead of
  reconstructing it from the delay's history.
- **Compute:** 17 257 CU at 120 installments (`tests/compute.rs`); the holder's cancellation
  alone afterwards 48 659 CU.

### 5.19 `set_cancel_delay(cancel_delay_seconds: u32)`

- **Signers:** `admin` (`has_one = admin`, `Unauthorized`).
- **Checks:** `cancel_delay_seconds >= MIN_CANCEL_DELAY_SECONDS` (7 days;
  `CancelDelayTooShort`).
- **Writes:** `pool.cancel_delay_seconds`. Applies to requests filed from now on; a pending
  request keeps the deadline it stored. **Events:** `CancelDelaySet`.

### 5.20 `annotate_payment(draw_index: u16, entry_count: u32, receipt_hash: [u8; 32])`

The repair path of the payment entries: a `repay` carries the payer's receipt hash, which a third
party may leave zero or wrong; the holder or the attestor attaches the real receipt afterwards.

- **Signers:** `annotator`, `pool.admin` or `pool.attestor` (`Unauthorized`).
- **Constraints:** `pool`, `line` (`has_one = pool`), `pagare` (seeds, `has_one = line`); no
  asset or Core account.
- **Checks:** `receipt_hash != 0` (`DocumentHashRequired`); `1 <= entry_count <=
  pagare.entry_count` (`EntryNotFound`): the referenced entry exists. The chain keeps no kinds of
  past entries, so that it is a payment entry is the engine's to check. Any title status,
  burnt included: the sidecar persists and a receipt may arrive late.
- **Writes:** the `Recibo` entry (from the annotator, to the pool, over `receipt_hash`). When the
  holder signs and the title is `PAGARE_PAID_PENDING_CANCEL`, also `pagare.receipt_hash`: the
  payer's hash on the PAGADO mark is provisional until then (a stranger's covering payment can put
  any hash there). After the burn `receipt_hash` stays as the repayment or the holder's annotation
  left it (the cancellation's own receipt is `cancel_receipt_hash`), and the attestor's annotation
  never touches it.
- **Events:** `PagareEntry`, then `PagareReceiptAnnotated` (`draw_index`, `entry_count` of the
  `Recibo`, `payment_entry_count` referenced, `receipt_hash`).

## 6. Installments, arrears and late interest (`math.rs`)

A scheduled line stores its terms and its repayments; everything else is recomputed on every
call by `arrears(terms, paid, now)`. Nothing about arrears is stored, so there is no state to
drift.

**Due dates.** The due dates count from an *anchor*, one interval before the first due date,
`anchor = first_due_at - interval`, fixed by the signed contract. Installment `k` (1-based) is
due at `anchor + k * interval_seconds` (so installment 1 at `first_due_at`), and at that very
second it already counts as due. `installments_due(now)` is `min(installments, (now - anchor) /
interval)`, zero before the first due date.

**Installment size.** `total = drawn + interest_due`: what was drawn plus the interest fixed at
each draw. Each installment is `total /
installments` rounded down and the last one carries the remainder, so `cumulative_due(k)` is
`k * base` for `k < installments` and exactly `total` for the last. Example: 1 000 003 in six is
five of 166 667 and one of 166 668.

**Installments covered.** `installments_covered(paid)` is `paid / base`, capped at
`installments - 1` unless `paid >= total`. Paying 6 × 166 667 = 1 000 002 of the example above
covers five installments, not six: the last one is only covered by paying the total. Late
interest never pays an installment (`paid` is principal + ordinary interest only).

**Missed** is `due - covered` when positive; the grace period does not change the count, it
only delays late interest and collection.

**Acceleration (decision 28).** `mark_in_collection` sets `accelerated_at`. From then on `due =
installments` whatever the clock says, so `overdue` is the whole unpaid balance and `missed =
installments - covered`; installment `k` becomes exigible at `min(due_k, accelerated_at)` and its
late interest runs from `max(min(due_k + grace, accelerated_at), late_paid_until)`: one already
overdue keeps its own start and grace, one not yet due starts at the mark with no further grace
(the grace was consumed by the third missed installment). `first_missed_due` follows the exigible
instant. The loop then runs over every remaining installment, which is the shape `tests/compute.rs`
already bounds.

**Late interest** accrues on the principal share of each overdue part `part_k` separately
(decision 11: no interest on overdue interest), from `start_k = max(due_k + grace,
late_paid_until)` until `now`, linearly at `late_bps` a year of 365 days:

```
p_k      = part_k * drawn / total                            the principal share, rounded down
late_due = Σ_k p_k * min(now - start_k, cap_k) * late_bps / (10 000 * SECONDS_PER_YEAR)
cap      = ceil(10 000 * SECONDS_PER_YEAR / late_bps)      seconds after which a share would
                                                             exceed itself
```

Every installment repays principal and interest in the proportion of the whole line, so the
share is the same whatever order the repayment allocation follows; it depends only on the total
paid. This is the reading that does not punish interest-first allocation: a borrower who paid
part of an installment is not charged as if none of that payment had reached the principal.

The weighted sum is accumulated in `u128` and divided once, so rounding happens once per
settlement. Two caps apply, and they are different things:

- **Per settlement:** `late_due <= overdue_principal` for this repayment; each principal share
  accrues at most itself in one settlement because of `cap`. After a repayment moves
  `late_paid_until`, a part that is still unpaid accrues again, so across several repayments one
  share can accrue more than itself.
- **Over the life of the line:** `late_due <= drawn - late_collected`. Once a borrower has
  paid late interest equal to the principal drawn, no more is charged; every further payment goes to
  interest and principal.

`arrears` returns `Arrears { due, covered, missed, overdue, overdue_principal, late_due,
first_missed_due }`; `ScheduleTerms.accelerated_at` is zero until the mark.
`repay` uses `late_due`; `mark_in_collection` uses `covered` and `missed`; `write_off` uses
`first_missed_due`. The loop runs at most `installments <= 120` times, which is what
`tests/compute.rs` bounds: the arrears instructions stay under the compute ceiling at 18 and at
120 installments.

## 7. Invariants

What must hold between instructions, and which code keeps it:

1. `NAV == vault.amount + outstanding_principal` by definition; `outstanding_principal` only
   changes in `draw`, `repay`, `write_off` and only by the amount that moved or was lost.
2. `outstanding_principal == Σ (drawn - principal_repaid)` over lines in `Active` or
   `InCollection`. Not checked on chain (it would need every line account); what is checked
   at the end of every money instruction is `Pool::check()`: `vault.amount +
   outstanding_principal` fits a `u64`, on the balance reloaded after the CPIs, else
   `InvariantBroken`.
3. `share_mint.supply >= DEAD_SHARES` once anything was deposited: the dead account's shares are
   never burned (no instruction takes `dead_shares` as a burn source). `Pool::check()` re-checks
   it (`supply == 0 || supply >= DEAD_SHARES`) in `deposit`, `redeem`, `repay` and `write_off`,
   the four instructions that carry the share mint (for `NavUpdated`); `draw`
   runs the NAV leg only.
4. Only `draw` and `redeem` move tokens out of the vault, both signed by the pool PDA, both to
   an account whose owner signed the instruction (the borrower of an admin-approved line, the
   holder burning shares).
5. `drawn <= principal` and `drawn` never decreases; `principal_repaid <= drawn` and
   `interest_repaid <= interest_due`: every line clamps to `remaining`.
   `pool.outstanding_principal == Σ (drawn - principal_repaid)` over `Active` and
   `InCollection` lines. `interest_due` is the sum of the per-tranche figures fixed at each
   draw; it grows only at a draw and shrinks only by the forgiveness of `mark_in_collection`.
6. `late_collected <= drawn` on a scheduled line, by the lifetime cap in `arrears`.
7. Every arithmetic step is `checked_*` or in `u128`; a `None` becomes `Overflow` and the
   transaction fails. `tests/math_props.rs` fuzzes the pure functions with `proptest`.

## 8. Roles

| role | identified by | can |
|---|---|---|
| admin | `Pool.admin`, fixed forever | `open_line` (up to `max_line_bps` of the NAV per line: `LineTooLarge`; any borrower other than itself: `BorrowerIsAdmin`; no limit on the number of lines), `write_off` (only when overdue past grace), `suspend_draws` (close a line's draw window now), `cancel_line` (clear an undrawn approval), as the holder of the pagarés in the pool: `cancel_pagare` (only a marked one, with the attestor and a receipt, or alone after a request's deadline), `request_cancellation`, `endorse_pagare` (en procuración, with the attestor), `revoke_procuracion`, `court_annotation` (with the attestor), `annotate_payment`; `set_paused`, `set_attestor` (never to itself), `set_collection_period` (never below 30 days), `set_cancel_delay` (never below seven days) |
| attestor | `Pool.attestor`, set at `init_pool`, rotated by `set_attestor` (which the new key co-signs) | co-signs `open_line`, `draw`, `endorse_pagare`, `court_annotation` and `cancel_pagare` (except the backstop); signs `annotate_payment` alone. It can block every opening, draw, endoso and court annotation, and delay a cancellation by the cancel delay, and it moves nothing |
| share-mint freeze authority | the freeze authority of the admin-supplied share mint (Token ACL's `MintConfig` in production); not recorded by the program | thaw or freeze share accounts, the dead account included: admits and bars investors |
| borrower | `CreditLine.borrower` | `draw` its own approved line before the term |
| payer (relayer) | any signer passed as the `payer` of `draw`, `cancel_pagare`, `endorse_pagare` or `revoke_procuracion` | pays the rent of the pagaré asset and its sidecar at the draw (and of the plugin at an endoso) and receives the asset's rent back at the cancellation; nothing else |
| procurador | `Pagare.procurador`, named by `endorse_pagare` | `cancel_pagare` on a marked pagaré of its procuración, with the attestor; never alone |
| sponsor | `CreditLine.sponsor` | nothing after co-signing the opening |
| depositor / holder | any signer with the asset or shares | `deposit`, `redeem` |
| anyone | | `repay` any drawn line, `mark_in_collection` |
| upgrade authority | BPF loader authority of the program id | replace the program; a single key on devnet |

There is no operator role, no fee switch, no admin withdrawal and no way to change the asset, the
admin or the grace period after `init_pool`. No account of this program is ever closed; the only
account that disappears is the pagaré asset, burnt by the Core program in `cancel_pagare`.

Besides the token programs, the one foreign program the pool depends on is Metaplex Core
(`CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d`, crate `mpl-core` 0.12.1, tests against the
pinned binary `release/core@0.15.3`). Its upgrade authority, Metaplex, can change how the pagaré
assets behave; the program's own records (the sidecars and the events) do not depend on it.

## 9. Known limits (proved by `tests/known_limits.rs`)

Each of these is a passing test that documents behaviour the program *has*: a guard it applies
or a limit it accepts.

| test | behaviour |
|---|---|
| `admin_cannot_be_the_borrower_of_its_own_line` | `open_line` refuses `borrower == admin` (`BorrowerIsAdmin`) |
| `a_line_for_the_whole_vault_is_refused_under_the_concentration_cap` | a line above `max_line_bps` of the NAV fails with `LineTooLarge` (1,000 bps in the test and in production pools; the harness opens its other pools at 10,000 bps, where the cap is inert). The admin can still reach the whole vault through several lines to wallets it controls, and the NAV the cap is measured against can be inflated by a deposit in the same transaction: the cap bounds one honestly proposed line, not the admin (decision 20) |
| `first_deposit_minimum_is_one_unit_of_the_asset_with_a_dead_shares_floor` | the minimum first deposit is one whole unit of the asset for six or more decimals; below six decimals the floor of 1 000 × `DEAD_SHARES` base units applies, so a two-decimal asset needs 10,000.00 |
| `limit_deposit_just_before_a_repayment_shares_the_interest` | interest counts when collected, so a deposit that lands just before a repayment gets the same uplift as one made months earlier |
| `limit_redeem_between_collection_and_write_off_exits_at_par` | a line in collection still counts fully in the NAV until the admin writes it off; a holder who redeems in between exits at par and the loss falls on the rest |
| `limit_pause_blocks_exit_while_repayments_continue` | while paused, repayments raise the NAV but nobody can redeem |
| `limit_total_write_off_leaves_a_pool_that_cannot_take_deposits` | if every line is written off and the vault is empty, `NAV == 0` with `supply > 0` and `shares_for_deposit` divides by zero: the pool is dead |
| `limit_collection_is_unreachable_when_covered_plus_max_missed_exceeds_installments` | a line with `installments < covered + max_missed` can never enter collection; only the term releases it |
| `limit_late_interest_is_settled_at_execution_time_so_clients_must_pad` | see §5.6 |
| `limit_draw_accepts_a_valid_non_canonical_pagare_bump` | `draw` verifies the pagaré PDAs with the bumps the client passes (one `create_program_address` each, deterministic compute); a valid bump that is not the canonical one is accepted too, and the asset then sits where `find_program_address` never looks. Proving a bump canonical costs the search the client's bump avoids. The attestor derives the canonical bump and co-signs every draw, and `PagareIssued` and the sidecar carry the asset's address, so indexers never derive it |
| `a_repay_takes_at_most_three_pagare_records` (inverted 2026-11-05) | `repay` takes at most `MAX_SIDECARS_PER_REPAY` (3) pagaré records and refuses more with `TooManySidecars`; the client splits a larger payment (the amount is free) into parts of at most three records, each under the 100 000 CU ceiling |
| `limit_a_payment_landing_first_makes_an_in_flight_repay_retry` | `repay` takes exactly the records `[pagares_paid ..= last_touched]`, computed by the client when it builds the transaction. Another payment landing first can change that set, and the transaction fails with `PagareAccountsMismatch` with nothing moved; the client re-reads the line and retries |

Other points outside the test file:

- `init_pool` applies an asset-mint policy (§5.1) and records the issuer controls it accepts in
  `asset_controls`. If the asset issuer freezes the vault or pauses the mint, every instruction
  that moves tokens fails until it is undone; that is accepted and visible to clients (the
  `asset_controls` field of the pool), not refused.
- Token-2022 mints (MEX, CETES) can be the asset. The program uses
  `anchor_spl::token_interface` (`InterfaceAccount`, `Interface<TokenInterface>`) and
  `transfer_checked` everywhere, and every money instruction takes the asset mint read-only for
  its decimals. `tests/pool.rs::a_pool_on_a_token_2022_asset_runs_the_full_cycle` runs the whole
  cycle on a Token-2022 asset.
- Every line repays late interest, then interest, then principal, and clamps overpayment, so a
  line with principal left always has interest left too and a zero-loss write-off cannot forfeit
  the interest.
- Missed installments are counted from amounts paid, not from due dates: a borrower who pays
  one installment of principal and one of interest in a different order is still "covered"
  by amount.
- The cost of the procurador's `cancel_pagare` and of `endorse_pagare` varies with the keys by
  1 500 CU per extra bump attempt of `find_program_address` (§10); it is bounded and stays under
  the ceiling.

## 10. Tests and what each file proves

163 Rust tests in 14 binaries and 11 JS tests.

| file | tests | harness | proves |
|---|---|---|---|
| `src/` (unit: `math.rs`, `state.rs`, `instructions/lines.rs`) | 29 | plain | rounding, caps, installment arithmetic, arrears cases; `Pool::check` and `min_first_deposit`; the on-chain decimal and hex text of the attributes |
| `tests/math_props.rs` | 7 | proptest | the pure functions never panic and keep their bounds over random inputs |
| `tests/vectors.rs` | 1 | plain + `clients/js/test/arrears-vectors.json` | 421 committed numeric vectors (44 of them accelerated) match the program arithmetic (shared with the JS client's tests) |
| `tests/layout.rs` | 1 | LiteSVM | the account sizes (Pool 346, CreditLine 375, Pagare 387 bytes with the discriminator) and the `reserved` tail of each account created zero |
| `tests/pool.rs` | 23 | LiteSVM | init, the asset-mint and share-mint policies, first deposit, pricing after interest, redeem liquidity, wrong accounts, pause, dead-shares inflation defence, rounding and slippage floors, a full cycle on a Token-2022 asset |
| `tests/lines.rs` | 11 | LiteSVM | open/draw/repay full cycle raises the share price, the attestor on openings and draws, sponsor co-sign and role, sponsor pays interest, overpay and wrong status, liquidity, strangers and the pause on draws, write-off lowers the NAV once, bad params, the bullet line's window, the concentration cap |
| `tests/schedule.rs` | 20 | LiteSVM, clock warped | schedule validation, installments on their due dates, missed count over time, late interest after grace, caps, repayment order, collection after three missed, acceleration and the forgiven interest, no cure after the mark, write-off timing in both paths |
| `tests/tranches.rs` | 12 | LiteSVM, clock warped | tranched draws: the strict window, the payability floor, per-tranche interest to the last due date, `MAX_DRAWS`, the draw's document hash, weekly, fortnightly and monthly calendars, a half-drawn line left open by a full repayment, `suspend_draws` and `cancel_line` |
| `tests/known_limits.rs` | 12 | LiteSVM | the twelve tests in §9 |
| `tests/pagare.rs` | 22 | LiteSVM + the pinned Core binary | one frozen asset and one sidecar per draw, held by the pool PDA in the pool's collection, the relayer paying the rent, pre-funded addresses swept; every plugin read back through the `mpl-core` decoders (the crate-vs-program compatibility proof); `drawn_through` and the signature rules; wrong asset, sidecar or collection refused; the registry's issuance entry; the payment entries (`Abono` and `Pagado`, the cursor, the exact sidecar set), coverage oldest first by principal (interest alone covers nothing); `cancel_pagare`: the PAGADO mark required, the burn, cancellation in collection and after a write-off, the holder, the attestor, the receipt and the accounts; the endoso en procuración (entry, flags, the procurador recorded, the burn delegate on the program PDA, the refusals), its revocation, the court annotations (on a live, a written-off and a burnt title) and the procurador's extinction through the program address; the backstop (the cancel delay parameter, the request, the holder alone after the deadline); `annotate_payment` |
| `tests/core_capabilities.rs` | 8 | LiteSVM + the pinned Core binary | what Core allows after creation (§2.3): permanent delegate authorities hand off only by revoke then approve from the collection's authority and `None` is terminal; only the freeze delegate thaws and a frozen asset with no delegate is stuck; an asset's update authority cannot leave its collection and `AddBlocker` closes the rest; `AddBlocker` costs about 4 200 CU at creation; AppData is written by its fixed data authority and removed by the collection's authority, and an Owner-held AppData authority follows the asset through `TransferV1`; owner-managed delegates are the owner's to add, approve and revoke, reset to `Owner` on transfer, and the approved burn delegate burns a thawed asset; a burn delegate approved to a program address refuses the procurador's own key |
| `tests/audit.rs` | 5 | LiteSVM | a third-party repayment does not close a line that can still draw, and a full repayment closes one that cannot; a collection with plugins or minted assets is refused; `write_off` waits for the collection period from the mark; the attestor is neither the admin nor a borrower |
| `tests/reaudit.rs` | 5 | LiteSVM | an interest-only payment appends no registry entry, and the entry counter is wide enough; the attestor and the admin cannot be procurador; the holder corrects the payment receipt until the burn; a new cancellation request replaces a pending one without shortening it |
| `tests/compute.rs` | 7 | LiteSVM | every instruction it drives stays under a 100 000 CU ceiling (below) |
| `tests/common/mod.rs` | | | the LiteSVM fixture: loads `target/deploy/elote_pool.so`, builds every instruction, warps the clock |
| `clients/js/test/*.test.ts` | 11 | `tsx --test` | the arrears vectors replayed against `schedule.ts` (5) and the pagaré helpers of `pagare.ts` (6) |

**Compute** (`tests/compute.rs`, at 120 installments unless noted; the ceiling is 100 000 CU,
below the 200 000 a transaction gets per instruction without a compute-budget request):

| instruction | CU |
|---|---|
| `init_pool` | 34 543 to 42 043 |
| `draw` | 80 627 (second tranche 80 802), of which the Core CPI is about 37 000 |
| `repay`, every installment missed | 62 015 |
| `repay` after an early mark | 53 169 |
| `repay` covering five records, every installment missed | 91 455 |
| `repay` at 18 installments | 34 967 |
| `mark_in_collection`, every installment missed | 34 769 (3 missed: 12 336) |
| `write_off`, every installment missed | 48 614 |
| `cancel_pagare` by the owner, with the attestor | 48 935 |
| `cancel_pagare` by the holder alone (backstop) | 48 659 |
| `cancel_pagare` by the procurador | 53 514 to 58 379 |
| `request_cancellation` | 17 257 |
| `endorse_pagare`, plugin added | 33 540 to 38 040 |
| `endorse_pagare`, plugin approved | 40 712 to 45 212 |
| `revoke_procuracion` | 37 144 |
| `court_annotation` | 14 776 |

The arrears loop is the only loop in the program and runs at most `installments <= 120` times;
with every installment missed, every installment takes the accrual branch. `draw` is
deterministic, since the client passes the PDA bumps. The figures that involve a
program-address search vary with the keys: `init_pool` (its `init`s) and `endorse_pagare` (the
delegate's address), by 1 500 CU per extra attempt. The procurador-branch `cancel_pagare` cost varies by up to a few 1 500 CU
steps with the procurador's key (the `find_program_address` bump search); it is bounded, and the
code is unchanged for it.

```sh
NO_DNA=1 anchor build --ignore-keys --arch v0      # programs → target/deploy/elote_pool.so + target/idl
cargo test -p elote_pool                           # 163 tests, LiteSVM in-process, no validator
npm run test:client                                # 11 JS tests
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
                                   clients/js/src/schedule.ts  (hand-written: arrears, mirrors math.rs;
                                                                lineSchedule, maxLinePrincipal)
                                   clients/js/src/pagare.ts    (hand-written: the asset PDA, whose seed
                                                                is a field of the line account, the
                                                                draw account helper, repaySidecars,
                                                                MAX_SIDECARS_PER_REPAY)
                                   clients/js/src/index.ts
                                                     │
                                                     ▼
                                   @softlaw/elote-pool-client  (npm, trusted publishing on tag client-v*)
```

`npm run check:client` regenerates the client and fails if the committed output differs, so the
IDL in `idl/` and the client cannot drift from the program silently. The generated client decodes
every typed field of the accounts and events; `lineSchedule(line)` returns a line's schedule or
`null` for a bullet line, and anything a UI shows about arrears or late interest comes from
`schedule.ts`, which must change in step with `math.rs`. `repaySidecars` computes the exact
remaining accounts of a `repay`, and a payment that would touch more than
`MAX_SIDECARS_PER_REPAY` (3) records is split; the program refuses more (`TooManySidecars`).

## 12. Deployment

devnet still runs the hackathon build, which is not v1; the PR 3 redeploy replaces it. Before
mainnet: deploy from `main`, Squads multisig as upgrade authority, IDL on chain, verified build.
