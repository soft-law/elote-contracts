# Operations runbook: `elote_pool`

Status: **draft for the owner's approval** (review register item R-14, 2026-10-31). Nothing here
is in force until Mario signs it off. Names and contact details are placeholders in angle
brackets; fill them in before the first mainnet pool.

Scope: the on-chain program as of the Phase 1 hardening branch. Pool v2 items (Token ACL thaw,
attestor, pagaré token) are marked "after pool v2" and become binding when that batch ships.

## 1. Who holds which key

| Key | Holder | Used for |
|---|---|---|
| Pool admin (`Pool.admin`) | Squads vault `<index n>` of the Softlaw multisig, one vault per series (decision 15) | `open_line`, `write_off`, `set_paused`; after pool v2 also `set_attestor`, `suspend_draws`, `cancel_line` |
| Program upgrade authority | `<single hot wallet on devnet>`; a Squads multisig before mainnet (README "Deployments") | Replace the program |
| Keeper | `<backend service key, no funds beyond fees>` | `mark_in_collection` (permissionless; the key only pays fees) |
| Attestor (after pool v2) | Agreements engine signer, rotatable through `set_attestor` | Co-signs `open_line`, `draw`, `endorse_pagare` |
| Token ACL list authority (after pool v2) | `<backend ABL operator key>` | Adds and removes wallets on the series' allow list; freezes and thaws share accounts |

The admin is part of the pool PDA's seeds and cannot be rotated. Members rotate inside Squads.

## 2. Keeper: `mark_in_collection`

`mark_in_collection` is permissionless and never paused. It records a fact (the line missed
`max_missed` installments and the grace of the last of them has run); nothing moves. It must be
called promptly because `write_off` on a line in collection is only allowed after it.

| Item | Value |
|---|---|
| Cadence | Once a day at `<hh:mm>` UTC, plus a run within an hour of every `grace_end` crossing (`drawn_at + (covered + max_missed) * interval + grace`) |
| Input | Every `Active` scheduled line of every pool, read from the chain; arrears computed with `lineArrears` from `@softlaw/elote-pool-client` |
| Action | Send `mark_in_collection` for each line whose computed `covered + max_missed <= installments` and `now > grace_end` |
| Expected failures | `NotEnoughMissed` when a repayment landed between the read and the send: retry on the next run. `LineStatus` when another caller got there first: ignore |
| Alert | A line eligible for more than 24 h without a `LineInCollection` event pages `<on-call>` |
| Record | The `LineInCollection` event (pool, line_id, missed, remaining) is the record; the backend indexes it |

Known limit (SECURITY.md): on a short schedule with `covered + max_missed > installments`
collection is unreachable and only the write-off after `term_end + grace` remains. The keeper
reports those lines separately so the admin can act at the term.

## 3. Who calls what, and when

| Event | Who | Instruction | Preconditions and notes |
|---|---|---|---|
| New credit approved | Admin (Squads vault), borrower, sponsor if named; after pool v2 also the attestor | `open_line` | Agreement signed and hashed by the agreements engine; `principal <= NAV * max_line_bps / 10_000`; borrower is not the admin |
| Borrower takes the money | Borrower | `draw` | Before `term_end`; vault has the liquidity. After pool v2: per tranche, with the attestor and the signed pagaré hash |
| Repayment | Anyone (borrower, sponsor, third party) | `repay` | Never paused. Screen the `payer` of every `LineRepaid` against sanctions lists (decision 6) |
| Three installments missed | Keeper | `mark_in_collection` | Section 2 |
| Loss recognised | Admin | `write_off` | `Active` and `now > term_end + grace`, or `InCollection` and `now > oldest missed due + grace`. Out-of-court collection has run its course per the credit policy |
| Pause | Admin | `set_paused(true)` | Section 5 |
| Resume | Admin | `set_paused(false)` | After the incident review |
| Investor leaves | Holder | `redeem` | Paid from the vault balance only; a thin vault means a retry with fewer shares |

## 4. Revocation of a participant (after pool v2)

When an investor or borrower fails a KYC/KYB re-check, lands on a sanctions list or asks to
leave the programme, the three records that admit them are closed in this order. Each step is a
separate signature so a partial revocation is visible.

1. **Close the attestation.** The backend calls SAS `close_attestation` on the wallet's
   attestation under Elote's credential. From this moment the wallet cannot be an endorsee of any
   pagaré (`endorse_pagare` checks the attestation on chain).
2. **Remove the wallet from the allow list.** The ABL operator removes it from the series' Token
   ACL list. From this moment the wallet's share account cannot be thawed again.
3. **Freeze the share accounts.** The ABL operator freezes every share account the wallet holds
   in every series. A frozen holder cannot `deposit` (mint into a frozen account fails) or
   `redeem` (burn needs a thawed account); repayments on its lines keep working.

For a borrower also: the admin calls `suspend_draws` on every open line (no further tranche;
repayment unchanged). Record the three transaction signatures plus the reason code in the
compliance log at `<location>`. A revocation that must be reversed (false positive) runs the
same three steps backwards: thaw, re-list, re-attest, with the compliance officer's approval.

## 5. Pause: when and how

`set_paused(true)` blocks `deposit`, `redeem`, `open_line` and `draw`. It does not block
`repay`, `mark_in_collection` or `write_off`. Borrowers with an approved line cannot draw while
paused and their term keeps running, so a long pause can lapse an approved line.

Pause when: a suspected program bug that moves money wrongly; a compromise of the admin or
upgrade key; a court order; the asset issuer froze the vault. Do not pause for a single
borrower's default (that is collection) or for a KYC failure (that is section 4).

Procedure: the Squads proposal names the reason and the expected duration; `<two of n>`
members approve; the backend posts the `PoolPaused` event to the status page at `<URL>`;
the incident review decides the resume. A pause longer than `<7 days>` is reported to every
holder by `<channel>`.

## 6. Incident contacts (placeholders)

| Role | Name | Reach |
|---|---|---|
| On-call engineer | `<name>` | `<phone / pager>` |
| Compliance officer | `<name>` | `<email>` |
| Squads approvers (quorum `<2 of 3>`) | `<names>` | `<channel>` |
| Legal counsel | `<firm>` | `<email>` |
| Etherfuse (asset issuer and rails) | `<contact>` | `<email>` |
| Security disclosures | GitHub security advisory (SECURITY.md) | https://github.com/soft-law/elote-contracts/security/advisories/new |

## 7. Checks the program makes for you

- `open_line` refuses a line larger than `max_line_bps` of the NAV (`LineTooLarge`) and a line to
  the admin itself (`BorrowerIsAdmin`). The app computes the cap before signing; the error is the
  backstop.
- Every money instruction ends with `Pool::check()`: the NAV cannot overflow and the share supply
  never falls below the dead shares once anything was deposited (`InvariantBroken`). If it ever
  fires, pause the pool and page `<on-call>`: it means state the program considers impossible.
- A repayment that settles no late interest leaves the late-interest watermark where it was, so
  paying one unit repeatedly never erases accrued late interest.
