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
  reporting anything done), `superpowers:using-git-worktrees` or a plain branch as named per PR below, and
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

### PR 1: Phase 1 hardening (no account layout change; the IDL changes for `init_pool`'s new argument, the read-only `vault` on `OpenLine`, new errors and one new event)

Branch `hardening/phase-1` from the tip of `poc/kyc-identity-eval` (which is `main` 3c1debd plus the docs and PoCs; `main` itself has none of the plans yet). Items, all spec-free:

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

Branch `v2/pool-and-lines` from `hardening/phase-1` after PR 1 is reviewed (rebase when it merges). Order inside the
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

## Decided after the first version of this brief (2026-10-31, later the same day)

- **Decision 25, the calendar comes from the signed contract.** In PR 2 step 5 the schedule anchor
  is `first_due_at`, an argument of `open_line` inside the schedule parameters, not `drawn_at`:
  installment `k` is due at `first_due_at + (k - 1) * interval`; `availability_end` is set at
  `open_line` (`first_due_at` on scheduled lines, `term_end` on bullet lines); `drawn_at` stays as
  the time of the first draw, informational; `last_due` for the per-tranche interest is
  `first_due_at + (installments - 1) * interval`. Add `cancel_line` (admin, `drawn == 0`) with a
  `LineCancelled` event for lines never drawn by their first due date. The hold on the draw-window
  tests is lifted; write them against this anchor. The "minimum days before the first due date"
  item below is closed: not needed.
- **D16, every obligation is born from a signed document.** The attestor co-signs `open_line` too
  (not only `draw` and `endorse_pagare`), attesting that the signed agreement's hash matches the
  arguments. Add the signer to the `OpenLine` accounts and a test that `open_line` without it
  fails with `AttestorRequired`.
- **Architecture gate before PR 2 steps 5 and 6 are closed.** The research session is producing
  a legal brief with a list of verifiable conditions for the pagaré token. The coordinator will
  send you the mapped conditions; do not consider steps 5 and 6 final until that message arrives.
  Steps 1 to 4 and all of PR 1 are not affected.

## Answers to the developer's first reading (2026-10-31)

1. `OpenLine` takes `vault` read-only (`address = pool.vault`) for the NAV cap; regenerate the
   client and update the two scripts. Accepted.
2. `PoolLineCapSet { pool, max_line_bps }` as a new event after `PoolConfigured`. Accepted.
3. `Pool::check()` runs the vault-plus-outstanding leg everywhere and the supply leg only where
   `share_mint` is already an account (`deposit`, `redeem`). Accepted.
4. Test-harness default `max_line_bps = 10_000`; the cap has its own tests at a lower value;
   scripts default to 1,000. Accepted.
5. PR 2 drops the v1 pool code paths (program-created share mint) and, with them, the v1 line
   semantics: every line opened by the new program is version 3, bullet or scheduled, with one
   repayment order (late interest, then interest, then principal) and clamped overpayment. The
   v1 "principal first, reject overpayment" order dies; invert or delete its tests and update
   `ARCHITECTURE.md` §5 and §9 accordingly. Decided by the coordinator under decision 14.
6. D1 governs the per-tranche interest, and after decision 25 `last_due` is
   `first_due_at + (installments - 1) * interval` on a scheduled line. The instruction table's
   `term_end - now` is superseded.
7. `poolMaxLineBps` reader in `clients/js/src/schedule.ts` next to the grace reader. Accepted.

## Status log

- **2026-10-31, PR 1 accepted** at `e1a0e36` on `hardening/phase-1` (nine commits on `8c44e74`,
  worktree `.claude/worktrees/hardening-phase-1`, local, not pushed). Tests 75 Rust + 4 JS → 82
  Rust + 6 JS, 0 failures; full command list green; CU at 120 installments: repay 58,221,
  mark_in_collection 37,785, write_off 39,891. Accepted deviations: demo script default 5,000 bps;
  `min_first_deposit` keeps a floor of 1,000 × `DEAD_SHARES` under `10^decimals`; the liquidity test
  drains by redemption; Prettier on one test file. Accepted design facts: the cap bounds one honest
  proposal, not the admin (R-01, Q8 work); pools created before the branch open no new line after an
  in-place upgrade, with no zero-means-uncapped fallback. New errors 6019 `BorrowerIsAdmin`, 6020
  `LineTooLarge`, 6021 `InvariantBroken`; new event `PoolLineCapSet`; `init_pool` takes
  `max_line_bps`; `open_line` takes the vault read-only; client exports `poolMaxLineBps` and
  `maxLinePrincipal`. The app and backend must pick these up.
- **Rule agreed the same day:** only the developer session edits its worktree; the coordinator
  sends changes as messages.
- **PR 2 step 1 accepted** (Token interface; 83 Rust + 6 JS; repay 59,128 CU at 120). Every money
  instruction now takes the asset mint read-only and an explicit token program: app and backend
  change list.
- **Step 2 answers:** `Pausable` is accepted and recorded like the freeze authority, as
  `asset_controls: u8` in `Pool.reserved` (bit 0 freeze authority, bit 1 Pausable) replacing the
  single `asset_freezable` flag; a `DefaultAccountState` other than Initialized on the asset is
  rejected; any confidential extension (`ConfidentialTransferMint`, `ConfidentialTransferFeeConfig`,
  `ConfidentialMintBurn`) is rejected, because the NAV reads `vault.amount`.
- **PR 2 step 2 accepted** (`e12354b`, asset-mint policy; 85 Rust + 7 JS). Deviation recorded:
  `permissioned` lives in `reserved[6]` and `asset_controls` in `reserved[7]` because PR 1 took
  `reserved[4..6]` for `max_line_bps`; new event `PoolFlags { pool, asset_controls, permissioned }`
  and client reader `poolFlags`; new error 6022 `AssetMintNotAllowed`.
- **Step 3 answers:** share mint may be classic or Token-2022 (the first mainnet pool must be
  Token-2022 frozen-by-default under Token ACL; runbook says so); reject the three confidential
  extensions and `InterestBearingConfig` on the share mint; `NonTransferable` shares allowed; a
  frozen default state requires a freeze authority; dead-shares thaw per 2e with both paths tested;
  `init_pool` joins `compute.rs`.
- **PR 2 step 3 accepted** (admin-supplied share mint; 89 Rust + 7 JS; `init_pool` 43,519 CU
  classic / 37,136 permissioned). `ShareMintNotAllowed` 6024; the `["shares", pool]` PDA is gone;
  `init_pool`, `deposit` and `redeem` take `share_token_program`; 2e realised as "the gate's thaw
  rides in the init transaction" (path 2) or a thaw before the first deposit (path 1); the demo
  runs on a classic share mint until PR 3. First mainnet pool: Token-2022 frozen-by-default under
  Token ACL, `PoolFlags.permissioned` as the public fact (runbook, SECURITY.md).
- **PR 2 step 4 accepted** (`08006a7`, Pool v2 fields; 92 Rust + 7 JS; Pool 299 bytes, `attestor`
  at 235 and `collection` at 267, `POOL_VERSION = 2`; `init_pool(grace, max_line_bps, attestor)` plus
  the collection account; `set_attestor`; events `AttestorSet`, `PoolCollectionSet`, `NavUpdated`;
  `repay` and `write_off` take the share mint). App/backend list grows accordingly.
- **Step 5 go given** without the legal brief, because its conditions bear on step 6 and on the
  agreements engine; step 6 (pagaré asset, cancel, endorse) stays gated. Step 5 scope restated in
  the coordinator's message of the same day: line v3 fields, attestor on `open_line` and `draw`,
  `first_due_at` anchor, proportional interest to `last_due`, principal-only late interest with
  regenerated vectors, `suspend_draws`, `cancel_line`, one repayment order.
- **2026-11-01, step 5a accepted** (`d3d0ac5`, local): late interest on the principal share of
  each overdue part (`part * principal / total`, rounded down; `Arrears.overdue_principal`),
  `ScheduleTerms.anchor` (one interval before the first due date), `math::tranche_interest`,
  416 vectors (413 regenerated + 3 hand cases), JS mirror updated; 94 Rust + 7 JS. The line v3
  program change is written and builds, uncommitted in the developer's worktree (7 files) when
  that session paused on its usage limit; nobody else touches it. Remaining for step 5: commit B
  (tests), commit C (`suspend_draws`, `cancel_line`, `LineCancelled`), docs, Codama, scripts,
  report, push. R-10 closes with the single repayment order. Error codes so far: 6024
  `AttestorRequired`, 6025 `AvailabilityEnded`, 6026 `TooManyDraws`; events `LineTermsSet`,
  `LineSponsorRole`, `LineDrawState`.
- **Pushed 2026-10-31 with Mario's approval:** `poc/kyc-identity-eval` and `hardening/phase-1`;
  `v2/pool-and-lines` is pushed by the developer after each accepted step (remote at `08006a7`).
- **2026-11-01, step 5 accepted** (`277b213` on origin: 5a `d3d0ac5`, 5b `e24b023`, 5c
  `277b213`; 108 Rust + 7 JS; 416 vectors). CreditLine 305 bytes (offsets 246 drawn, 254 draws,
  256 availability_end, 264 first_due_at, 272 sponsor_role, 273 expediente_root), `LINE_VERSION 3`
  for every line, `MAX_DRAWS 64`; R-10 and the cancel half of R-19 closed; new terminal status
  `Cancelled`; events `LineTermsSet`, `LineSponsorRole`, `LineDrawState`, `LineDrawsSuspended`,
  `LineCancelled`; errors 6024-6026. CU at 120 installments: repay 84,381, mark 60,106, write_off
  65,271, draw 22,111.
- **Ordered next:** step 5d (recover CU headroom by computing the principal share once for the
  equal middle installments, vectors byte-identical), then step 6 in parts: 6a harness with the
  Metaplex Core binary and the collection check at `init_pool`; 6b issuance of the Core asset and
  the `Pagare` sidecar at `draw`; 6c `cancel_pagare`. **6d `endorse_pagare` and the verifier stay
  gated on the legal brief.**
- **Step 5d accepted** (`bcfeb3c`): principal share computed once for the regular installment;
  vectors byte-identical; repay 63,946 / mark 39,635 / write_off 44,800 CU at 120 installments.
- **6a accepted as planned:** mpl-core crate 0.12.1 (solana-program 3.0 line, no anchor feature)
  against the `release/core@0.15.3` program binary committed under
  `programs/elote_pool/tests/fixtures` (< 1 MB, sha256 verified at load, tag and URL in a README);
  collection admin-supplied and validated at `init_pool` (Core-owned, `CollectionV1`, update
  authority = pool PDA).
- **6a accepted** (`53dd606`): fixture 809,936 bytes, sha256
  `5b4dacdae4af476d3c9d8aa6712883f559ba0e13705f9b244d96d1fd0dd56ce5` asserted at load;
  `CollectionNotAllowed` 6027; 109 Rust + 7 JS. Rule learned the hard way (the crate's borsh
  decoder panicked on an asset passed as a collection): on chain, read foreign Core accounts by
  fixed offsets and bounded lengths, never with the crate's full decoders.
- **6b accepted** (`16467fa`; 115 Rust + 11 JS): `draw` mints the pagaré Core asset in the pool's
  collection (update authority expressed as `UpdateAuthority::Collection`, whose authority is the
  pool PDA) and the `Pagare` sidecar (163 bytes with 32 reserved); attributes in hex; a real
  griefing vector (pre-funding the predictable asset PDA bricked every draw) is closed by sweeping
  lamports before the CPI, with a test; `DocumentHashRequired` 6028; client helpers in
  `clients/js/src/pagare.ts`. **Decided:** add `PermanentBurnDelegate` (pool PDA) at creation so
  cancellation is one `BurnV1` whatever the holder; `draw` takes `asset_bump` and `pagare_bump`
  as arguments, verified with `create_program_address`, so its CU is deterministic (~82.5k at
  120) instead of 82k to 87k with an unbounded tail.
- **2026-11-01, legal brief draft received.** Mapping in
  `docs/plans/2026-11-01-pagare-token-legal-conditions.md`. Sent to the developer before 6c:
  `cancel_pagare(k, receipt_hash)` signed by the holder (admin while in the pool) and the attestor,
  sidecar kept as the persistent PAGADO mark (`paid_at`, `receipt_hash`, status), burn last,
  `PagareCancelled` with the receipt hash; `draw` gains `cert_fingerprint` and `signature_method`;
  neutral asset and collection names; the new attestor co-signs `set_attestor`; 6d prework: verify
  whether Core's permanent plugin authorities can be reassigned at endoso (C9). 6d and the verifier
  still gated; per-installment vs per-draw pagaré (C19/C20) open with counsel and Mario.
- **Decisions 1 and 2 landed** (`8c08c87`, local at report time): `PermanentBurnDelegate` in the
  plugin set; `draw(amount, document_hash, asset_bump, pagare_bump)` verified with
  `create_program_address`, deterministic 86,064 / 86,239 CU at 120 installments (Core CPI
  43,671). Accepted plan for the next three commits: draw additions (`cert_fingerprint`,
  `signature_method`, neutral names, `set_attestor` co-signed by the new attestor; sidecar grows
  with `drawn_through`, status, `paid_at`, `receipt_hash`, reserved bytes kept); `cancel_pagare`
  per the C22 spec with the holder signer = `pool.admin` while in the pool and no AppData length in
  the event (fixed-offset rule); 6d prework as pinned LiteSVM experiments on Core plugin-authority
  reassignment. Per-draw shape kept.
- **6c commits 1 and 2 accepted** (`8c08c87`, `3c2c13b`, local; 118 Rust + 11 JS):
  `draw(amount, document_hash, asset_bump, pagare_bump, cert_fingerprint, signature_method)`,
  `SignatureMethodInvalid` 6029, nine attributes, asset name `PAGARE <line>-<draw>`; sidecar 245
  bytes, never closed, with `drawn_through` (invariant pinned), `status`, `paid_at`,
  `receipt_hash`; `set_attestor` takes the new attestor as a signer; draw 89.7k CU at 120 with
  ~10k of room (attributes are the lever). Known limit accepted: a valid non-canonical bump is
  accepted by `draw`; the attestor's client is the canonicality guard. Next: commit 3
  `cancel_pagare`, commit 4 the Core authority experiments, then push.
- **6c commit 3 accepted** (`7c87f38`; 123 Rust + 11 JS, clippy clean; push of commits 1-3
  authorised): `cancel_pagare(draw_index, receipt_hash)` in `instructions/pagare.rs`; holder
  signer = `pool.admin` while the asset is in the pool (`NotHolder` 6032), attestor co-signs, a
  payer receives the asset's rent; checks before any write: outstanding (`PagareAlreadyPaid`
  6031), receipt nonzero, line `Active | Repaid`, coverage `principal_repaid >= drawn_through`
  (`PagareNotCovered` 6030). Sidecar marked PAGADO and never closed; `PagareCancelled` carries
  `holder` (no AppData length); `BurnV1` last with the pool PDA as permanent burn delegate (Core
  force-approves through the permanent freeze; one-byte tombstone stays, rent returns to the
  payer). 26,891 CU at 120. Two deviations accepted: coverage counts **principal only** (the
  plan's principal + interest let a título burn with principal still owed, since repay settles
  interest first); the event names the holder, so 6d needs no new shape. Receipt reuse is not
  checked on chain (engine issues one per pagaré; attestor is the guard). Next: commit 4.
- **6c commit 4 accepted** (`1024104`; 126 Rust; `tests/core_capabilities.rs`, no program change;
  push authorised). Core 0.15.3 findings: permanent delegates cannot be re-approved directly
  (`CannotRedelegate`); revoke resets to the collection's authority (the pool PDA) which can then
  approve a key or `None`; `None` is terminal; only the freeze delegate thaws, and frozen + `None`
  is stuck; an asset's update authority never leaves its collection, `AddBlocker` at creation
  closes the remaining plugin door. Coordinator's reading: two designs, **A** relinquish at
  endoso (C9 exceptions impossible, C5/C28 off chain) vs **B** retain and bound by instructions;
  **B is the working assumption pending counsel Q7**; `AddBlocker` not added to `draw` yet. Two
  measurements ordered before 6d: draw CU at 120 with `AddBlocker` in the creation set; whether
  the AppData data authority can change after creation and whether `AddBlocker` blocks updating
  an existing adapter. Traps for the 6d spec: thaw before any relinquish; approving the endorsee
  as permanent delegate is leaky (the delegate can revoke back to the pool). 6d and verifier held.
- **6c follow-up accepted** (`5092994`, pushed with `1024104`; 128 Rust + 11 JS): the AddBlocker
  refusal pins `InvalidAuthority`; the two measurements are pinned. `AddBlocker` costs 4,199 CU
  at creation (39,408 → 43,607; band 3,000-6,000), so draw at 120 would sit near 94k, inside the
  ceiling with about half the room gone; still an option, not a change to `draw`. AppData: the
  data authority is fixed at creation and cannot be moved or set to `None`; only it writes the data
  or changes the schema; adding or removing an adapter is the collection's authority alone and
  `AddBlocker` does not stop it. So under A or B alike the pool PDA is the endoso log's only writer
  and, as the collection's authority, could remove it: the answer is the instruction set (only
  `endorse_pagare` writes, nothing removes) plus governance, the same as for the delegates. This
  weakens A further: relinquishing the delegates does not remove the vehicle's power over the
  log. 6d and verifier held; nothing in progress on the dev side.
- **Arrears refactor accepted** (`4a440c0`, pushed; origin tip of `v2/pool-and-lines`): the loop
  carries the previous cumulative due instead of recomputing it and hoists the constant accrual
  cap; same arithmetic in the same order, 416 vectors byte-identical, no regeneration. At 120
  installments: repay 56,569 CU (was 63,946), mark_in_collection 33,453 (39,635), write_off
  38,620 (44,802). 128 Rust + 11 JS, clippy clean. Dev holds on 6d and the verifier.
- **Mario decided A for C9** (decision 27, 2026-11-01): 6d design and measurements released,
  code held until the spec and CU numbers are accepted. Sent to the developer: measure the first
  endoso at 120 with the current plugin set and with a set without the permanent transfer and
  burn delegates (in-pool cancel then burns as owner after a thaw), `AddBlocker` in the creation
  set; pin whether an AppData adapter can carry data authority `Owner` that follows `TransferV1`
  (if so the log travels with the holder); `endorse_pagare` as an owner-signed instruction (SAS
  gate, `endoso_hash`, attestor, `PagareEndorsed`, `TransferV1` with the signer's authority;
  first endoso thaws and relinquishes every delegate, irreversibly); `cancel_pagare` gains the
  endorsed branch (holder = Core owner at fixed offset, burn as owner); direct Core transfers
  outside the program are allowed by construction and C28 binds only program-routed endosos;
  traps: thaw before relinquish, never frozen again after the first endoso. Deliverables in order:
  Owner-authority finding, endoso CU numbers, then a written 6d spec.
- **6d prework accepted; creation set decided** (facts pinned in `tests/core_capabilities.rs`,
  seven tests, prework commit pending): AppData with data authority `Owner` works and follows
  `TransferV1` (a holder's write replaces the data, so the immutable history is the events plus
  Core's ownership transactions); the owner adds owner-managed delegates even under `AddBlocker`,
  approves and revokes them alone, and `TransferV1` resets them to `Owner`; an approved burn
  delegate burns a thawed asset. Numbers at 120: today's set makes the first endoso 190k CU;
  freeze delegate with authority `UpdateAuthority` + `AddBlocker` + AppData `Owner`, no transfer
  or burn delegates: draw 84.8k, first endoso en propiedad 79.3k, procuración 45.8k, in-pool
  cancel 45.2k (thaw then burn as owner). **That set is adopted.** Spec additions: sidecar keeps
  `endoso_count` and `last_endoso_hash` for program-routed endosos; `endorse_pagare(kind, to,
  endoso_hash)` with kinds Propiedad and Procuracion, `revoke_procuracion`; cancel's endorsed
  branch accepts the owner or the approved burn delegate. **Decision 28** (per draw, partial
  payments, acceleration at the third missed installment): impact assessment ordered on late
  interest over all remaining principal from `mark_in_collection`, fields, vectors, CU; no code.
- **6d prework committed** (`fc7feba`, accepted, push authorised; 130 Rust): seven pinned Core
  tests, the 6d spec in ARCHITECTURE §2.3, traps in SECURITY. Sidecar grows to 279 bytes at 6d;
  cancel's endorsed branch chooses by Core's owner and relies on Core to refuse anyone but the
  owner or the approved burn delegate. **Decision 28 accepted as PR 2 step 7**, after the comment
  cleanup: `accelerated_at: i64` on CreditLine (305 → 313), one-loop arrears formulation (all
  installments due after the mark, each accruing from max(min(due_k + grace, accelerated_at),
  late_paid_until), overdue = whole balance), `mark_in_collection` sets the timestamp and
  prorates `interest_due` to the installments due at the mark (unaccrued ordinary interest
  forgiven; pending Mario's and counsel's confirmation; forgiven amount added to
  `LineInCollection`), vectors regenerated in the same commit, client mirror in schedule.ts.
  CU at 120 roughly unchanged; post-mark repay costs the worst-case figure. **Comment cleanup**
  ordered as its own commit before step 7 (Mario, 2026-11-01); a separate audit agent reviews
  the pushed tip in its own worktree, report under `docs/audits/`.
- **Pushed** `fc7feba` and `017774a` (SECURITY sentence on the reliance on Core's check in
  cancel's endorsed branch); origin tip `017774a`. Sessions paused on usage limits the night of
  2026-11-01 and resumed 2026-11-02: dev on the cleanup then step 7 (proration last, behind one
  function, pending research Q14); audit agent resumed; research on the supplement, Q14 first.
- **Research supplement applied (2026-11-02)**: Q14 confirms the step 7 proration (per-installment
  moratory start and principal-only base already hold). 6d spec amendments sent: registry becomes
  a program-owned hash chain in the sidecar (`log_root`, `entry_count`, `last_kind`; AppData
  dropped from the creation set; re-measure); procurador's burn delegate points at a program PDA
  `["procurador", pagare, key]`; holder endosos need no attestor or gate; `kind` mandatory;
  `revoke_procuracion` owner-only; termination entry on en propiedad over a procuración;
  `court_annotation` and late entries (two dates); payment entries per repayment with
  `receipt_hash` (shape decided on CU at 120, inside `repay` vs bundled `annotate_payment`).
  6d code split: part 1 (registry, procuración, court, payments) on release; part 2 (en
  propiedad) gated on C32 design. Order: cleanup → amended §2.3 + two assessments → step 7.
- **Comment cleanup accepted** (`f86b482`, push authorised; comments only, 130 Rust green, CU
  unchanged): 590 → 516 comment lines of 2,993; every step, decision, D and R reference gone from
  source and tests; legal narrative reduced to tags; duplicated struct/handler text removed; the
  layout tables, formulas, ordered check lists and the three safety notes stay. One correction:
  the schedule interval counts from `first_due_at`, not `drawn_at`. **Decision 29** narrows 6d
  v1 to procuración (registry, `endorse_pagare` kind Procuracion by the pool, revocation, court
  annotation, payment entries, cancellation by the procurador via the program PDA); en propiedad
  specified, not built. Dev now on the §2.3 amendments and the two assessments, then step 7.
- **Final legal brief applied (2026-11-02)**: three more §2.3 points sent: payment entries at
  `repay` are valid under the template's mandate clause (holder/procurador-signed entries remain
  the alternative); the covering payment writes a Pagado entry and sets the sidecar to
  paid-pending-cancel, `cancel_pagare` requires that status and burns; the time-locked holder-only
  cancellation (`cancel_delay_seconds` on the pool) is specified and built in part 1 if small;
  with no AppData adapter the "removing adapters alters the title" objection dissolves.
- **6d spec accepted** (`1175c6d`, push authorised; 131 Rust, eight pinned Core tests): registry
  as a sidecar hash chain (`log_root`, `entry_count`, `last_kind`, flags; eleven kinds; full
  entries in `PagareEntry`), creation set without AppData (draw falls to 78.9k CU at 120),
  procuración 32.7k, revocation 36.6k, court annotation 14.1k, in-pool cancel 44.1k, cancel by the
  procurador 48.3k; sidecar 353 bytes with 32 reserved; a later procuración approves instead of
  adding (flags bit 0); thaw before the procurador's burn. **Payment entries: shape (i)** inside
  `repay` (~5k CU per touched sidecar, client splits beyond eight), Pagado entry at coverage sets
  paid-pending-cancel; backstop built in part 1 (`cancel_delay_seconds` in Pool.reserved 8..12,
  floor 7 days). Dev on step 7; 6d part 1 released after step 7 and the audit report.
- **Audit report received** (`docs/audits/2026-11-01-elote-pool-audit.md`, branch
  `audit/2026-11-01-elote-pool`, commit `b0d7ce2`): risk 5, eight findings (one sev 5, three sev 3,
  four informational), no funds path, baseline reproduced, refactor `4a440c0` verified. **Step 8
  "audit fixes"** ordered after step 7: F-01 Repaid only when the window is closed or nothing
  remains to draw (PoC as regression test, D5 reworded); F-02 collection must carry no plugin
  header and no minted assets; F-03 `collection_period_seconds` (Pool.reserved 12..16, floor 30
  days) and `collection_started_at` on the line (layout grows once with step 7: 305 → 321);
  F-05 role separation and share-mint policy; F-07 error hygiene; F-06 closes with 6d (cancel by
  sidecar status); F-08 and F-04 documentation/ops. Order: step 7 → step 8 → 6d part 1.
- **Step 7 accepted** (`c149700`, `4cb372a`, plus the §2.3 sentences `e4b2144`; push authorised;
  136 Rust + 11 JS): `accelerated_at: i64` on CreditLine (313 bytes), set by
  `mark_in_collection`; one-loop arrears with every installment exigible at
  min(due_k, accelerated_at) and accruing from max(min(due_k + grace, accelerated_at),
  late_paid_until); no cure after the mark; vectors 421 (original 416 unchanged, 44 new in kind
  via a second random stream), mirrored in schedule.ts; `math::accelerated_interest` keeps
  interest_due × due_count / installments with due_count = max(due at the mark, covered +
  max_missed), floored at interest already repaid, `interest_forgiven` appended to
  `LineInCollection`. CU at 120 all-missed: repay 56,992 / mark 34,114 / write_off 39,093; after
  an early mark repay 48,148. **F-03 decision:** `accelerated_at` doubles as the collection
  start; `write_off` gates on `accelerated_at + collection_period_seconds`; layout stays 313.
- **Step 8 accepted** (`f366ff5`, push authorised; 141 Rust + 11 JS): F-01 Repaid needs nothing
  due and nothing drawable (regression tests in `tests/audit.rs`); F-02 bare collection only
  (base record, no plugin header, nothing minted); F-03 `collection_period_seconds` (Pool.reserved
  12..16, floor 30 days, `set_collection_period` + `CollectionPeriodSet`), `write_off` waits for
  it from `accelerated_at`; F-05 `AttestorIsAdmin`, `BorrowerIsAttestor`, share mint without
  ScaledUiAmount/Pausable; F-07 codes 6033-6038; F-06, §8-1, F-08 documented. write_off at 120
  now 47,896 CU (real worst case, 30 days of accrual on every installment). **6d part 1
  released** with three spec closures to confirm before the repay changes: a `pagares_paid`
  cursor on the line so repay must pass exactly the covered sidecars oldest first; a repair path
  `annotate_payment(k, receipt_hash)` (holder or attestor) for payer-supplied or missing receipt
  hashes; no SAS read on the procurador in v1 (attestor co-signature is the gate). Also: backstop
  in v1 = admin alone after the delay (SECURITY wording); zero-debt closing payment named for
  indexers; interface change list opened at `docs/plans/2026-11-02-pr2-interface-changes.md` on
  the dev branch. A re-audit of the post-`4a440c0` delta is planned before PR 3.
- **Closures confirmed, 6d part 1 started** (`05a7b2e` accepted and pushed with the interface
  list through step 8): `pagares_paid: u16` in CreditLine.reserved 30..32 (free; layout stays
  313), repay must pass exactly the sidecars [cursor ..= last_touched] (`PagareAccountsMismatch`
  6039); receipt hash zero on repay means "to follow", `annotate_payment(draw_index, entry_count,
  receipt_hash)` by holder or attestor appends a Recibo entry (kind 11); attestor co-signature is
  the v1 gate on the procurador. Part 1 order: registry chain at draw → procuración → revocation
  → court annotation → cancel branches → backstop → repay entries.
- **Part 1 (a) accepted** (`ddca633`; 142 Rust + 11 JS): Pagare 353 bytes (`log_root`,
  `entry_count`, `last_kind`, `flags`, `cancel_receipt_hash`, `requested_at`,
  `requested_receipt_hash`; status 2 = paid-pending-cancel); twelve entry kinds (Emision 0 …
  Recibo 11); draw opens the chain with the Emision entry and emits `PagareEntry`; entry hash =
  sha256(asset ‖ kind ‖ from ‖ to ‖ hash ‖ stated_at ‖ entry_count) (asset first, so two pagarés
  with the same document hash in the same second have distinct chains); creation set = freeze
  delegate (UpdateAuthority), ImmutableMetadata, Attributes, AddBlocker; cancel thaws as the
  collection authority and burns as owner. CU at 120: draw 82.4k, cancel 44.5k. Push after each
  accepted piece.

## Points the coordinator still owns

- Whether the series' Core collection is created by `init_pool` or supplied like the share mint.
  Default until told otherwise: supplied by the admin and validated (same pattern as 2c).
- Exact `sponsor_role` semantics beyond the byte (nothing enforced on chain in this batch).

## How to report

First message: acknowledgement, the branch name, your reading of the three PRs, and anything in
the plans that contradicts the code you see. Then one message per PR step with the test and CU
numbers, and a message before any action that is hard to reverse. Cite files and lines. If a
command fails, send the output, not a summary.
