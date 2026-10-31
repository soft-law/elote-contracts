# Dev brief: pool v2, line v3 and the pagaré token

Date: 2026-10-31. For: the developer session on this repository (`elote-contracts-8e`, started by
Mario). From: the coordinator session (`elote-contracts-52`). Status: active brief; the coordinator
updates it when a decision changes.

## Who does what

| Role | Session | Talks to you about |
|---|---|---|
| Coordinator (owner delegate) | `elote-contracts-52` | scope, decisions, ambiguities, review, when to push |
| Design (app and UX) | `theoffice-8d` | event shapes and UI states, through the coordinator |
| Research (legal, academic) | `loans-gov-mexico-c3` | legal constraints, through the coordinator |
| Owner | Mario | approves pushes, PRs and deployments |

Send progress and questions to `elote-contracts-52` with `SendMessage`. Do not message design or
research directly for decisions; the coordinator routes them. Do not reopen a decided question
(`docs/decisions/2026-10-31-owner-review.md`); if a plan item is ambiguous or contradicts the code,
stop and ask with the file and line.

## Read first, in this order

1. `docs/decisions/2026-10-31-owner-review.md`: every decision, with rationale. Source of truth.
2. `docs/plans/2026-10-24-series-shelf-contract-plan.md`: pool v2 (Token-2022 interface,
   asset-mint policy, admin-supplied share mint under Token ACL, `permissioned` flag, dead-shares
   thaw, `NavUpdated`). Its five open questions are decided (see its last section).
3. `docs/plans/2026-10-31-multi-draw-plan.md`: line v3, tranched draws with proportional
   interest (decisions D1 to D15), late interest on overdue principal only, sponsor roles,
   concentration cap.
4. `docs/plans/2026-10-31-expediente-token-design.md`: the pagaré token. One Metaplex Core asset
   per draw is the electronic pagaré; `draw` mints it; `cancel_pagare` burns it; `endorse_pagare`
   moves it. Read "Changes to the tranched plan" there for the exact fields and instructions.
5. `ARCHITECTURE.md` (how the code does it today; you will update §0, §5, §7, §8, §9),
   `SECURITY.md` (roles and trust table; you add the attestor and the series vehicle's key),
   `README.md` "Build and test" and "Deployments".
6. The review register for the Phase 1 items: https://claude.ai/artifact/B3nu5ja6XzF7oZsJwqXVee
   (rows R-11, R-21, R-24, R-25, R-27, and the two deltas). The items are copied below so you do
   not need the page to start.

Use `/mem-search` (claude-mem) for past context on this repo before you start: searches worth
running are "elote_pool known limits", "arrears vectors", "anchor build --arch v0", "Token ACL",
"SAS attestation verify".

## Toolchain and skills

- Rust 1.89.0 (`rust-toolchain.toml`), Anchor CLI 1.2.1, Solana CLI 4.1.2, Node 22.12+, LiteSVM
  0.11. Build with `NO_DNA=1 anchor build --ignore-keys --arch v0`; never run `anchor keys sync`
  or `anchor deploy` with the generated keypair (README explains why). Program id stays
  `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`.
- Skills to invoke: `solana-dev` (installed from https://github.com/solana-foundation/solana-dev-skill;
  use its program, testing and security references, and its Anchor 1.2 notes),
  `superpowers:test-driven-development` (write the LiteSVM test first for every behaviour
  change), `superpowers:verification-before-completion` (run the full command list below before
  reporting anything done), `superpowers:using-git-worktrees` or a plain branch from `main`, and
  `code-review` on your own diff before handing a PR to the coordinator.
- Commands that must be green before any report of "done":
  ```sh
  NO_DNA=1 anchor build --ignore-keys --arch v0
  cargo test -p elote_pool
  npm run codama && npm run check:client
  npm run typecheck && npm run test:client
  UPDATE_VECTORS=1 cargo test --test vectors   # only in the commit that changes math.rs, then re-run the suite
  ```
- Conventions already in the code, keep them: every arithmetic step `checked_*` or `u128`;
  `None` becomes `Overflow`; events through `emit_cpi!`, append-only (fields of an existing event
  never change; new facts are new events); nothing personal on chain, ever (no names, no RFC, no
  folio, no URI with identifiers, in state or in instruction data); seeds and bumps validated as
  today; balances reloaded after CPIs before events; the 100,000 CU ceiling in `tests/compute.rs`;
  one commit per coherent change, with the attribution line the harness gives you.

## Work plan: three pull requests, in order

### PR 1: Phase 1 hardening (no account layout change, no IDL change except new errors)

Branch `hardening/phase-1` from `main`. Items, all spec-free:

- **R-11**: in `repay`, advance `late_paid_until` only when `late_part > 0`; test: miss one
  installment, repay 1 unit twice 30 s apart, assert the second settlement is nonzero.
- **R-21, R-27**: doc fixes: `tests/common/mod.rs` says LiteSVM 0.9.0 (is 0.11.0); `lines.rs:5`
  is 130 chars and omits `write_off` from the pause-exempt list; two comments that restate the
  code near `lines.rs:312` and `:319`; the `write_off` doc in `lib.rs` reads as a waiting period;
  README line 19 still says Anchor 1.1.2; SECURITY.md pause row lacks the draw-window facet.
- **R-24**: a guard in `clients/js/src/schedule.ts` so `lateDue` is 0, never negative, when
  `lateCollected > principal`; test in `clients/js/test`.
- **R-25**: `Pool::check()` at the end of every money instruction: `vault + outstanding` does not
  overflow and `share_mint.supply >= DEAD_SHARES` once anything was deposited; new error.
- **Deltas**: derive `MIN_FIRST_DEPOSIT` from the asset's decimals (R-07 delta); reject
  `borrower == admin` in `open_line` (R-01 delta); invert the two `known_limits.rs` tests that pin
  those behaviours.
- **D13 / decision 20**: `max_line_bps: u16` on `init_pool`, stored in `Pool.reserved` (there are
  28 free bytes; a `u16` fits without a layout change), enforced at `open_line` as
  `principal <= nav * max_line_bps / BPS` with a distinct `LineTooLarge` error; default 1,000
  bps in the scripts. The app computes the cap before signing; the error is the backstop.
- **R-14**: a draft `docs/runbook.md` (keeper cadence for `mark_in_collection`, who calls what,
  revocation = close attestation + remove from list + freeze accounts, incident contacts as
  placeholders) for the owner to approve.

Estimate 1 to 2 days. Report test counts before and after.

### PR 2: pool v2 + line v3 + pagaré token (one breaking batch)

Branch `v2/pool-and-lines` from `main` after PR 1 merges (or rebased on it). Order inside the
branch, each step with its tests before moving on:

1. **Token interface**: `TokenInterface`, `InterfaceAccount<Mint/TokenAccount>`,
   `transfer_checked` everywhere (series-shelf 2a). Classic and Token-2022 assets both pass.
2. **Asset-mint policy at `init_pool`** (2b, decision 15): reject transfer fee, permanent
   delegate, transfer hook, non-transferable, confidential-only, mint close authority; accept a
   freeze authority and record `asset_freezable` as a read-only flag. Test every rejected
   extension.
3. **Admin-supplied share mint** (2c, 2d, 2e): Token-2022, `mint_authority == pool PDA`, supply
   0, decimals == asset decimals, no fee, no hook, no close authority, no permanent delegate;
   `permissioned` flag set when `DefaultAccountState = Frozen`; the dead-shares account thawed or
   pre-thawed at init; test deposit fails before thaw and succeeds after, with Token ACL's
   `MintConfig` as freeze authority (use the Token ACL program binary in LiteSVM, or a stub gate
   that records the same account state; say which in the PR).
4. **Pool v2 fields** (decision 24, expediente design): `attestor: Pubkey`, `collection: Pubkey`
   (the series' Core collection), 64 bytes appended; `set_attestor` admin instruction; `version = 2`;
   `NavUpdated { pool, total_assets, share_supply }` after every deposit, redeem, repay and
   write-off (2f).
5. **Line v3, tranched** (multi-draw plan): `drawn`, `draws`, `availability_end`,
   `sponsor_role`, `expediente_root`; `open_line(..., sponsor_role, expediente_root)`;
   `draw(amount, pagare_hash)` with the checks of the plan's instruction table (D3 strict window,
   D3b schedule payability, `MAX_DRAWS`, liquidity) and the attestor as a required signer (D14);
   interest per tranche at an annual rate to `last_due` (D1, bullet lines to `term_end`,
   scheduled lines to `schedule_end`); `suspend_draws` (D10); `repay`, `line_arrears`,
   `mark_in_collection`, `write_off` on `drawn`; events `LineDrawState`, `LineDrawsSuspended`,
   `LineSponsorRole`. Late interest on the overdue **principal** part only (decision 11):
   change `math::arrears`, regenerate the 413 vectors in the same commit, update the JS mirror
   and its tests.
6. **Pagaré token** (expediente design): at `draw`, CPI into Metaplex Core to create one asset in
   the pool's collection, owner and update authority = pool PDA, `PermanentFreezeDelegate` and
   `PermanentTransferDelegate` with the pool PDA as authority, `ImmutableMetadata`, Attributes
   `{pagare_hash, amount, currency, issued_at, due_at, line, draw_index}` with authority `None`,
   an `AppData` section with the pool as data authority; `Pagare` sidecar PDA
   `["pagare", line, draw_index]`; `cancel_pagare(k)` (coverage rule, burn, close sidecar, rent to
   the relayer, fails in `InCollection` or `WrittenOff`); `endorse_pagare(k, kind, endoso_hash,
   to)` (signers: current holder key and attestor; `to` must hold a valid SAS attestation under the
   pool's credential: verify owner = SAS program, PDA from (credential, schema, wallet), credential
   and schema match the pool config, not expired against the clock; append to `AppData`;
   `TransferV1` through the permanent delegate); events `PagareIssued`, `PagareCancelled`,
   `PagareEndorsed`. The SAS credential and schema addresses go in the pool config next to
   `attestor` and `collection` if they do not fit elsewhere; ask the coordinator before adding
   more than 64 bytes to the Pool.
7. **Tests**: the lists in the two plans, plus the compute test with the Core CPI on `draw` at
   120 installments. Use the Metaplex Core program binary in LiteSVM (`mpl-core` from the
   Metaplex release) and record its version in `tests/common`.
8. **Docs and client**: Codama regen; `ARCHITECTURE.md` §0 (the pagaré token and the sistema de
   información, two sentences), §5 instructions, §7 invariants, §8 roles (attestor, series
   vehicle key, Metaplex Core upgrade authority as a dependency), §9 known limits (invert what
   changed); `SECURITY.md` trust table; `README.md` instruction and event tables; the plan files'
   "Then" lists.

Estimate 7 to 9 days with tests. Report at the end of each numbered step: tests added, CU
numbers, anything the plan did not foresee.

### PR 3: scripts and devnet

`scripts/draw.ts --amount`, `scripts/open-line.ts --sponsor-role --expediente-root`,
`scripts/cancel-pagare.ts`; a devnet redeploy from `main` once the coordinator and Mario say so
(decision 17: nothing is pushed or deployed without Mario). The v1 devnet demo lines will not
deserialize after PR 2; that is accepted (decision 14).

## Out of scope, do not build

Revolving lines (deferred); any KYC provider integration (Didit lives in softlaw-backend); the
CETES product; SNS; Layer 2 ideas beyond the pagaré asset itself (no secondary market venue);
investor-side thaw relayer (backend); the agreements engine (NOM-151, salts, document storage).

## Points the coordinator still owns

- A minimum number of days between a later tranche and the first due date (a product question
  with Mario; if it lands, it is one `require!` in `draw` and a line parameter).
- Whether the series' Core collection is created by `init_pool` or supplied like the share mint.
  Default until told otherwise: supplied by the admin and validated (same pattern as 2c).
- Exact `sponsor_role` semantics beyond the byte (nothing enforced on chain in this batch).

## How to report

First message: acknowledgement, the branch name, your reading of the three PRs, and anything in
the plans that contradicts the code you see. Then one message per PR step with the test and CU
numbers, and a message before any action that is hard to reverse. Cite files and lines. If a
command fails, send the output, not a summary.
