# Pagaré token: the legal conditions mapped to the design

Date: 2026-11-01. Source: loans-gov-mexico draft brief
`/Users/wario/Documents/softlaw/loans-gov-mexico/pt-ars/phase4/brief_draft.md` (Phase 4 draft, citations
largely verified against the SJF and Diputados, editorial and devil's-advocate review pending;
final at `brief-tok/T-pagare-token-legal.md`). This file is the coordinator's mapping of its 31
conditions onto the program, the agreements engine, the templates and counsel. It supersedes the
"position" section of `2026-10-31-expediente-token-design.md` where the two disagree.

## The verdict, and what it does to the posture

The brief answers "is the token the pagaré" with **"no, not yet"** for the design as written, on
three high-strength points, and with **"yes, with conditions"** for a reconfigured reading:

| Hypothesis | Verdict | Meaning for us |
|---|---|---|
| H1: the Core asset without the text is the título | **Not sustained.** The asset lacks the six mentions of LGTOC art. 170 and art. 14 denies effects to a title that lacks them; 1a./J. 64/2001. | The sentence "the token is the pagaré" cannot be defended in that strict form. |
| H2: the signed data message is the título and the asset is its registration and circulation record inside the sistema de información | **Indeterminate with a defined route; "yes with conditions" C1-C5, C6a, C22, C26.** A ceiling, not a forecast of success in court: existence, holding after an endoso and ejecutividad stay open, and the three 2025 tesis (2031391, 2031392, 2031573) are dismissals of fintech lenders' digital documents. | **This is the posture to defend.** It keeps everything built so far: the asset, the custody in the pool, the endoso log, the cancellation. Only the wording and three mechanics change. |
| H3a: paper pagaré in custody, the asset as its registry | Robust fallback; the token loses legal function. | Contingency for collection; the program needs nothing extra. |
| H3b: electronic pagaré with a PSC signature | Equivalent to H2 with C6a. | Same as H2. |

Decision, adopted by Mario on 2026-11-02 with the grading "indeterminado, con ruta definida": **adopt H2 as the posture** ("the electronic
pagaré is the signed data message issued through our sistema de información; the asset is its
único registro de emisión, tenencia y circulación"), keep H3a as the collection fallback, and keep
the H1 question for counsel (Q2 below). The identity page, the counsel brief and the design doc
get that wording.

The other two high-strength failures are mechanics, both fixed below: a permissionless
`cancel_pagare` is not payment "contra su entrega" (arts. 17, 129; CCF 2088); and "sin CNBV" is
false for a SOFOM E.N.R., which needs CONDUSEF registration, a CNBV PLD dictamen and PLD/FT
supervision (LGOAAC 87-B, 87-P, 95 Bis; P./J. 2/2016). What holds is "sin autorización ni
supervisión prudencial de la CNBV".

## Conditions, who owns each, status

Labels from the brief: [L] legal requirement, [G] good practice, [U] uncertain. ⚠ = contradicts
the design as it was. Status: **adopted** (sent to the developer or written into a plan),
**engine** (softlaw-backend / agreements engine), **template** (legal agent via theOffice),
**counsel** (waits for the law firm), **ops** (runbook, governance).

| # | Condition (short) | Owner | Status and how |
|---|---|---|---|
| C1 [L] | The signed text carries the six art. 170 mentions; nothing lives only on chain; amount in words and figures, fixed day, place, both rates | template | template; `draw` carries `pagare_hash` of that text (already) |
| C2 [G] | `amount`, `currency`, `due_at` derive from the text; the attestor compares and refuses to sign on mismatch | engine | engine (`TermsMismatch` is the attestor's refusal; the program cannot read text) |
| C3 [L, form U] | The text names the vehicle, not the PDA, as beneficiary; the verifier shows the holder's name; the PDA is described as custody | template, verifier | template; verifier (`holder_attestation` display) |
| C4 [L, form U] | Holder, debtor and judge get the clear text and the NOM-151 constancia without the operator's discretion | engine | engine key management; verifier entitlement rules |
| C5 ⚠ [L / U vs 2031392] | Every endoso, procuración included, is a signed data message (endorsee name, signature, kind, place, date) that includes `pagare_hash` and the asset address; the asset stores `endoso_hash` | program + engine | adopted in the 6d design (endoso_hash in AppData and event); engine produces the message. Mario, 2026-11-01: the endoso en procuración (LGTOC 35) transfers the título's possession but not its ownership, is revocable, and the procurador may collect but not endorse en propiedad. Proposed mapping under A: Core owner stays the legal owner; procuración = log entry (kind, endorsee) plus an owner-managed burn delegate so the procurador can extinguish after collecting, revocable by the owner, cleared by a later endoso en propiedad. Core facts being pinned (owner-managed plugins under AddBlocker; approve/revoke by the owner; reset on TransferV1) |
| C6a ⚠ [G; meets 2031391] | Double signature: PSC-accredited certificate over the text plus the wallet key over `draw`; record `signature_method` and `cert_fingerprint` | program + engine | **adopted**: `draw` gains `cert_fingerprint` and `signature_method`, stored in sidecar and attributes |
| C6b [U] | SAT e.firma variant; not the only route until Q1 | counsel | counsel Q1 |
| C7 [U] | The vehicle's endosos also signed with the legal representative's FEA, besides the Squads vault | engine, ops | engine; powers file |
| C8 [G/U] | The text designates the system (program + engine; chain and Core as infrastructure, with a migration rule) and does not mention the endorsee restriction | template | template; the SAS gate stays in the program only |
| C9 ⚠ [U technical / G] | No power of vehicle, admin or attestor over an asset that left the pool en propiedad, except return from procuración, court-ordered transfer, and extinction with the holder's signature | program | **findings in** (`1024104`, three pinned tests against Core 0.15.3): a permanent delegate cannot be re-approved directly (`CannotRedelegate`); revoke resets it to the collection's authority (the pool PDA), which can then approve a new key or `None`; `None` is terminal; only the freeze delegate thaws, and frozen + `None` is stuck forever; the update authority of an asset in a collection can never leave it (so the pool keeps it through the collection; `AddBlocker` at creation would close the rest). Two coherent designs: **A** relinquish at endoso (thaw, revoke, approve `None`), after which the C9 exceptions are impossible and C5/C28 leave the chain; **B** retain the delegates on the pool PDA and bound their use to enumerated instructions (`endorse_pagare` gated and logged, `court_order_transfer`, `cancel_pagare` with the holder). **Mario decided A on 2026-11-01** (decision 27: the endoso transfers ownership, the pagaré lives on its own, the history travels with it): thaw and relinquish every permanent delegate at the first endoso, `AddBlocker` at creation, later endosos possible outside the program, no court-ordered transfer on chain, cancel of an endorsed pagaré burns as the owner. Counsel Q7 becomes a confirmation question (does the vehicle's unavoidable collection authority defeat the endoso en propiedad; does a court order against the holder suffice). Measured (`5092994`): `AddBlocker` costs ~4.2k CU at creation (draw would sit near 94k of 100k), kept as an option; the AppData data authority is fixed at creation (cannot move or go to `None`) and only the collection's authority adds or removes adapters, `AddBlocker` notwithstanding, so under A or B the vehicle remains the endoso log's only writer and could remove it: bounded by the instruction set (only `endorse_pagare` writes, nothing removes) plus governance, which is a further reason A does not deliver "no power" |
| C10 [G] | Update authority in a multisig with independent signers, published threshold, timelock, reproducible build, binary hash, event per version | ops | runbook, Squads; already decisions 3 and 17 of the register |
| C11 [G] | Upgrade policy that never changes the semantics of recorded endosos | ops | regression tests on the endoso log in CI |
| C12 [U] | Monitor Metaplex Core's authority; migration plan with holder signature | ops | runbook |
| C13 [G] | The attestor is not an affiliate of the vehicle or the borrower | ops, counsel | governance; today the attestor is Softlaw's engine, which is the vehicle's operator: flag to counsel (Q11) |
| C14 ⚠ [G] | Attestor rotation needs holder + vehicle + substitute (or a court); admin alone is not enough; key escrow | program (minimal) + ops | **adopted** minimal: the new attestor co-signs `set_attestor`; the rest is runbook and counsel |
| C15 [L if the attestor issues constancias / G otherwise] | NOM-151 constancias from an accredited PSC over pagaré, endosos and receipts; attestation limited to the checkable; insurance or bond | engine, ops | engine contracts a PSC; attestation wording |
| C16 [L] | Natural persons: adhesion contract, RECA and CAT if the creditor is an Entidad Financiera; pagaré rates equal the contract's | template, counsel | template; counsel Q4 |
| C17 [L substance / G technique] | Unique salt per document, off chain only, destroyed when the lock ends; privacy notice; express consent | engine | engine (`expediente_root` already salted) |
| C18 [G] | Borrower keys not reused across lines | app | app wallet policy |
| C19 ⚠ [G] | One pagaré per installment, fixed day, in series, acceleration clause in each; a draw's series endorsed as a block, or give up acceleration | **owner + counsel** | **decided by Mario, 2026-11-01 (decision 28)**: one pagaré per draw, partial payments, acceleration at the third missed installment (`max_missed = 3` in the templates; `mark_in_collection` records the acceleration; late interest on all remaining principal from the mark, impact assessment ordered). Per-installment assets discarded (≈0.0056 SOL each and dozens of Core CPIs per draw). Counsel Q6 becomes a confirmation (85/2011 fit, acceleration of an endorsed title, who declares it) |
| C20 [L if single pagaré] | A signed record of every payment linked to the asset | program + engine | **design proposed**: a permissionless `annotate_pagare(k)` that **decided 2026-11-02 (requisito legal per the final brief)**: `repay` takes `receipt_hash` and the touched sidecars as remaining accounts (oldest first by `drawn_through`) and appends an Abono entry to each (~5k CU per sidecar; the client splits payments touching more than eight); the entry that reaches coverage is kind Pagado and sets the sidecar to paid-pending-cancel; valid under the mandate clause in the pagaré text; the engine prepares the receipt before the transaction and obtains the NOM-151 constancia after confirmation |
| C21 [G] | The contract reference is descriptive ("en relación con"), never "conforme a" | template | template |
| C22 ⚠ [L act of holder + receipt / G FEA, NOM-151, archive, final burn / U form] | Holder's FEA receipt (`receipt_hash`); `cancel_pagare` needs the holder's signature and writes a persistent PAGADO mark; AppData archived; burn optional and last; receipt to the debtor; block in InCollection/WrittenOff | program + engine | **landed** (`7c87f38`): `cancel_pagare(k, receipt_hash)` signed by holder (the admin while the asset is in the pool) and attestor; coverage rule is principal only (`principal_repaid >= drawn_through`, oldest pagaré first; interest alone extinguishes nothing, and since repay settles interest first every unit of interest fixed up to that payment is also repaid); sidecar persists as the PAGADO mark with `paid_at`, `receipt_hash`, status; `PagareCancelled` names the holder; burn last, refused InCollection/WrittenOff |
| C23 [G] | Imputation rule agreed in contract and pagaré | template | template (late interest, interest, principal; oldest pagaré first) |
| C25 [L conservation / G independence] | Ten years' conservation by the creditor of messages, endosos, receipts, constancias | engine | engine; independent of the attestor |
| C26 ⚠ [L] | SOFOM E.N.R.: CONDUSEF registration, PLD dictamen, SIC, 95 Bis; SPV: LFPIORPI fr. IV; reword "sin CNBV" | counsel, ops | **adopted** in decision 21's wording; counsel Q4 |
| C27 [U] | Endosos to investors "sin mi responsabilidad" | counsel | counsel |
| C28 [U] | Endorsees en propiedad only institutional or qualified, in blocks, no publicity; investor-quality attestation | program (gate) + counsel | the SAS gate can carry an investor-class claim later; counsel Q4 |
| C29 ⚠ [G] | No "programa", "serie", "emisión", "inversión" in names and metadata | program, ops | **adopted**: asset named `PAGARE <line>-<draw>`, neutral collection name |
| C30 [G] | Assets denominated in pesos, not used as a means of payment | design | holds (currency attribute = the peso asset mint) |
| C31 [U] | Prior consultation with the CNBV before endorsing to investors | counsel | counsel |
| C32 [new, DA Phase 5] | After an endoso en propiedad by the vehicle: to whom does the debtor pay, how are the line's cash flows redirected, what happens to NAV and outstanding principal | **design + counsel** | **decided for the pilot (decision 29): procuración only, no en propiedad endosos by the vehicle in v1**; whole-line sale with repay redirection is the first extension, per-pagaré sale rejected for any visible version (theOffice design note 2026-11-02). Research states the legal consequences in Phase 6; the en propiedad code waits |

## Final brief of 2026-11-02: what it changed

Final at `/Users/wario/Documents/softlaw/loans-gov-mexico/brief-tok/T-pagare-token-legal.md`
(revision log `pt-ars/phase6/revision_log.md`). 34 conditions (12 legal, 16 good practice, 6
uncertain; 11 ⚠ measured against the 31-10 design, so adoption does not remove the mark). New:
C32 (payment destination, decided for the pilot in decision 29), C33 (H3a paper pagaré as the
collection contingency), C34 (evidence package for the lawsuit: engine deliverable). H2 is graded
"indeterminado, con ruta definida"; "sí, con condiciones" is the ceiling. Nothing marked
[requisito legal] is final until counsel answers Q1-Q4.

- **Payment entries (C-1)**: a payment or court entry is not an endoso, but art. 130 puts the
  payment annotation on the holder; an entry written by the program at `repay` needs a mandate
  clause in the pagaré text, otherwise the holder or procurador signs it. Template carries the
  mandate; shape decided on CU.
- **Attestor on cancellation (C-2)**: acceptable only if the receipt and the PAGADO mark exist at
  the moment of payment and only the burn waits. Adopted: the covering payment writes the Pagado
  entry and sets the sidecar to paid-pending-cancel; `cancel_pagare` burns later. Backstop
  adopted: holder-only cancellation after a short published delay (compatible with 17/129; the
  debtor also has consignación, CCF 2097-2102). Rotation stays the primary path but is slow.
- **Procuración pilot (§6.6)**: the procurador's standing rests on LGTOC 35 and CCom 1061 fr. II;
  the endoso en procuración must carry FEA (C5, C7, the 2031392 lesson); no notice to the debtor
  required (39, 41); one receipt per payment with FEA, NOM-151 and the art. 130 entry.
- **Conflicts the brief keeps**: holder endosos signed only with a wallet key remain exposed to
  the FEA requirement of 2031392 (Q1; none in v1); the power to remove adapters conflicts with
  append-only if an adapter holds title data (dissolved: the registry is in the sidecar and the
  asset carries only immutable attributes); never structure collection as a "pagaré domiciliado"
  with a third-party payer (LGTOC 173 párr. 2).
- **G7**: PAGADO mark plus NOM-151 archive before the burn closes the payment objection in good
  measure and the integrity objection only in part; the text must designate the sidecar and the
  archive as part of the system (C8) and access must not depend on the operator (C4).
- **Vehicle**: LGOAAC 11 Bis gives the system rules to the CNBV; 87-F gives a SOFOM an independent
  título ejecutivo (contract plus certified statement), a parallel route to the pagaré.
- **Still unverified by research (§8)**: consignación in the relevant venue, CNPCF 348-350, the
  85/2011 ejecutoria, whether the SAT e.firma suffices, oficio P090/2024, and every design fact we
  reported (the Core facts are now pinned in `tests/core_capabilities.rs`).

## Supplement of 2026-11-02 (endosos, procuración, acceleration): what it changed

Source: `loans-gov-mexico/brief-tok/T-pagare-token-legal-supplement-endosos.md`. Applied to the 6d
spec (dev message of 2026-11-02) and to decisions 27 and 28:

- **Registry**: leaves the holder-owned AppData (the holder could rewrite it; payment and court
  entries would depend on the holder's signature) and becomes a program-owned, append-only hash
  chain in the sidecar (`log_root`, `entry_count`, `last_kind`; full entries in events and in the
  engine). Compatible with A per Q-B.7: the vehicle keeps a bounded power to annotate, none to
  move, freeze or burn.
- **Holder endosos**: signature-verified entries, no attestor, no SAS gate (Q-B.6); attestor and
  gate bind only when the pool is the endorser. `kind` mandatory (art. 30). `revoke_procuracion`
  by the owner alone (arts. 35, 41). Termination entry written when an endoso en propiedad
  clears a procuración.
- **Procurador**: may receive payment, sign the receipt and extinguish (35, 129, 17); the burn
  delegate points at a program PDA so a burn without receipt is technically impossible (Q-A.3).
- **Court**: `court_annotation` (annotates, never moves; LGTOC 20, 341, 287 by analogy, CCom
  1168, 1067 Bis for apremio); late entries with two dates and the art. 28 constancia (Q-B.9).
- **Payments**: an entry per payment in the title's registry with `receipt_hash`, in one act
  with the receipt (art. 130, 8o fr. VIII); shape (inside `repay` or bundled) decided on CU.
- **Acceleration (Q14)**: the rule holds; moratory interest per missed installment from its own
  due date (the grace is an express waiver), on principal only; clause imperative with an
  objective trigger; the on-chain state is a record, not a declaration. Residual: 194/2005.

## Risks the brief names that the program answers now

- **Burn erases the record (G7, not verified by them).** Answered by keeping the sidecar as the
  persistent mark and archiving AppData before the burn (C22).
- **Reversed payment after burn.** The burn is last and only after the holder's signed receipt;
  a reversal is then a new claim, not a lost title.
- **Imputation.** The debtor may declare which debt a payment goes to (CCF 2092) and interest
  precedes principal "salvo convenio" (2094): the contract and pagaré state the order the program
  enforces (C23).

## Blocking questions for counsel (from the brief, verbatim in substance)

1. **Q1 Firma.** Which signature method survives tesis 2031391 in BC and Jalisco; is the double
   signature (C6a) viable; does the SAT e.firma count; how is art. 97 fr. II proved with an
   embedded wallet.
2. **Q2 Título.** Is H2 accepted; is art. 5o paras. 2-3 the "express" permission art. 5o Bis asks
   for.
3. **Q3 Sistema.** Do arts. 17 and 27 require an act of authority; would a judge apply the SHCP
   rules (PSC + NOM-151) by analogy.
4. **Q4 Vehículo y perímetro.** SOFOM E.N.R., fideicomiso or SPV; "en serie o en masa"; whether
   endoso without recourse closes LIC 2o/103; LRITF 15; prior CNBV consultation; circulation model
   (participations, endoso or both) and how the participations qualify.

High impact: Q5 endoso "en el propio mensaje de datos"; Q6 calendar (85/2011 vs 194/2005);
Q7 post-endoso control; Q8 cancellation equivalence. These go into the counsel brief's question 15
and 16 in place of the earlier wording.

## What changes in the three documents

- `2026-10-31-expediente-token-design.md`: the "position" section is superseded by H2; the
  issuance, custody, cancellation and endoso sections gain the adopted conditions above.
- `docs/decisions/2026-10-31-owner-review.md`: decision 24 reworded to H2 pending Mario; decision
  21 reworded for C26; new decision 26 (cancellation by the holder with receipt) and 27 (control
  passes with the endoso, C9).
- theOffice counsel brief: Q6 and Q15 wording, the H2 posture, the four blocking questions.
