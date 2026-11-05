# Operations runbook: `elote_pool`

Status: **draft for the owner's approval**. Nothing here is in force until Mario signs it off.
Names and contact details are placeholders in angle brackets; fill them in before the first
mainnet pool.

Scope: the on-chain program `elote_pool` and the keys, services and people that operate it.

## 1. Who holds which key

| Key | Holder | Used for |
|---|---|---|
| Pool admin (`Pool.admin`) | Squads vault `<index n>` of the Softlaw multisig, one vault per series (decision 15) | `open_line`, `write_off`, `set_paused`, `set_attestor`, `set_collection_period`, `set_cancel_delay`, `suspend_draws`, `cancel_line`; as holder of the pagarés, `endorse_pagare`, `revoke_procuracion`, `court_annotation`, `annotate_payment`, `request_cancellation`, `cancel_pagare` |
| Program upgrade authority | `<single hot wallet on devnet>`; a Squads multisig before mainnet (README "Deployments") | Replace the program |
| Keeper | `<backend service key, no funds beyond fees>` | `mark_in_collection` (permissionless; the key only pays fees) |
| Attestor | Agreements engine signer, rotatable through `set_attestor` (the admin and the new key both sign); never the admin | Co-signs `open_line`, `draw`, `cancel_pagare`, `endorse_pagare`, `court_annotation`; may sign `annotate_payment` |
| Token ACL list authority | `<backend ABL operator key>` | Adds and removes wallets on the series' allow list; freezes and thaws share accounts |

The admin is part of the pool PDA's seeds and cannot be rotated. Members rotate inside Squads.

**Creating a series.** The admin creates two things before `init_pool`. The Metaplex
Core collection of the series' pagarés, with the pool PDA (derivable from the admin key and the
asset mint) as update authority and, as its name, the vehicle's legal-name hash prefix or a
neutral label the admin chooses: never a series label, and never the words "programa", "serie",
"emisión" or "inversión", which a court read as an offering in 2016609 (brief condition C29);
the URI public and non-personal or empty (no folio, no person). The assets the program mints are
named `PAGARE <line_id>-<draw_index>` for the same reason. And the share mint: the first
mainnet pool, and every CR series after it, uses a Token-2022 mint with `DefaultAccountState =
Frozen` and Token ACL's `MintConfig` as freeze authority, mint authority = the pool PDA, the
asset's decimals, no other extension. `init_pool` then records `permissioned = true`, and
`PoolInitialized.permissioned` (event) or `pool.permissioned` (client) is the fact anyone can
check before investing. In the same transaction, or before the first deposit, the
gate thaws the dead-shares account `["dead", pool]`; until then the first deposit fails with
Token-2022's `AccountFrozen`. The devnet demo uses a plain classic share mint and shows
`permissioned = false`.

## 2. Keeper: `mark_in_collection`

`mark_in_collection` is permissionless and never paused. It is the on-chain record of the
vencimiento anticipado (decision 28): from the mark every installment is due and late interest runs
on all remaining principal. It records a fact (the line missed
`max_missed` installments, past the due date of the last of them; there is no grace period); nothing moves. It must be
called promptly because `write_off` on a line in collection is only allowed after it, and the
pool's collection period counts from the mark.

| Item | Value |
|---|---|
| Cadence | Once a day at `<hh:mm>` UTC, plus a run within an hour of every `last_missed_due` crossing (`first_due_at + (covered + max_missed - 1) * interval`) |
| Input | Every `Active` scheduled line of every pool, read from the chain; arrears computed with `lineArrears` from `@softlaw/elote-pool-client` |
| Action | Send `mark_in_collection` for each line whose computed `covered + max_missed <= installments` and `now > last_missed_due` |
| Expected failures | `NotEnoughMissed` when a repayment landed between the read and the send: retry on the next run. `LineStatus` when another caller got there first: ignore |
| Alert | A line eligible for more than 24 h without a `LineInCollection` event pages `<on-call>` |
| Record | The `LineInCollection` event (pool, line_id, missed, remaining) is the record; the backend indexes it |

Known limit (SECURITY.md): on a short schedule with `covered + max_missed > installments`
collection is unreachable and only the write-off after `term_end` remains. The keeper
reports those lines separately so the admin can act at the term.

## 3. Who calls what, and when

| Event | Who | Instruction | Preconditions and notes |
|---|---|---|---|
| New credit approved | Admin (Squads vault), borrower, attestor, sponsor if named | `open_line` | Agreement signed and hashed by the agreements engine; `principal <= NAV * max_line_bps / 10_000`; the borrower is neither the admin nor the attestor |
| Borrower takes the money | Borrower, attestor, relayer as `payer` | `draw` | Per tranche, while the draw window is open (until the first due date, or `term_end` on a bullet line); the vault has the liquidity; the signed pagaré's hash. Each tranche mints its pagaré token into the series' collection, and the relayer funds about 0.006 SOL of rent for the asset and its record; the asset's half comes back at `cancel_pagare`, the record's stays locked as the PAGADO mark |
| Repayment | Anyone (borrower, sponsor, third party) | `repay` | Never paused. The client passes at most five pagaré records per `repay` and splits a larger payment; a payment that lands first can make an in-flight `repay` fail with `PagareAccountsMismatch`, and the client re-reads the line and retries. Screen the `payer` of every `LineRepaid` against sanctions lists (decision 6) |
| Pagaré paid | Admin (as holder while the asset is in the pool) or the procurador of record, and the attestor; the relayer as `payer` (the procurador's client also passes the program address `["procurador", pagare, procurador]`); if the attestor is unavailable, the admin files `request_cancellation(draw_index, receipt_hash)` and, after the pool's cancel delay (at least 7 days, `set_cancel_delay`), cancels alone with the same receipt | `cancel_pagare(draw_index, receipt_hash)` | The pagaré carries the PAGADO mark a repayment wrote (`PAGARE_PAID_PENDING_CANCEL`), whatever the line's status (`repay` marks the pagarés oldest first as the principal repaid reaches each `drawn_through`; interest paid covers nothing by itself; `repay` needs the touched pagaré records as remaining accounts, see `repaySidecars` in the client). The engine issues the FEA-signed receipt (LGTOC 129), archives the pagaré's registry (its `PagareEntry` events) under NOM-151, then asks for the burn with the receipt's hash; if the payer's receipt hash was zero or wrong, the admin's `annotate_payment` replaces it on the record before the burn; the `Pagare` record stays as the PAGADO mark. In collection or after a write-off only the pagarés a payment covered cancel; the others stay in force for the collection |
| Three installments missed | Keeper | `mark_in_collection` | Section 2 |
| Loss recognised | Admin | `write_off` | `Active` and `now > term_end`, or `InCollection`, `now > oldest missed due` and the pool's collection period (at least 30 days, `set_collection_period`) has run from the mark. Out-of-court collection has run its course per the credit policy |
| Pause | Admin | `set_paused(true)` | Section 5 |
| Resume | Admin | `set_paused(false)` | After the incident review |
| Investor leaves | Holder | `redeem` | Paid from the vault balance only; a thin vault means a retry with fewer shares |

## 4. Revocation of a participant

When an investor or borrower fails a KYC/KYB re-check, lands on a sanctions list or asks to
leave the programme, the three records that admit them are closed in this order. Each step is a
separate signature so a partial revocation is visible.

1. **Close the attestation.** The backend calls SAS `close_attestation` on the wallet's
   attestation under Elote's credential. From this moment the attestor refuses to co-sign an endoso
   to the wallet (`endorse_pagare` needs its signature; the program does not read the
   attestation itself).
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
