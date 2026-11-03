# Council of curators: design for PR 4

Status: draft for Mario's approval, 2026-11-03. Owner of the decisions: Mario (product), coordinator
(technical calls, delegated by Mario: "you decide the last word"). Technical input: dev session
analysis of Squads v4 (`af94153`, 2.1.0), kept with the dev.

## Intent

At launch, every credit line is approved by people, not only by KYC and code. A council of curators
reviews each borrower's expediente crediticio (loan contract, pagaré text, KYC/KYB result, business
documents) and attests it on chain. Only an approved line can be drawn. The same council acts for
the pool as holder of the pagarés: it endorses, revokes, annotates and cancels them, and it can stop
a credit or start collection. Later the council's power passes to an investors' DAO without a
program rewrite.

This replaces the single attestor key and most admin powers in elote_pool v1
(`v2/pool-and-lines` @ `8e6f0b6`, scripts `scripts/pr3` @ `a547737`). The devnet redeploy waits
for this change.

## Decided by Mario (2026-11-03)

1. KYC applies to borrowers and to curators. Investors do not go through KYC (pilot scope; whether
   this holds in production is open, see "Open for Mario").
2. The borrower requests the line; the council approves the full expediente once. One approval
   covers the whole credit (all draws).
3. The "attestation check" the program enforces is the council's approval of the expediente.
4. The council is a mini DAO: curators propose, the others vote, a threshold decides. Mario picks the
   curators and the threshold for now; an investors' DAO later.
5. No PSC certificate in the pilot. The proof is public Solana records: the borrower's wallet
   signature at each draw and the council's on-chain approval.
6. Endoso, revocation and cancellation of pagarés are council actions (the council represents the
   pool).
7. A council proposal to cancel a credit only blocks future draws. Collecting the debt early is a
   separate council proposal.
8. For now everything is council-gated; more roles may come later.

## Decided by the coordinator (technical calls)

- **The council is a Squads v4 multisig.** Its vault PDA (index 0) is the pool's `council`
  authority. Votes, proposals and the vote record live in Squads (audited, live on devnet:
  `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`). elote_pool never CPIs into Squads or into any
  caller-chosen program. Rejected: governance accounts inside elote_pool (more audit surface, the
  arbitrary-invoke risk class).
- **DAO later = rotate the authority.** `council_kind`: 0 = verified Squads vault, 1 = generic
  signer (for a Realms governance PDA). Rotation in two steps: `propose_council` (current council)
  then `accept_council` (signed by the new authority), so the pool never rotates to an address that
  cannot sign.
- **Split `open_line`.** `request_line` (borrower, plus sponsor where the terms need one) stores the
  terms, `agreement_hash`, `expediente_root`, `pagare_template_hash`, `terms_hash`; status
  Requested. `approve_line` (council) takes `expediente_root` and `terms_hash` as arguments, which
  must match (`ExpedienteMismatch`), checks the cap, sets status Approved and `approved_at`.
  `reject_line` (council) and `withdraw_request` (borrower) close the request and return the rent.
  The borrower never has to sign inside a Squads execution.
- **Draw by the borrower alone.** `draw` computes the pagaré hash on chain:
  `sha256(pagare_template_hash || line || draw_index || amount || due_at)`, so the borrower's
  signature covers a pagaré derived from the approved text. `signature_method` and
  `cert_fingerprint` stay in the arguments and the record (pilot: `wallet`, empty), so a PSC
  signature can be added later without a layout change.
- **Council instructions:** `approve_line`, `reject_line`, `endorse_pagare`, `revoke_procuracion`,
  `court_annotation`, `annotate_payment` (with the N-04 receipt override), `cancel_pagare` (holder
  branch), `suspend_draws` / `cancel_line` ("cancel the credit"), `propose_acceleration`,
  `withdraw_acceleration`, `accelerate_line`, `write_off`, `set_collection_period`,
  `propose_council`, resume after pause, pool parameters.
- **Procurador alone:** its `cancel_pagare` branch (PAGADO mark plus its PDA delegate).
- **Unchanged:** `repay`, `deposit`, `redeem`, `mark_in_collection` (anyone, after the third missed
  installment plus grace).
- **Collect the debt early:** `propose_acceleration` (council, emits a notice event) then
  `accelerate_line` after `acceleration_notice_seconds` (pool parameter, provisional floor 7 days,
  counsel sets the figure; the notice is addressed to the borrower); withdrawable before it fires. Same math as `mark_in_collection`
  without the missed-installment requirement. No Squads time_lock (it would delay every council
  action).
- **Guardian:** a key that can only pause the pool; only the council resumes. Held by Elote
  operations.
- **Threshold guard on chain (council_kind 0):** at `accept_council` and in every council
  instruction, check owner = Squads program, vault = PDA(multisig, 0) with stored bump,
  `config_authority` = default (autonomous multisig), threshold ≥ floor(voters/2)+1. Squads itself
  allows any threshold ≥ 1 and can lower it later, hence the per-instruction check. If the CU spike
  shows it does not fit, keep it at `accept_council` plus a cheap threshold read.
- **`valid_until` on every council instruction** (`ProposalExpired`): Squads proposals have no expiry
  and execute even when stale after a config change. Council instructions also bind to concrete
  data (hashes, draw_index).
- **Conflicts:** a curator cannot be a borrower (`approve_line` refuses if the borrower is a member
  of the multisig, council_kind 0). A procurador cannot be the pool, the council or the guardian.
- **Removed:** the attestor and `set_attestor`; the attestor backstop (`request_cancellation`,
  `cancel_delay_seconds`, `set_cancel_delay`, `Pagare.cancel_after`, `requested_receipt_hash`, the
  `backstop` field); kind 10 becomes reserved. The PAGADO mark protects the debtor from the moment
  of payment; counsel confirms.
- **Admin:** no rotating authority in PDA seeds. Pool seeds become `[POOL, asset_mint, series_id]`
  if the series shelf already carries a series id, else `creator` (pays and seeds `init_pool`, no
  other power). The dev picks with that rule and documents it.
- **Events (auditor LM-005):** LineRequested, LineApproved{expediente_root, terms_hash},
  LineRejected, RequestWithdrawn, AccelerationProposed, AccelerationWithdrawn, LineAccelerated,
  CouncilProposed, CouncilAccepted{old, new, kind}, PoolPaused{by}, PoolResumed.

## Settled after the builder check (coordinator's calls)

- **`terms_hash` is computed on chain**, never supplied: sha256 of a fixed encoding of the stored
  terms (principal, rate, term_end, schedule, sponsor and role, `agreement_hash`,
  `pagare_template_hash`, `expediente_root`). `request_line` stores the terms and the hash;
  `approve_line` takes the hash the curators computed off chain from the documents and refuses on
  mismatch. The encoding is specified once, in the client, and reproduced by the scripts and the
  curators' tooling (one test vector pins it). The council's approval therefore covers the pagaré
  template and the agreement.
- **Acceleration scope:** `propose_acceleration` applies only to drawn lines (Active or past the
  draw window); for never-drawn lines the council uses `cancel_line`. Proposing **suspends draws at
  once** (same effect as `suspend_draws`), so a borrower cannot drain the line during the notice;
  withdrawing the proposal does not resume draws by itself (the council resumes explicitly).
  Scheduled lines use decision 28's math. Bullet lines: whole principal due at the mark, ordinary
  interest accrued to that date kept, unaccrued interest forgiven pro rata by time, late interest
  from the mark; this mirrors decision 28's working rule and goes to Mario and counsel with it.
- **CU:** the 100,000 CU ceiling applies to elote_pool's own consumption per instruction. The Squads
  execution transaction may request a higher compute budget; the council tooling sets it. If an
  elote_pool instruction itself exceeds the ceiling, restructure (for example thaw and burn in two
  instructions) rather than raising the ceiling.
- **Pool creation:** seeds use `creator` (v1 has no series id). `init_pool` accepts only
  council_kind 0 and runs the full multisig check (owner, vault PDA index 0, `config_authority`
  default, majority threshold); it does not need the council's signature, since a vault PDA of a
  valid multisig can sign by construction. Kind 1 is reachable only by rotation. If the series is
  permissioned, `init_pool` requires the share mint's freeze authority to be the pool PDA, so the
  creator keeps no power over investors. Other small calls (rent payer inside Squads, pause scope:
  draws, deposits and new requests; repay and redeem stay open; the council rotates the guardian)
  are left to the builder and noted in ARCHITECTURE.

## CU spike results (2026-11-03, real Squads v4 binary in LiteSVM, throwaway)

- Squads overhead: about 20.6k fixed, plus about 0.9k per account in the message, plus about 60 CU
  per member.
- Full multisig check on every council instruction: 2.1-2.6k. Kept on every council instruction.
- Heaviest case, the council's `cancel_pagare` via Squads plus the check: about 83-86k for 3-10
  members, under the ceiling with about 14k margin (about 1k less once the attestor is gone).
  `endorse_pagare` about 68-77k, `approve_line` about 50-54k. `draw` leaves Squads and gets cheaper.
- CPI depth Squads, pool, Core, system works in practice.
- Council size capped at 10 members (the check refuses more), so the cost stays bounded.

## Goals

- A line cannot be drawn without the council's approval of exactly the expediente the borrower
  requested.
- Every council action is a multisig decision with a recorded vote, bound to concrete data, that
  cannot be replayed late.
- The authority can move to a DAO with no program change.
- The program stays within the 100,000 CU ceiling per transaction, including the Squads execution
  overhead.

## Non-goals

- Governance code inside elote_pool (proposal or vote accounts).
- On-chain KYC checks; curator and borrower KYC happen off chain before membership or request.
- PSC signatures in the pilot.
- Endoso en propiedad (part 2).

## Anti-goals (a failure even if it works)

- A single key able to approve a loan, move a pagaré or stop collection.
- Council actions that a stale or replayed proposal can trigger.
- A curator approving their own line.
- A role change that leaves the debtor without a way to get a paid pagaré cancelled.

## Constraints

- CPI depth: Squads (1) → elote_pool (2) → Core (3) → system (4); limit 5. It fits with no margin;
  nothing may add a level.
- CU: the council's `cancel_pagare` costs 48.9–58k today; inside `vault_transaction_execute` it may
  reach 70–100k. The first commit is a measurement spike; the design is final only after it.
- v1 rules: no backward compatibility on devnet; typed fields; minimal comments; subagents on Opus.
- Nothing is pushed without coordinator acceptance; no devnet deploy without Mario's word.

## Plan shape (dev's estimate, about PR 2's size)

1. CU spike with the Squads binary in LiteSVM (scratchpad, throwaway) → fixes the threshold-check
   placement.
2. Layout, errors, events.
3. `init_pool` with council, guardian; `propose_council` / `accept_council`.
4. `request_line` / `approve_line` / `reject_line` / `withdraw_request`.
5. `draw` by the borrower, on-chain pagaré hash.
6. Pagaré instructions to the council; procurador alone; backstop removed.
7. Council line instructions, acceleration with notice, guardian pause.
8. `valid_until`.
9. Integration test with the real Squads v4 binary (dumped, not deployed): a 2-of-3 multisig
   executes `approve_line` and `cancel_pagare` by CPI; CU pinned.
10. Codama client, scripts (`--attestor-keypair` out; `council-propose.ts`, `request-line.ts`,
    `approve-line.ts`), local e2e with Squads preloaded.
11. Docs (ARCHITECTURE, SECURITY, runbook, interface reference) → re-audit of PR 4.

Blast radius outside the program: decisions 26 and 30, legal-conditions C6a/C13/C14/C22, counsel
brief Q1/Q7, theOffice template conditions, mobile v8 prompt (new state "solicitud en revisión del
consejo"; no double signature), example figures.

## For counsel (to add to the brief)

1. Wallet-only signature of the pagaré without a PSC certificate (C6a dropped for the pilot; tesis
   2031391; CCom 97 fr. II exclusive control, including which wallet: Privy embedded or self-held).
2. The council as the SOFOM's credit committee and as the pool's representative for endoso and
   cancellation; curators' legal standing (apoderados or committee members); later an investors'
   DAO in that role.
3. Early collection by council decision: the contract clause it needs (fraud, false information).
4. Removing the holder-only cancellation backstop: is the PAGADO mark at payment enough?
5. The acceleration notice period.

## Answered by Mario (2026-11-03)

1. Investors without KYC for testing (devnet); production decided later.
2. First council: two curators, Gabriel and Mario, threshold 2 of 2. More curators join by council
   proposal (a Squads config transaction voted by the council). The majority guard accepts 2 of 2
   (floor(2/2)+1 = 2). Liveness risk noted: with 2 of 2, one lost key stops the council until a
   member is replaced, which itself needs both; adding a third curator removes that risk.
3. The curators are the admin. Guardian: each curator's own key can pause alone (fast in an
   emergency); only the council resumes. (Coordinator's reading of "guardian key the curators
   also".)
4. Maximum review time 15 days: `approve_line` refuses after `requested_at + 15 days`
   (`RequestExpired`); after that the borrower withdraws and recovers the rent, or requests again.
   The borrower sees only the result, not the curators.
5. The acceleration notice also goes by SMS to the KYC-verified number (backend, off chain).
6. Pool limits are council-gated: the council can raise the pool's lending limits (Mario: "the
   council approves to raise the pool limit 100k"); the figure and unit are confirmed with Mario.

## Left to the builder

Instruction and error names; field order; the exact fixed offsets read from the Squads multisig
account, pinned by a test against the real binary; test structure.
