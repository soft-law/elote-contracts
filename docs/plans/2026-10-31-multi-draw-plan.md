# Tranched credit lines: contract plan

Date: 2026-10-31. Status: plan, not started. Source: product decisions of 2026-10-31 (partial draws
on an approved line; interest proportional to time; weekly, quincenal or monthly installments with
a minimum per cycle and partial payments; no fee inside the pool; revolving credit deferred).
Checked against `main` 3c1debd. Builds on `docs/plans/2026-10-24-series-shelf-contract-plan.md`;
the layout change below lands in the same breaking batch as pool v2. The survey of other protocols
that informed it is in `docs/research/2026-10-31-credit-lines-*.md` (Huma, Goldfinch, Maple,
Wildcat, TrueFi, Atlendis, Credix, Jet; every claim there cites a file and line of the repo read).

## What is asked

An approved line is a commitment of `principal`. The borrower draws it in one or more tranches,
each of any size up to what is still available, non-revolving (repaid principal is not drawable
again). Interest is proportional to the time each tranche is outstanding. Repayment is by
installments at a weekly, quincenal or monthly interval, with the installment as the minimum per
cycle and partial payments accepted, as the v2 schedule already does. Revolving credit (crédito en
cuenta corriente) is a different instrument and is deferred; the design notes for it are kept in
the research folder and are not part of this plan.

Legal shape, per the legal agent's drafting guidance received through theOffice on 2026-10-31
(pilot guidance pending counsel, not legal advice): disposiciones parciales fit inside the apertura
de crédito simple as a term of the contract; what keeps it "simple" is the **express**
non-revolving clause, because silence invites a cuenta corriente reading. The legal agent first suggested one
master agreement plus a hashed annex per disposición; theOffice then reported the owner's
document set as the contrato de línea de crédito at opening plus **one pagaré per disposición**,
so the per-draw hash is the pagaré's (open question 8 on custody). The withdrawal fee is a platform service fee, defined in the
platform terms and cross-referenced in the agreement's gastos clause, charged only when the
borrower sends funds to a bank account, never part of the rate. The program has no fee switch
(`ARCHITECTURE.md` §8) and this plan does not add one. Items the legal agent routes to licensed
counsel: apertura vs cuenta corriente article cites, whether pagarés are needed at all, CAT and
total-cost disclosure with variable draw dates and the SPEI fee, day count (360 or 365), late
interest terms, the sponsor's guarantee form (aval, fiador or obligado solidario), a
no-subrogation clause for third-party repayment, and notice and cure mechanics before collection.

## Facts from the code that shape the plan

| Fact | Where | Consequence |
|---|---|---|
| `draw` moves the whole `principal` in one transfer and flips `Approved -> Active` | `instructions/lines.rs:205-243` (`let amount = line.principal;` at `:217`) | Becomes `draw(amount)`. |
| `interest_due = principal * rate_bps / BPS`, fixed at opening; `rate_bps` is a flat rate for the whole term, not annual | open_line `lines.rs:83`, `state.rs:109-111`, `math.rs:31` | Proportional interest needs an annual rate and a time factor. Both change for version-3 lines. |
| `repay` computes `principal_left = principal - principal_repaid` | `lines.rs:304` | Must become `drawn - principal_repaid`. |
| `line_arrears` builds the schedule from `total = principal + interest_due` and caps late interest at `principal` | `lines.rs:386-397` | Both use `drawn`. |
| `mark_in_collection` and `write_off` compute remaining and loss from `principal` | `lines.rs:446-449`, `:515` | Both use `drawn - principal_repaid`. |
| Installment `k` is due at `drawn_at + k * interval` and counts as due at that very second; `cumulative_due` is a fraction of `total` | `math.rs:42-72`, `state.rs:118-134` | A tranche after an installment is due creates instant arrears. Draws on a scheduled line close strictly before the first due date (D3). |
| `open_line` requires `principal + interest_due >= installments` | `lines.rs:154-155` | A tiny first tranche would zero the installment amount and make `installments_covered` divide by zero (`math.rs:60-61`, `:75-82`). Every tranche must keep the schedule payable (D3b). |
| Late interest already accrues per second at an annual rate in bps, in `u128`, from an amount × seconds product | `math.rs:91-110` | Proportional interest per tranche is the same formula: `amount * rate_bps * seconds / (BPS * SECONDS_PER_YEAR)`. No new math shape. |
| `CreditLine.reserved` has 2 free bytes | `state.rs:118-134` | The account layout changes (line version 3). |
| Nothing is on mainnet; the devnet pool is a demo | series-shelf plan | A breaking line layout is acceptable now, once, with pool v2. |
| Events are append-only | `ARCHITECTURE.md` §7.8 | `LineDrawn` keeps its fields and is emitted per draw; new facts go in a new event. |
| `repay` is permissionless and stays so (decision 2026-10-31) | `lines.rs:256` | Unchanged. |
| `validate_schedule` allows 1 to 120 installments and any positive interval | `lines.rs:150-153` | Weekly (7 d), quincenal (15 d) and monthly (30 d) schedules already fit: 120 weekly installments is 2.3 years. Nothing to change for the cadence. |

## Design

### State: line version 3

Append to `CreditLine` (after `closed_at`, before `reserved`):

| field | type | meaning |
|---|---|---|
| `drawn` | `u64` | principal transferred to the borrower so far; `drawn <= principal` |
| `draws` | `u16` | number of tranches; `MAX_DRAWS = 64` |
| `availability_end` | `i64` | last time a draw is accepted (strict `<`); see D3 |
| `sponsor_role` | `u8` | `Promoter = 0`, `Guarantor = 1`; see D12 |

`principal` keeps its meaning as the approved commitment. `available = principal - drawn`.
`interest_due` keeps its meaning as interest charged to date, summed per tranche. `version = 3`
for every line opened after this lands. `InitSpace` grows by 19 bytes; the line PDA is created by
`open_line`, so no realloc path. Old v1/v2 line accounts on devnet will not deserialize under the
new struct; the devnet demo pool is redeployed, as the series-shelf plan already expects for pool v2.

Huma keeps terms and balances in two accounts (`huma.json:7870-8030` in the Solana survey). One
account is kept here: every pool instruction and client already keys on one `CreditLine`, and a
split would add an account to each of them for no gain at our size.

### Decisions (defaults confirmed or set by the owner on 2026-10-31)

- **D1. Interest proportional to time, per tranche.** On a version-3 line `rate_bps` is an
  **annual** rate. At each draw,
  `interest_due += amount * rate_bps * (last_due - now) / (BPS * SECONDS_PER_YEAR)`,
  rounded down once per tranche, where `last_due` is `term_end` on a bullet line and the last
  installment's due date, `schedule_end(drawn_at, interval, installments)` (`lines.rs:162-165`),
  on a scheduled line, since that is when the last payment is owed and it can be earlier than
  `term_end`. `interest_due` starts at 0 at opening. Undrawn principal
  carries no interest and no commitment fee. This matches how Atlendis prices a late draw
  (`protocol-v1 PoolLogic.sol:232-238`) and the owner's "interest proportional". The flat figure
  stays available for the agreement's "if fully drawn today" line as
  `principal * rate_bps * (term_end - opened_at) / (BPS * SECONDS_PER_YEAR)`, computed by the
  client, not stored. Version 1 and 2 lines keep their flat semantics; the field's unit is
  documented per version in `state.rs`.
- **D2. Non-revolving.** `available = principal - drawn`, which only goes down. Confirmed.
- **D3. Availability window.** Set at the first draw: `term_end` for a bullet line;
  `drawn_at + interval_seconds` (the first due date) for a scheduled line. A draw is accepted
  while `now < availability_end`, strictly, because an installment counts as due at the second it
  falls due (`math.rs:42-56`). At or after it, `draw` fails with `AvailabilityEnded`. Every tranche
  therefore lands before any installment is due, so `total` changes only while
  `cumulative_due(k) = 0`, and the schedule re-amortizes the drawn principal, not the limit, which
  is the Goldfinch mistake to avoid (`CreditLine.sol:279-287` schedules principal on
  `currentLimit`). The owner's answer ("due date finish all") is read as: at the first due date
  the draw window closes and everything from then on is repayment. Drawing across several
  installments would need per-tranche schedules (Jet's one account per draw with FIFO repayment,
  `idl-full.json:2088-2148`) and is out of scope.
- **D3b. No tranche may leave the schedule unpayable.** `open_line` requires
  `principal + interest_due >= installments` so that every installment is at least one unit
  (`lines.rs:154-155`). With `interest_due` starting at 0 and tranches of any size, a first draw
  of 5 units on a 12-installment line would make `installment_amount` zero and
  `installments_covered` divide by zero (`math.rs:60-61`, `:75-82`), so `repay` and
  `mark_in_collection` would fail with `Overflow` until another draw rescued the line. `draw`
  therefore requires, on a scheduled line, `drawn + interest_due >= installments` after the
  tranche (`InvalidSchedule`). The opening check stays as a bound on the full commitment,
  evaluated with the fully-drawn-today interest.
- **D4. Installments stretch with the draws.** On a scheduled line `installment_amount` and
  `cumulative_due` derive from `total = drawn + interest_due`, recomputed on every read. Nothing
  is stored, so a second tranche simply raises every installment. The minimum per cycle is the
  installment; partial payments are accepted with late interest first, then interest, then
  principal, exactly as v2 lines do today (`lines.rs:318-319`).
- **D5. Statuses.** First draw: `Approved -> Active`, sets `drawn_at` and `availability_end`.
  Later draws: `Active` only, not `InCollection`, not paused, `now < availability_end`,
  `amount <= available`, `amount <= vault.amount`. A full repayment of what was drawn closes the
  line (`Repaid`) and forfeits the undrawn remainder; the app should warn before repaying in full
  while availability is open.
- **D6. Liquidity per tranche.** No reservation of undrawn commitments in the vault. A tranche can
  fail with `InsufficientLiquidity` and be retried, as a whole draw can today. Known limit: a large
  redemption can starve a committed borrower; reserving commitments would move them into the NAV
  model and is a separate decision.
- **D7. Single-draw lines are not a separate kind.** A borrower who wants the whole amount draws
  once. The agreement template can still say "one disbursement"; the program does not enforce it.
- **D8. Draw gates are checked before anything is written**, in one helper (Huma
  `Credit.sol:583-607`; avoid Goldfinch's check-after-mutate at `CreditLine.sol:172-173`).
- **D9. Each tranche carries its document hash.** `draw(amount, document_hash)` takes the keccak of
  the signed per-disposición document (the pagaré, per the document set reported on 2026-10-31,
  plus the schedule annex if separate), rejects a zero hash as `open_line` does for the agreement
  (`lines.rs:78`), and emits it in `LineDrawState`. It is not stored: the event is the
  record, and the master agreement's hash stays the only hash on the account. The app reads the
  annex back from the agreements engine by that hash, as it does for the agreement today
  (`ARCHITECTURE.md` §0).
- **D10. The admin can close the draw window early.** A new `suspend_draws` instruction (admin
  signer, `has_one = admin`) sets `availability_end = now` on an `Approved` or `Active` line, so
  draws stop on a KYC or KYB failure or a default event, which the legal agent lists as a required
  lender right. It never touches repayment, and it is not a pause: the pool-wide `set_paused`
  stays as it is. Emits `LineDrawsSuspended { pool, line_id, available_forfeited }`.
- **D9 extended, D14 and D15 (pagaré token).** Per `docs/plans/2026-10-31-expediente-token-design.md`
  (owner, 2026-10-31): **the token is the pagaré.** `draw` mints one Metaplex Core asset per
  disposición by CPI, frozen in the pool PDA, with a `Pagare` sidecar PDA `["pagare", line,
  draw_index]`; `draw` and `endorse_pagare` require the pool's `attestor` co-signature (D14); the
  Pool gains `attestor` and `collection` (64 bytes, pool v2 batch); `CreditLine` gains
  `expediente_root: [u8; 32]` at opening (D15, 51 bytes of growth in total). New instructions
  `cancel_pagare`, `endorse_pagare`, `set_attestor`; tests and estimate in that document.
- **D12. Two sponsor roles, one byte.** The loans-gov-mexico research
  (`research-brief-tokenized-securities.md`, Finding 2) reads the Ley de Disciplina Financiera as
  making a state or municipality a debtor the moment it gives an aval, a guarantee, a first-loss
  reserve from public funds, a revenue pledge or control of the vehicle (LDF art. 2 XI, art. 24),
  which triggers the full regime (two-thirds of Congress, competitive process, RPU). A public
  sponsor must therefore co-sign as a **promoter** only: referral, training, data, no payment
  obligation. A private sponsor, or development banking through a partner SOFOM, can be a
  **guarantor** with full liability, since the research's RCT evidence says a co-signer whose
  liability was halved cut repayment by about a fifth (`sr/synthesis.md` §2.2.6). Version-3 lines
  carry `sponsor_role: u8` (`Promoter = 0`, `Guarantor = 1`), set at `open_line` and emitted in
  `LineOpened`'s successor event `LineSponsorRole { pool, line_id, sponsor, role }` (append-only
  rule). The program enforces nothing on it: it is the on-chain record of which kind of promise
  the sponsor made, for the agreements engine and for counsel. Reading of the LDF is the
  research team's, with no SHCP criterion **no verificado**.
- **D13. Concentration cap per line, in Phase 1.** Nafin's network rules cap exposure at 30% of
  capital per legal entity and 10% per natural person (`N-guarantees-etherfuse-liquidity.md` §A2).
  The register already has "the admin can open a line for 100% of the vault" as P1-1. Phase 1
  adds `max_line_bps` to `init_pool` (a share of the NAV at `open_line`, default 1,000 bps) and a
  `LineTooLarge` error; the tranched plan inherits it unchanged, because the cap binds the
  commitment, not the tranche.
- **D11. Late interest basis, flagged for counsel.** On a v2 line late interest accrues on
  `overdue`, which is principal **and interest** fallen due and unpaid (`math.rs` `Arrears.overdue`
  and `late_interest`). The legal agent notes that charging interest on overdue interest is
  capitalisation and needs express agreement in Mexico. Decided in the owner review: late interest
  accrues on the **overdue principal part only**, in this batch (open question 6). The program's
  year is 365 days (`SECONDS_PER_YEAR`, `state.rs:35`), so the template must say 365, not 360.

### Instruction changes

| instruction | change |
|---|---|
| `open_line` | `interest_due = 0`, `drawn = 0`, `draws = 0`, `availability_end = 0`, `version = 3`. The schedule bound (`lines.rs:154-155`) uses the fully-drawn-today interest. `LineOpened.interest_due` keeps its meaning and is therefore 0 (open question 3). |
| `suspend_draws` (new) | admin only; `availability_end = now` on an `Approved` or `Active` line (D10); emits `LineDrawsSuspended`. |
| `draw(amount, document_hash)` | new arguments (D9). Checks in order: not paused; status `Approved` or `Active` (`LineStatus`); `amount > 0` and `amount <= principal - drawn` (`WrongAmount`); `now <= term_end` (`TermEnded`); on the first draw of a scheduled line, `ScheduleEndsAfterTerm` as today; on later draws `now < availability_end` (`AvailabilityEnded`); on a scheduled line `drawn + interest_due >= installments` after the tranche (`InvalidSchedule`, D3b); `draws < MAX_DRAWS` (`TooManyDraws`); `amount <= vault.amount` (`InsufficientLiquidity`). Writes `drawn += amount`, `draws += 1`, `interest_due += interest(amount, rate_bps, term_end - now)`, `pool.outstanding_principal += amount`; first draw also sets `status = Active`, `drawn_at = now`, `availability_end`. Emits `LineDrawn` (as today, `amount` = this tranche) then `LineDrawState { pool, line_id, amount, drawn, available, interest_due, availability_end }`. Huma's draw event lacks the outstanding and the room left (`huma.json:8153-8184`); ours carries both. |
| `repay` | `principal_left = drawn - principal_repaid`; everything else as today. |
| `line_arrears` | `total = drawn + interest_due`; `principal: line.drawn` for the late-interest lifetime cap. |
| `mark_in_collection` | `remaining` from `drawn`. |
| `write_off` | `loss = drawn - principal_repaid`. Status arms unchanged (`lines.rs:501-510`); an approved commitment never drawn lapses at `term_end` with nothing to book. |

New errors: `AvailabilityEnded`, `TooManyDraws`. New events: `LineDrawState` (with
`document_hash`), `LineDrawsSuspended`. `LineDrawn` is emitted once per tranche, so indexers that
summed `LineDrawn.amount` keep working.

### Invariants to add to `ARCHITECTURE.md` §7

- `drawn <= principal`, and `drawn` never decreases.
- `principal_repaid <= drawn` (version 1 rejects, versions 2 and 3 clamp, as today).
- `pool.outstanding_principal` equals the sum over open lines of `drawn - principal_repaid`.
- `interest_due` on a version-3 line equals the sum of the per-tranche figures, each computed at
  its draw time; it never changes except at a draw.
- `late_collected <= drawn` on a scheduled line.

### Tests (LiteSVM, `programs/elote_pool/tests/lines.rs` and `schedule.rs`)

1. Two tranches sum to the commitment; the third fails with `WrongAmount`; `outstanding_principal`
   and the NAV move per tranche.
2. Interest: a tranche drawn with half the time to the last due date left owes half the
   annual-rate interest of one drawn at the first draw, to the unit; on a scheduled line whose
   `schedule_end` is before `term_end`, the tranche accrues to `schedule_end`, not `term_end`; a half-drawn line repaid in full needs exactly
   `drawn + Σ interest(tranche)`; the sum over tranches is within `n - 1` units of a single draw
   of the same total at the same times (`proptest` in `math_props.rs`).
3. Scheduled line: a second tranche before the first due date raises every installment; a tranche
   at exactly `drawn_at + interval` fails with `AvailabilityEnded` (boundary) and one second
   earlier succeeds; arrears before and after the second tranche are what `math::arrears`
   predicts (vector test).
4. Scheduled line with 12 installments: a first tranche of 5 units fails with `InvalidSchedule`;
   a tranche that brings `drawn + interest_due` to 12 succeeds and `repay` works (D3b).
5. Weekly, quincenal and monthly schedules each run a full cycle with two tranches, three partial
   payments and one missed installment past grace; `mark_in_collection` behaves as on a v2 line.
6. A tranche while `InCollection`, while paused, after `term_end`: all fail. A tranche larger than
   the vault fails with `InsufficientLiquidity` and succeeds after a deposit.
7. Full repayment of a half-drawn line closes it; a further draw fails with `LineStatus`.
8. Write-off of a half-drawn line books `loss = drawn - repaid`. `MAX_DRAWS` reached fails with
   `TooManyDraws`.
9. Compute: `draw` stays well under the 100k CU ceiling with the extra multiplication.

### Then

Codama regen for `clients/js`; `scripts/open-line.ts` unchanged; a `scripts/draw.ts --amount`;
`README.md` instruction table (`draw` row, line 88) and the schedule paragraph; `ARCHITECTURE.md`
§5.5, §7, §8; `SECURITY.md:24` ("draw the whole principal"). The app's draw screen takes an amount
and shows `available` and `availability_end` (hand the `LineDrawState` event shape to the app
team). The agreements engine gets the template variant the legal agent drafts.

## Open questions, all decided in the owner review of 2026-10-31

Every default below is now the decision (`docs/decisions/2026-10-31-owner-review.md`), with two
changes: question 1 is **(a)**, and question 6 is **overdue principal only**.

1. **What "interest proportional" means on an installment line (D1).** **Decided: (a)**, interés
   ordinario per disposición, with interés moratorio as a separate late penalty (Mario via
   theOffice, 2026-10-31; owner-confirmed). The two readings, for the record:
   - **(a) Per tranche, to maturity** (the plan as written): each tranche owes
     `amount × annual rate × (term_end − draw time) / year`, fixed at the draw, split across the
     installments. Same shape as today's v2 total, so no change to the schedule math. A borrower who
     repays principal early still owes that interest.
   - **(b) On the saldo insoluto per installment**: interest for each period on the principal
     still outstanding at its start, a real tabla de amortización. Interest falls as principal is
     repaid. This changes `math.rs` (per-period interest, `cumulative_due` no longer a fraction of a
     fixed total) and touches v2 behaviour, which already charges on the undeclining principal, so
     the question predates this plan. Roughly two more days and a reviewer pass on the math.
   Default: (a) for the batch with pool v2, (b) as a follow-up if the template and counsel want
   it. Either way `rate_bps` becomes an **annual** rate on version 3 lines, flat-for-the-term on
   versions 1 and 2, documented per version.
2. **Repaid-in-full forfeits the remainder (D5).** Default yes; the legal agent's clause list
   already has "automatic cancellation of the undrawn remainder at window close, full repayment,
   early termination or collection".
3. **`LineOpened.interest_due` reports 0.** Default yes, keeping the field's meaning; the
   fully-drawn-today figure is in the signed agreement and derivable by the client.
4. **Reserve undrawn commitments in the vault (D6)?** Default no.
5. **Devnet continuity.** The layout change breaks the existing devnet demo lines. Redeploy when
   pool v2 lands.
6. **Late interest on overdue interest (D11).** **Decided: overdue principal only**, in this
   batch. `math::arrears` accrues late interest on the unpaid principal part of each overdue
   installment, and the v2 vector tests in `tests/schedule.rs` and `tests/vectors.rs` are
   regenerated. Pre-mainnet, so no live line changes behaviour. Day count stays 365 days in the
   program; the template says so.
7. **Granularity of interés moratorio.** The reported document set has moratorio accruing "per
   period of delay (día, semana, quincena or mes, as the fund configures)". The program accrues it
   per second (`math.rs:91-110`), which is the continuous version of a daily rule and strictly
   lower than any whole-period rounding. Default: keep per-second accrual and let the template
   describe it as daily; if counsel wants whole periods, `late_interest` gains a `period_seconds`
   and rounds elapsed time up. Decide with question 6.
8. **One pagaré per disposición: custody.** With the document set settled as contrato de línea
   de crédito at opening plus one pagaré per disposición, the per-draw hash (D9) is the hash of
   the signed pagaré bytes, plus the schedule annex if separate. LGTOC 170's determinate sum is
   satisfied per disposición, but the creditor must hold the título, and a hashed JSON alone does
   not satisfy that: electronic pagaré custody and conservation are a counsel-plus-engineering
   item for the agreements engine before any merchant signs **no verificado**. The program side
   is unaffected: it stores nothing but the master hash and emits the pagaré hash per draw.

## What the lending evidence says about these choices

From the loans-gov-mexico research (2026-10-31; files under
`/Users/wario/Documents/softlaw/loans-gov-mexico/`, mostly RCTs outside Mexico, certainty low to
moderate), kept here so the next person does not redo the search:

- **Weekly, quincenal or monthly is fine.** Moving from weekly to monthly did not change
  first-loan default (Field & Pande, India); the schedule cadence this plan offers is supported.
- **Avoid an initial grace period.** A payment holiday at the start raised default by 6 to 9
  points (Field et al.). The program has none: the first installment falls one interval after the
  first draw. Keep it that way.
- **A limited number of postponements may help.** BRAC's vouchers to skip a cuota cut default by
  about a third and raised business income. Not in this plan; a `skips_allowed` parameter is a
  candidate for a later version, never an open-ended grace.
- **Co-signer liability must be full** (D12). Group liability is not needed for repayment.
- **Graduation (bigger tranches after good payment) has no clean evaluation** either way; the
  tranche cap and the availability window are exposure controls, not validated risk reducers.
- **Loss sizing for the series documents, not the program:** the research recommends PD 12 to
  20% a year (stress 30 to 40%), LGD 80 to 90%, expected loss 10 to 18% for uncollateralised
  Mexican micro and small business credit (`M-pagares-recovery.md` §3.7); a 65% LGD needs
  collection before 90 days, an obligado solidario and captured cash flows together. First-loss
  tranches in Goldfinch and Maple were exhausted by off-chain conduct, and recovery went through
  courts.
- **CAT.** Every program or municipal rate the research read is published as a plain rate, and
  no source says how to disclose total cost for a stablecoin-denominated loan. The events this
  plan emits (`LineOpened`, `LineDrawState`, `LineRepaid`, `LateInterestCollected`) carry every
  amount and time needed to compute the CAT in MXN off chain; the SPEI fee lives in the platform
  terms and must be added there.
- **Pagaré holder.** A PDA cannot be the tenedor of a pagaré (LGTOC arts. 23, 38, 170); a
  persona (SOFOM, fiduciario or SPV) holds the paper and the chain tracks the economic interest.
  That is what D9 already assumes: the program stores and emits hashes only.

## Deferred: revolving lines

Removed from scope on 2026-10-31. If it returns, the design to start from is: availability =
limit − outstanding; interest checkpointed before every balance change with the weighted-seconds
form of `math.rs:108-113`; interest billed per cycle with draws blocked while any bill is unpaid;
principal at maturity; a `close_line` instruction. The reference implementations are TrueFi's
`AutomatedLineOfCredit.sol:324-326`, `369-373` and Huma's `Credit.sol:583-607`, both in
`docs/research/`. It would add about 50 bytes to the line and four to five days of work, and it
needs its own legal template (crédito en cuenta corriente).

## Estimate

Program and tests: 2 days, plus half a day for the late-interest split in `math::arrears` and the regenerated v2 vectors (decision 11). Docs, client regen and scripts: half a day. Lands with pool v2 (one
breaking batch, one external review before mainnet).
