# elote-contracts

**Mexican loans on Solana.** The `elote_pool` program behind [elote.baby](https://elote.baby):
signed legal contracts become programmable loan contracts on Solana that carry out the repayment
terms, and depositors share the interest that borrowers repay.

> Demo on Solana devnet with test assets. Not an offer of securities.

This is the production repository for the program, its TypeScript client
(`@softlaw/elote-pool-client`, in [`clients/js`](clients/js)) and the demo scripts. The app lives in
`soft-law/elote-web` (private), the documentation in `soft-law/elote-docs`, and the agreements server in
`soft-law/softlaw-backend` (private).

Hackathon snapshot: https://github.com/soft-law/elote-baby/tree/colosseum-2026 (program, app,
agreements server and the Meteora launch as submitted).

## The `elote_pool` program

`elote_pool` is a Solana program (Anchor 1.2.1) for a liquidity pool with a share token.
Investors deposit the pool asset and receive shares. A council — vault 0 of a [Squads](https://squads.so)
multisig — approves credit lines for entrepreneurs from the pool. When borrowers repay, the interest
stays in the pool and every share is worth more. A recognised loss lowers the value of every share once.

The demo asset is **tUSDC (devnet test token, no value)**: a plain SPL token created by the demo
script; lines and pagarés are in MXN. Nothing here holds or moves real money.

Documentation for investors, borrowers and developers (how the pool works, credit lines, the app,
the agreements server, program reference and addresses) lives in `soft-law/elote-docs`.

## What it is

- **Shares that appreciate.** Accounting follows ERC-4626:
  `shares = assets * supply / NAV` on deposit, `assets = shares * NAV / supply` on redemption,
  with a dual price (decisions 40 and 40b, as Maple v2): a **deposit** is priced at the NAV with
  no provision (`vault + outstanding principal`, any impaired book at par, so a recovery hands an
  entrant nothing), and a **redemption** at the provisioned NAV
  (`vault + outstanding principal - impaired cost × provision_bps`). Both legs round down, in
  favour of the pool. A front end shows both prices.
  Interest counts when it is collected; nothing accrues.
- **A council, not an admin.** Every privileged action — approving a line, a draw's limits, pausing,
  writing off, publishing the exchange rate — is taken by a council that is vault 0 of an autonomous
  Squads v4 multisig (at most ten members, a majority threshold). The program checks the multisig on
  every council instruction, and each decision carries a `valid_until` it is refused past, at most 15
  days ahead (Squads proposals never expire). The council can hand over to a new one in two steps
  (`propose_council` / `accept_council`).
- **Dead shares.** The first deposit (at least one whole unit, and at least 1,000 × `DEAD_SHARES`)
  locks 1,000 of its shares in a share token account owned by the pool, which nothing can move. The
  supply never falls below that floor, so a donation to the vault cannot round later deposits down to
  nothing. `deposit` and `redeem` also take a minimum the caller accepts (`min_shares_out`,
  `min_assets_out`).
- **Pesos on the title, dollars in the vault.** Lines and pagarés are denominated in MXN (centavos
  on chain); the vault holds a dollar asset (USDC, or Etherfuse's MEXe). A draw delivers the peso
  tranche as asset units at the FIX (Banxico's MXN/USD rate) in force, which the borrower signs;
  repayments are credited in MXN at the FIX of the payment. The FIX is published by a `post_fix`
  feeder key within a daily band and checked against RedStone's on-chain MXNe reference, or by the
  council outside the band; the realized FX gain or loss on each repayment is recorded.
- **Credit lines from the pool.** A borrower `request_line`s a commitment — the principal, an annual
  rate, the term, the hash of the signed credit agreement and a commitment to the credit file — and
  the council `approve_line`s it within its review window and the line cap. The borrower then draws
  the commitment in tranches while the draw window is open, each tranche against a signed pagaré.
  Ordinary interest accrues per second (ACTUAL/365) on the principal outstanding, until the last
  due date or the acceleration (decision 48): every principal repayment lowers it from that second.
  Repayments go to late interest, then the ordinary interest accrued and unpaid, then principal,
  and never take more than is due.
  Anyone may repay on the borrower's behalf; the payer is recorded in the event.
- **Pagarés as tokens.** Each draw mints a pagaré as a Metaplex Core asset, held frozen by the pool,
  with an on-chain registry (a hash chain of entries) that records every act on the title: issuance,
  payments, endoso en procuración to a collection agent, court annotations and its final extinction.
- **Installments, arrears and late interest.** A line can instead be repaid in installments (see
  [Installments and collection](#installments-and-collection)). Missed installments are computed
  from what was repaid, late interest accrues on the overdue principal from the first day of delay
  (there is no grace period), and a line with three missed installments goes to collection.
- **Optional sponsor.** A sponsor can be named on a line; it co-signs the request and may pay on the
  borrower's behalf. A promoter refers and trains; a guarantor takes liability (recorded only). It
  never touches the pool's money otherwise.
- **Pause.** The council, by vote like any other decision, can pause deposits,
  redemptions, requests, approvals and draws; only the council resumes. Repayments, moving a line to
  collection, writing off an overdue line and unpausing are never blocked.

## Accounts

| Account | Address | Holds |
|---|---|---|
| `Pool` | PDA `["pool", creator, asset_mint]` | the council (a Squads multisig vault), the `multisig` and any pending council, the creator; the asset, vault, share token and dead-shares account; the series' Metaplex Core collection; outstanding principal and running totals; the line cap (`max_line_bps`), the outstanding-principal cap, the collection period and the acceleration notice; the FIX in force (`fix_rate`, `fix_date`, when posted), the FIX feeder and bounds, the realized FX; the asset's issuer controls (`asset_controls`), `permissioned` and the pause flag |
| share token | a mint the creator creates and hands to `init_pool` | classic SPL Token or Token-2022 with the asset's decimals, mint authority = the pool PDA, supply 0, no extension that takes or hides value; frozen by default (Token-2022 `DefaultAccountState`) for a permissioned series under Token ACL |
| collection | a Metaplex Core collection the creator creates and hands to `init_pool` | the series' collection of pagaré assets; its update authority must be the pool PDA, so only the program mints into it |
| vault | PDA token account `["vault", pool]`, owned by the pool PDA | the pool's liquid assets |
| dead shares | PDA token account `["dead", pool]`, owned by the pool PDA | the 1,000 shares locked by the first deposit |
| `CreditLine` | PDA `["line", pool, line_id (u64 LE)]` | the borrower, sponsor and payer, the terms hash, agreement hash and expediente root, status, the principal, rate and term, what was drawn and repaid (in MXN and the USDC it cost), the accrual of ordinary interest (`principal_seconds` to `accrued_at`), the draw window and first due date, the installment schedule (scheduled lines), late interest collected and settled, the payment cursor `pagares_paid`, and the acceleration (`accelerated_at`, a pending `acceleration_due_at`, the saved window) |
| `Pagare` | PDA `["pagare", line, draw_index (u16 LE)]` | the program's record of one draw's pagaré: the asset's address, the pagaré hash derived on chain and how the pagaré was signed, the hash, signature and day of the message the borrower signed (decision 55), the cotejo key that co-signed the draw (decision 56; the zero key when none was required) and the sealed copy's hash (decision 57), the amount and the line's drawn total through it, when it was issued and when it is due, the registry (a hash chain of entries whose content is in the `PagareEntry` events), the procurador of record, the FIX and USDC the tranche was delivered at (`fix_rate`, `fix_date`, `usdc_delivered`, `delivery_hash`), and once paid the "PAGADO" mark (status, paid_at, the payer's receipt hash) and the delivery and aval flags; never closed; no folio |
| pagaré asset | PDA `["pagare_asset", line, draw_index (u16 LE)]`, owned by the Metaplex Core program | the pagaré token itself: a Core asset in the series' collection, owned by the pool PDA and frozen by a permanent freeze delegate whose authority is the collection's (the pool PDA), with immutable metadata, locked attributes (pagaré hash, signed message hash, amount, currency, issue and due dates, line, draw index, certificate fingerprint, signature method) and an `AddBlocker`; no transfer or burn delegate: the pool owns the asset, only the program signs for it, and a procuración lends no power over it (decision 38). `deliver_pagare` thaws it, renounces the freeze and hands it to its debtor |

Every parameter is a typed field (Borsh, no padding), and each account ends in `reserved` bytes,
created zero, for fields added after mainnet: 48 on `Pool`, 64 on `CreditLine`, 28 on `Pagare`. With
the 8-byte discriminator the sizes are `Pool` 591, `CreditLine` 528 and `Pagare` 527 bytes
(`tests/layout.rs`); ARCHITECTURE.md §3 gives every offset. A bullet line (single maturity) has
`installments == 0` and the other schedule fields at zero. Clients read the typed fields;
`lineSchedule(line)` in the client returns a line's schedule, or null for a bullet line, and
`clients/js/src/schedule.ts` computes arrears exactly as the program does.

## Instructions

Most privileged instructions are council instructions: they are signed by the council (vault 0 of the
Squads multisig) and take `valid_until` as their last argument, refused past it (`ProposalExpired`) or
more than 15 days ahead (`ProposalTooFar`). `init_pool` does not.

| Instruction | Signers | Effect |
|---|---|---|
| `init_pool(params)` | creator | creates the pool, its vault and its dead-shares account around an asset mint of 6 decimals (`AssetDecimalsInvalid` otherwise: the FIX conversion counts in millionths) and a creator-supplied share mint, each under its own token program; applies the asset-mint and share-mint policies and records `asset_controls` and `permissioned`; records the council (vault 0 of an autonomous Squads v4 multisig of at most ten members with a majority threshold, which does not sign) and the series' Metaplex Core collection (a bare one with the pool PDA as update authority: no plugins, nothing minted); sets the line cap (largest line as a share of the NAV, 1 to 10,000 bps), the outstanding-principal cap, the collection period (at least 30 days), the acceleration notice (at least 7 days), the provision (100%) and the FIX bounds; there is no grace period on late payment; emits `PoolInitialized` |
| `deposit(assets, min_shares_out)` | depositor | assets in, shares out at the current NAV; the first deposit meets the minimum and locks 1,000 dead shares |
| `redeem(shares, min_assets_out)` | holder | shares burned, assets out at the current NAV, up to the vault's liquidity |
| `request_line(line_id, terms)` | borrower, payer, sponsor if named | the borrower requests line `line_id` (the pool's next) on `terms`, in centavos of MXN; stored as `Requested` with the terms hash; a separate payer funds the account; moves no money; refused while paused |
| `approve_line(expediente_root, terms_hash, valid_until)` | council | approves a requested line, naming its expediente root and terms hash, within 15 days of the request and within the line cap (its principal in MXN against the NAV) at a FIX at most five days old (age unchecked when the bounds pin the FIX, `min == max`); the borrower is neither the council nor a member of it; opens the draw window until 00:00:00 Mexico City of the first due day (decision 59), or until the term on a bullet line |
| `reject_line(valid_until)` | council | rejects a requested line; the rent returns to the payer |
| `withdraw_request()` | borrower | withdraws a requested line, expired or not; the rent returns to the payer |
| `cancel_line(valid_until)` | council | clears an approved commitment nobody drew (`Cancelled`, terminal); refused once anything was drawn |
| `draw(amount_mxn, fix_rate, fix_date, asset_bump, pagare_bump, cert_fingerprint, signature_method, signed_pagare_hash, pagare_signature, signed_on, sealed_hash)` | borrower, payer, and a cotejo key when the pool names any | pays a tranche of `amount_mxn` centavos to the borrower as asset units at the FIX in force (which the borrower signs as `fix_rate`/`fix_date`), while the window is open (strictly before it closes), at most 64 tranches a line and within what the line, the line cap (the line's USDC drawn against the NAV now), the vault and the pool cap allow; refused while paused or on a changed or stale FIX; no interest is fixed: it accrues on the principal outstanding from the draw (decision 48); mints the pagaré (a frozen Metaplex Core asset, its hash derived on chain from the approved template and terms) and its `Pagare` record, whose rent the `payer` funds; `signature_method` is 0 wallet, 1 PSC + wallet, 2 e.firma + wallet, with `cert_fingerprint` for the last two; decision 55: the borrower subscribes the pagaré by signing off chain a message with its full text, and the draw carries that message's SHA-256, the ed25519 signature and the day (`yyyymmdd`, which must be today in Mexico City), stored unverified and bound into the issuance entry (`signedPagareHash` and `verifyPagareSignature` in the client check them against the text; `assertPagareMessageText` refuses a message that is not plain text, so the wallet key never signs transaction-shaped bytes); decision 56: when the pool names cotejo keys, the optional read-only `cotejo` account must be one of them and sign (`CotejoRequired`, checked before anything is written; with no key set it is ignored), and the record and `PagareIssued` name it (the zero key when none was required); decision 57: `sealed_hash`, the SHA-256 of the encrypted content of the sealed copy the parties keep, recipients excluded, zero when there is none, is stored and emitted as given, never rewritten, and is in neither the pagaré hash nor the issuance entry |
| `suspend_draws(valid_until)` | council | closes a line's draw window now (a KYC failure, a default event); the undrawn remainder is forfeited, repayment is untouched |
| `resume_draws(valid_until)` | council | reopens a suspended line's window to its original end; refused while an acceleration is pending |
| `condone(interest, principal, document_hash, valid_until)` | council | the quita (decisions 50 and 51), against the hash of the written record: condones all the late interest the line has accrued to now, `interest` of the ordinary interest accrued and unpaid and `principal` of the principal outstanding; nothing is collected and no funds move. Condoned principal counts as settled (it meets installments and stops accruing) and its cost, at each pagaré's own FIX, is recognised as a loss now: it leaves the outstanding principal (and the provision in collection) for `written_off`, as a write-off of it would; on a line already written off the pool does not move. A quita of all the principal takes all the interest unpaid when it lands, whatever `interest` names. A partial one may leave ordinary interest owed. With principal it takes the same pagaré records as `repay` (at most three; a larger quita is split) and writes a `Condonacion` entry on each one it reaches; one it extinguishes is marked paid with the `CONDONED` flag, naming no payer. `Active`, `InCollection` or `WrittenOff` lines. A line left owing nothing that can draw nothing more closes, and only then may its títulos leave (decision 49). Never paused |
| `renounce_availability()` | borrower | gives up what is left to draw of an `Active` line, for good (decision 45): the window closes now and `resume_draws` cannot reopen it, so the line closes at the borrower's will with the payment that leaves it owing nothing. Refused when there is nothing to renounce (`WindowClosed`: window already closed, by date or by a suspension, or line drawn in full). Moves no funds. Never paused |
| `repay(max_usdc, fix_rate, fix_date, receipt_hash)` | anyone | pays up to `max_usdc`, credited in MXN at the FIX in force: late interest (which the credit must cover), then the ordinary interest accrued per second to that moment and unpaid, then principal, never more than is due (decision 48); the USDC taken is rounded up. Ordinary interest accrues on the principal outstanding only, so a principal repayment lowers it from that second, a payoff at any second costs the principal and the interest accrued to it, and a payment split in several transactions in the same second costs what one costs. The line becomes `Repaid` only when it owes nothing and can draw nothing more; while it can draw, a payment of all that is owed leaves it `Active`, and a payment when nothing is payable is refused. Takes the pagaré records `[pagares_paid ..= last_touched]`, oldest first, at most `MAX_SIDECARS_PER_REPAY` (3) of them (`PagareAccountsMismatch` / `TooManySidecars` otherwise); writes `Pagado` on each pagaré the principal fully covers and `Abono` on the one left uncovered. A payment of accessories alone leaves its `Abono` on the oldest outstanding pagaré, stating zero principal (LGTOC 17). Interest is taken first, so a payment that applies principal leaves no interest accrued unpaid: the entries reserve none; a pagaré paid while the line still owes stays with the pool until the line owes nothing (decision 49). `receipt_hash` is the payer's receipt, zero meaning "to follow". Clients split a larger payment. On a written-off line it is a recovery (decision 51): the same order and records, but the principal's cost comes off `written_off` instead of the outstanding principal, which the write-off already left, and `LossRecovered` states it apart from the interest; the line closes when it owes nothing and its títulos may leave. Never paused |
| `mark_in_collection()` | anyone | moves a scheduled line to `InCollection` once `max_missed` installments are missed — or every remaining one, when fewer remain (decision 41) — past the due date of the last of them, accelerating the balance and provisioning the line's outstanding cost in the NAV (decision 40). Never paused |
| `propose_acceleration(valid_until)` | council | proposes to collect a drawn line early: draws are suspended at once, and `accelerate_line` may fire after the pool's notice |
| `withdraw_acceleration(valid_until)` | council | withdraws a pending acceleration; draws stay suspended until `resume_draws` |
| `accelerate_line(valid_until)` | council | fires a pending acceleration once its notice has run: the line moves to collection, as `mark_in_collection` (the provision included), without missed installments |
| `write_off(valid_until)` | council | books a line's unpaid principal as a loss: an `Active` line after the term, or a line in collection once the pool's collection period has run from the mark and either its oldest missed due date or the term has passed. The debt stays owed: the line still takes recoveries and quitas, and late interest keeps running (decision 51). Never paused |
| `endorse_pagare(draw_index, kind, to, endoso_hash, valid_until)` | council | endoso en procuración of a pagaré to `to` (`KIND_PROCURACION`, the only kind the pool makes): appends the entry and names the procurador of record, who collects through `repay` and litigates with the entry; no power over the asset is lent (decision 38) and the asset stays with the pool. Refused when `to` is the zero key, the pool, the council, a member of it, the creator, the FIX feeder, or the line's borrower, payer or sponsor (`EndorseeInvalid`), on a written-off line, on a pagaré not outstanding, or while a procuración is in force |
| `revoke_procuracion(draw_index, entry_hash, valid_until)` | council | revokes the procuración in force over the signed revocation's hash: appends the `Revocacion` entry; there is no power over the asset to take back (decision 38); refused when none is in force |
| `court_annotation(draw_index, kind, document_hash, valid_until)` | council | records a court's act on a pagaré as an entry of its registry and moves nothing else — the judicial stage of a collection runs off chain, after the portfolio is sold or written off (decision 38-g); refused once the título was delivered |
| `annotate_payment(draw_index, entry_count, receipt_hash, valid_until)` | council | attaches the real receipt to a payment entry of a pagaré's registry (a `Recibo` entry) when the payer's hash was zero or wrong; allowed whatever the pagaré's status; the correction is the entry — no field is rewritten (LGTOC 41) |
| `deliver_pagare(draw_index)` | anyone | delivers a paid título (decision 38, LGTOC 129-130): thaws the asset, renounces the `PermanentFreezeDelegate` to `None` (terminal: nobody can ever freeze it again) and transfers it to the line's borrower — or to the aval who settled it (LGTOC 115) — never to the sender; appends the `Entrega` entry over the record's `receipt_hash` and sets the `DELIVERED` flag. Refused before the payment (`PagareNotCovered`), twice (`PagareAlreadyDelivered`), and while the line owes any principal or interest, whatever its status (`BalanceOutstanding`, decision 49): the pagaré promises interest until maturity, so one whose principal is covered while the line still owes is not paid in full (LGTOC 17 and 130). An active line that can still draw and owes nothing delivers; a written-off line still owes, so its títulos stay until recoveries and quitas leave it owing nothing (decision 51). A título the quita extinguished goes to the borrower. The client sends the deliveries in the settling repayment's own transaction, as many as fit, and the rest right after; a missed one can be sent any time. Never paused |
| `burn_pagare(draw_index, document_hash)` | the delivered título's owner | optional: a `Terminacion` entry (a zero hash is allowed: the burn is the owner's unilateral act), then `BurnV1` with the owner as authority — Core itself refuses anyone else; the rent returns to the owner and the record stays |
| `annul_pagare(draw_index, document_hash, valid_until)` | council | annuls an outstanding título issued in error: a `Terminacion` entry over the annulment document's own hash; nothing burns, no state moves, the debt follows the line — a título issued in error is annotated, never destroyed (decision 38). Refused once paid |
| `pause(valid_until)` | council | pauses deposits, redemptions, requests, approvals and draws; repay and everything else stay open |
| `resume(valid_until)` | council | ends a pause |
| `propose_council(new_council, kind, valid_until)` / `accept_council(valid_until)` | council / the proposed council | hands the pool to a new council in two steps (`kind` 0, a Squads vault, the only kind taken; the zero key withdraws the proposal), each after the multisig check |
| `set_fx_feeder(fx_feeder, valid_until)` | council | names the key that publishes the FIX within the daily band (the zero key for none; then only the council publishes) |
| `set_cotejo_keys(keys, valid_until)` | council | replaces the three cotejo keys as a whole (decision 56; the zero key is an empty slot, all zero lifts the requirement); a repeated non-zero key or one equal to the pool, the council or the program is `CotejoKeyInvalid`; also while paused, so the council can rotate a key in an incident; emits `CotejoKeysSet` |
| `post_fix(fix_rate, fix_date, valid_until)` | the FIX feeder or the council | publishes Banxico's FIX (`fix_rate` is MXN per USD × 10⁴, `fix_date` is `yyyymmdd`), within the pool's bounds; the feeder moves within 5% a day (one to three days) and against RedStone's MXNe reference, once per Mexico City day; the council publishes the rest, dated at most five days back |
| `set_fix_bounds(min_fix_rate, max_fix_rate, valid_until)` | council | sets the lowest and highest FIX `post_fix` accepts (zero and zero for none); `min == max` pins the FIX, which then never goes stale for approvals and draws — the parity recipe of a peso series |
| `set_max_line_bps(max_line_bps, valid_until)` | council | sets the line cap, 1 to 10,000 bps of the NAV, measured at every approval and draw |
| `set_provision_bps(provision_bps, valid_until)` | council | sets the provision the NAV takes on the lines in collection, 0 to 10,000 bps of their outstanding cost; `init_pool` starts it at 10,000 (100%) |
| `set_outstanding_cap(max_outstanding_principal, valid_until)` | council | sets the pool-wide cap on the USDC lent out at once |
| `set_collection_period(collection_period_seconds, valid_until)` | council | sets the seconds a line in collection waits, from its mark, before write-off; never below 30 days |
| `set_acceleration_notice(acceleration_notice_seconds, valid_until)` | council | sets the notice between `propose_acceleration` and `accelerate_line`; at least seven days |

The 84 error codes (6000 to 6083) are numbered from 6000 in the order of `PoolError` in
`programs/elote_pool/src/errors.rs`, where they are grouped by domain.

## Installments and collection

`request_line`'s terms take an optional `ScheduleParams { first_due_at, interval_seconds, installments, max_missed }`
(the annual `late_bps` is a term of the line itself). Without a schedule the line is a bullet line
(single maturity) and nothing below applies.

- **Schedule.** The calendar comes from the signed contract: installment `k` falls
  due at `first_due_at + (k - 1) * interval_seconds`, whenever the tranches were drawn. The
  schedule is on principal (decision 48): each installment is `drawn / installments`, rounded down;
  the last one carries the remainder, so they add up exactly, and a later tranche before the first
  due date raises every installment. Ordinary interest has no calendar of its own: every payment
  takes the interest accrued first. Limits: 1 to 120 installments, a positive interval, `late_bps <= 10_000`,
  `3 <= max_missed <= installments` (the owner's policy is three months of arrears before
  collection), `first_due_at` after the opening, the last due date no later than `term_end`, and
  at least one centavo of principal per installment both for the whole commitment at the opening
  and for what is drawn after each tranche.
- **Draw window** (decision 59). Draws end at 00:00:00 Mexico City (UTC-6 all year) of the day of
  `first_due_at`: `approve_line` stores that instant as `availability_end` and `draw` is refused from
  it on (`AvailabilityEnded`); one second before, 23:59:59 of the day before, is accepted. The
  client's `mexicoCityMidnight(firstDueAt)` gives it. The request checks only `first_due_at > now`;
  the approval is refused (`TermEnded`) once that midnight has come, so a line whose first due date
  falls on the day of the approval in Mexico City is never approved (the borrower withdraws the
  request).
- **Missed installments** are computed, never stored: installments fallen due minus installments fully
  covered by the principal repaid; one centavo short is a miss. At its due second an installment
  already counts as due. Paying exactly an installment's principal on its due date leaves it short
  by the interest the payment took first.
  There is no grace period: an installment is late from the second after it falls due.
- **Late interest** accrues on each overdue part of **principal** (no interest on interest) from
  its due date (or from the last repayment, whichever is later), linear per second at `late_bps` a
  year (365 days), alongside the ordinary interest until the last due date (decision 48):
  `Σ part * min(seconds, cap) * late_bps / (10_000 * 31_536_000)`, rounded down, in
  `u128`, where `cap = ceil(10_000 * 31_536_000 / late_bps)` seconds. Two caps apply:
  - **per settlement:** within one repayment, each overdue part accrues at most its own amount. A
    repayment that collects late interest settles it in full and records `late_paid_until`, so no
    period is charged twice; a repayment that collects none (the accrual rounded down to zero) leaves
    the watermark alone, so the accrual keeps running. A part that stays unpaid accrues again after
    the next settlement, so over several repayments the late interest on one part can exceed that part.
  - **over the life of the line:** late interest collected never exceeds the line's principal
    (`late_due <= principal - late_collected`). A borrower who pays only late interest reaches this cap
    and from then on every payment goes to interest and principal.

  Late interest goes to the vault and raises the NAV like interest; it is counted in the line's
  `late_collected`, not in the pool's `interest_collected`.
- **Repayment order** on every line: late interest, then the ordinary interest accrued and unpaid,
  then principal.
- **Collection.** Once `max_missed` installments are missed (at least three, three months of arrears)
  and the due date of the last of them has passed (`now > first_due_at + (covered + max_missed - 1) * interval`,
  with `covered` installments paid), anyone may call `mark_in_collection`, which moves the line
  to `InCollection` and records the acceleration (`accelerated_at`): from the mark every installment is
  due and late interest runs on all remaining principal (vencimiento anticipado), while ordinary
  interest stops at the mark (decision 48). Repayments are still accepted there;
  there is no cure, and only a full repayment moves the line to `Repaid`.
- **Write-off policy.** After the third month in arrears the line goes to out-of-court collection and the
  council records the write-off. The program allows `write_off` only when the pool's collection period has run from the mark and the line is in collection and its
  oldest missed installment is past due, or when the term has ended. Only the council may call it.

## Events

Every instruction emits its events through `emit_cpi!` (`programs/elote_pool/src/events.rs`), in
the order listed for each instruction.

| Event | Emitted by | Fields |
|---|---|---|
| `PoolInitialized` | `init_pool`, its only event | pool, council, creator, multisig, asset_mint, share_mint, collection, collection_period_seconds, max_line_bps, max_outstanding_principal, acceleration_notice_seconds, asset_controls (bits: 0 freeze authority, 1 `Pausable`), permissioned, min_fix_rate, max_fix_rate |
| `Deposited` | `deposit`, then `NavUpdated` | pool, depositor, assets, shares, nav_after, supply_after |
| `Redeemed` | `redeem`, then `NavUpdated` | pool, holder, shares, assets, nav_after, supply_after |
| `LineRequested` | `request_line` | pool, line_id, borrower, payer, sponsor, sponsor_role (0 promoter, 1 guarantor), agreement_hash, expediente_root, pagare_template_hash, terms_hash, currency, principal, rate_bps, term_end, first_due_at, interval_seconds, installments, late_bps, max_missed (the schedule fields zero on a bullet line) |
| `LineApproved` | `approve_line` | pool, line_id, expediente_root, terms_hash, availability_end |
| `LineRejected` | `reject_line` | pool, line_id |
| `RequestWithdrawn` | `withdraw_request` | pool, line_id |
| `LineCancelled` | `cancel_line` | pool, line_id |
| `LineDrawn` | `draw`, first | pool, line_id, draw_index, amount_mxn, fix_rate, fix_date, usdc_delivered, drawn, available, availability_end, outstanding_after (the pool's USDC cost) |
| `PagareIssued` | `draw`, after `LineDrawn` | pool, line_id, draw_index, asset (the Core asset), pagare_hash, amount_mxn, delivery_hash, due_at, holder (the pool PDA), signed_pagare_hash and signed_on (decision 55), cotejo (decision 56), sealed_hash (decision 57) |
| `PagareEntry` | every instruction that appends to a pagaré's registry: `draw` (the issuance entry, after `PagareIssued`), `repay`, `condone`, `endorse_pagare`, `revoke_procuracion`, `court_annotation`, `annotate_payment`, `deliver_pagare`, `burn_pagare`, `annul_pagare` | pool, line_id, draw_index, asset, kind, from, to, entry_hash, entry_count, recorded_at, stated_at |
| `FxRealized` | `repay` that applies principal, before `LineRepaid` | pool, line_id, receipt_hash, cost, received, delta (the deltas sum to `Pool.fx_realized`), nav_after |
| `PagarePayment` | `repay` that took pagaré records, after its `PagareEntry` events | pool, line_id, payer, receipt_hash, late_part, interest_part, principal_part, entries (draw_index, kind, entry_count, principal_applied, accessory_applied, principal_balance, by_borrower — the preimages of each entry's annotation hash) |
| `LateInterestCollected` | `repay` with late interest, before `LineRepaid` | pool, line_id, amount |
| `LossRecovered` | `repay` on a written-off line, before `LineRepaid` (decision 51) | pool, line_id, receipt_hash, late_part, interest_part, principal_part, cost_recovered (off `written_off`), received (the USDC that paid that principal), written_off_after, nav_after |
| `LineRepaid` | `repay`, then `NavUpdated` | pool, line_id, payer, receipt_hash, usdc_received, fix_rate, principal_part, interest_part, remaining (principal and interest accrued and unpaid, without late interest), nav_after |
| `LineInCollection` | `mark_in_collection` | pool, line_id, missed, remaining (principal and the interest accrued to the mark and unpaid), impaired_principal (the pool's whole impairment after the mark) |
| `AccelerationProposed` | `propose_acceleration` | pool, line_id, due_at |
| `AccelerationWithdrawn` | `withdraw_acceleration` | pool, line_id |
| `LineAccelerated` | `accelerate_line` | pool, line_id, remaining, impaired_principal |
| `LineDrawsSuspended` | `suspend_draws` | pool, line_id, available_forfeited |
| `LineDrawsResumed` | `resume_draws` | pool, line_id, availability_end |
| `LineCondoned` | `condone`, after its `PagareEntry` events, then `NavUpdated` when it recognised a loss | pool, line_id, document_hash, late_interest, interest, principal, cost, loss_recognized (zero on a written-off line), remaining, nav_after, condoned_at, entries (draw_index, entry_count, principal_condoned, principal_balance) |
| `AvailabilityRenounced` | `renounce_availability` | pool, line_id, borrower, available_forfeited, renounced_at |
| `LineWrittenOff` | `write_off`, then `NavUpdated` | pool, line_id, loss, loss_usdc, nav_after |
| `NavUpdated` | `deposit`, `redeem`, `repay`, `write_off`, `condone` recognising a loss, last in each | pool, total_assets, share_supply |
| `PagareDelivered` | `deliver_pagare`, after its `PagareEntry` | pool, line_id, draw_index, asset, recipient (the borrower, or the aval who settled it), payer, receipt_hash, entry_count, delivered_at |
| `PagareBurned` | `burn_pagare`, after its `PagareEntry` | pool, line_id, draw_index, asset, owner, document_hash |
| `PagareAnnulled` | `annul_pagare`, after its `PagareEntry` | pool, line_id, draw_index, asset, document_hash |
| `PagareReceiptAnnotated` | `annotate_payment`, after its `PagareEntry` | pool, line_id, draw_index, entry_count (the `Recibo` entry's), payment_entry_count (the entry it annotates), receipt_hash |
| `FixPosted` | `post_fix` | pool, fix_rate, fix_date, previous_rate, by, slot, posted_at, reference_rate, reference_at (the RedStone reference, zero when missing) |
| `FxFeederSet` | `set_fx_feeder` | pool, fx_feeder |
| `CotejoKeysSet` | `set_cotejo_keys` | pool, keys (the three slots, the zero key for an empty one) |
| `FixBoundsSet` | `set_fix_bounds` | pool, min_fix_rate, max_fix_rate |
| `CouncilProposed` | `propose_council` | pool, council, kind |
| `CouncilAccepted` | `accept_council` | pool, old, new, kind |
| `MaxLineBpsSet` | `set_max_line_bps` | pool, max_line_bps |
| `ProvisionSet` | `set_provision_bps` | pool, provision_bps |
| `OutstandingCapSet` | `set_outstanding_cap` | pool, max_outstanding_principal |
| `CollectionPeriodSet` | `set_collection_period` | pool, collection_period_seconds |
| `AccelerationNoticeSet` | `set_acceleration_notice` | pool, acceleration_notice_seconds |
| `PoolPaused` | `pause` | pool, by (the council) |
| `PoolResumed` | `resume` | pool |

## Build and test

Toolchain: Rust 1.89.0 (`rust-toolchain.toml`), Anchor CLI 1.2.1 (from [otter-sec/anchor](https://github.com/otter-sec/anchor), where Anchor now lives), Solana CLI 4.1.2 (the pairing Anchor 1.2 is tested against), Node 22.12+. `avm install 1.2.1 && avm use 1.2.1` and `agave-install init 4.1.2` set both; CI installs the same versions and checks the Anchor binary's sha256. The build pins platform-tools v1.54
(`--tools-version v1.54`): cargo-build-sbf 4.1.0's default does not build the program.

```sh
NO_DNA=1 anchor build --ignore-keys --arch v0 --tools-version v1.54 -- --features localnet   # target/deploy/elote_pool.so and target/idl/elote_pool.json
cargo test -p elote_pool         # unit tests + LiteSVM integration tests against the built .so
npm install
npm run codama                   # copies the IDL to idl/ and regenerates clients/js/src/generated
npm run check:client             # fails if the committed client differs from idl/elote_pool.json
npm run typecheck
npm run test:client              # replays the program's arrears vectors against clients/js/src/schedule.ts
```

The `localnet` feature lets `init_pool` accept any asset mint, which the tests and the local e2e
need. Without a feature the program accepts only mainnet USDC and MEXe; `--features devnet`
accepts the Etherfuse sandbox MEXe and USDC and Circle's devnet USDC (`state::ASSET_ALLOWLIST`). A
deployed build is verified byte for byte against a build with its cluster's feature.

`anchor build` on a fresh clone writes a new random `target/deploy/elote_pool-keypair.json`. It is
not the program's key: never run `anchor keys sync` or `anchor deploy` with it, or the program id
in `declare_id!` and `Anchor.toml` would change. Anchor 1.2 refuses to build on that mismatch, hence
`--ignore-keys`. Tests and the local validator use the explicit id.

`--arch v0` keeps the SBPF v0 format of the deployed program: Anchor 1.2 defaults to SBPF v3, which
the Solana 3.x runtime inside LiteSVM 0.11 cannot load. Moving to v3 goes together with
`anchor-lang`'s `solana-v4` feature and a LiteSVM on the Agave 4.x line.

The integration tests run the compiled program in LiteSVM 0.11.0, which loads SPL Token, Token-2022
and the Associated Token Account program by default; the harness adds Metaplex Core from
`programs/elote_pool/tests/fixtures/` (pinned by sha256, see the README there).
There are 356 Rust tests (unit tests in `src/` and integration tests in 22 binaries; the shared
reference's own tests run in each binary that includes it) and 43 JS tests. `programs/elote_pool/tests/` holds:

| File | Tests | What it checks |
|---|---|---|
| `pool.rs`, `lines.rs`, `requests.rs`, `schedule.rs` | 22, 11, 7, 53 | Behaviour of every instruction, happy paths and each error code; the request/approve/reject/withdraw flow; installments, arrears and collection; the draw window closing at 00:00:00 Mexico City of the first due day (decision 59); ordinary interest on the outstanding principal (decision 48) against an independent reference (`tests/reference/`), dust and split invariance, and a proptest state machine over random draws, payments and marks |
| `council.rs`, `valid_until.rs`, `squads.rs` | 17, 4, 9 | The Squads multisig check on every council instruction, council rotation; `valid_until` refused past or too far ahead; the hand-built Squads v4 instructions, the council setting the cotejo keys, and a second vault of the council co-signing a draw as cotejo |
| `cotejo.rs` | 13 | Decisions 56 and 57: the cotejo rule against an independent reference (`tests/reference/`), a property test over random key sets, refusals that write nothing, the zero key never matching, rotation, only the council setting keys, also while paused, the sealed hash stored as given, and a byte-level run through every instruction but `init_pool` and `accept_council` showing only `set_cotejo_keys` moves the keys and nothing rewrites an issued record |
| `fx.rs` | 27 | The FIX: the MXN↔USDC conversion, the daily band, the RedStone MXNe reference, the bounds, feeder versus council, one publication a day |
| `acceleration.rs` | 12 | `propose_acceleration` / `withdraw_acceleration` / `accelerate_line`, the notice, and the interplay with `suspend_draws` and `resume_draws` |
| `quita.rs` | 13 | Decision 51: the council's quita of principal (total, partial then paid, split, on a written-off line) and recoveries on a written-off line, every pool field worked by hand, each pagaré at its own FIX, the `Condonacion` entry replayed |
| `known_limits.rs` | 12 | Documentation tests that pin the known design limits listed in SECURITY.md; they pass by design and must be inverted when a limit is removed |
| `tranches.rs` | 17 | Tranched draws: the draw window, the payability floor, interest from each draw, the cap on tranches, `suspend_draws`, `cancel_line` |
| `pagare.rs` | 36 | The pagaré token: one frozen Core asset and one `Pagare` record per draw, every plugin read back through the `mpl-core` crate against the pinned Core binary, wrong accounts refused; the registry entries; the delivery to the borrower or the aval once the line owes nothing, the owner's burn, the council's annulment, coverage oldest first, the procuración, the refusals |
| `core_capabilities.rs` | 8 | What Metaplex Core allows after an asset's creation: handing off or extinguishing the permanent delegates, thawing, moving the update authority off the collection, adding plugins and adapters, who writes and removes an AppData adapter and whether it follows the asset (the program uses none), the cost of `AddBlocker`, owner-managed delegates for the procuración and a burn delegate held by a program address; pinned against the Core binary so an upgrade that changes an answer fails here |
| `layout.rs` | 5 | The account sizes (`Pool` 591, `CreditLine` 528, `Pagare` 527 bytes), the offsets `repay` and `condone` read and write a record at in place, against its Borsh encoding, the zeroed `reserved` tail of each account, and one `CreditLine` layout whether or not the line has a sponsor (the zero key for none) |
| `audit.rs`, `reaudit.rs` | 4, 4 | Regression tests of the findings of the 2026-11-01 audit and the 2026-11-02 re-audit |
| `compute.rs` | 12 | Compute units of `init_pool`, `draw` (with the Core CPI), `repay` (one covering three pagaré records and a recovery on a written-off line included), `condone` (over three records), `deliver_pagare` and `burn_pagare`, `endorse_pagare`, `revoke_procuracion`, `court_annotation`, `mark_in_collection` and `write_off`, at 18 and 120 installments, stay under 100,000 |
| `math_props.rs` | 12 | Property tests (proptest) over the share accounting, the arrears arithmetic, the payment order and the accrual against the reference |
| `vectors.rs` | 1 | Evaluates the arrears arithmetic on 427 cases (48 accelerated) and compares with `clients/js/test/arrears-vectors.json`, which `npm run test:client` replays against the TypeScript mirror. After a deliberate change to `math.rs`, regenerate with `UPDATE_VECTORS=1 cargo test --test vectors` in the same commit |

## Deployments and upgrade authority

| Cluster | Program id | Upgrade authority | Deployed from | Verified |
|---|---|---|---|---|
| devnet | `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM` | `GU7CfoGZYsfkx23aJoqFjfwyAmLCExqBBoBFrn2EvZKS` (single key) | `main` @ `cc0d95d` (decisions 55, 56, 57 and 59), 2026-11-12, slot 521495526 | executable hash `213763ea9f1051cd88fa72eaaf57a1a8cb2e63bf4f0c6b784aca2e41b84f68d7`: the deployed binary is the `solana-verify build` (Docker) of that commit with `--features devnet --arch v0 --tools-version v1.54`, and `solana-verify verify-from-repo` of the public repository at `cc0d95d` printed "Program hash matches" (verification data not uploaded on chain) |
| mainnet-beta | not deployed | | | |

The devnet council is vault 0 (`6s6aBtkG6Wm9N6Gx1igKnc3Q3fcJt2r1WtyyLSQrnS6x`) of the Squads v4
multisig `C3sa3p6WRkEULZbVmYGDB3i6YaoDAWJXM9euUPJg8Vap` (2 of 3, created with
`scripts/devnet-multisig.ts`). Both devnet pools are on Circle's devnet USDC
(`4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`), with `init_pool` as in `scripts/demo-pool.ts`
(5,000 bps line cap, no FIX bounds), and the council published Banxico's FIX of 2026-11-11
(18.6192) on each:

| Pool | Address | Creator | Share token | Pagaré collection | Cotejo keys |
|---|---|---|---|---|---|
| main (the app's) | `A1bNQufs6va2JcsBWA6WnxaXifatryacUx5xTZ59cNXE` | `.demo-keys/admin-3.json` | `79eMcMCNGBJF7yCcXg8dJgFQX3F2tMn2s5yyPZWWDB9k` | `2HekrwdTzexiG85K2gmJuSXFK2G8KTPYBRtFbqeyUct8` | none |
| cotejo smoke | `FExNrsXwHhjSjZgPc1hRNv8GYNiE9gEitvPQtmKPsuks` | `.demo-keys/admin-4.json` | `BKz5mExocBDnuAWjPskKLFiN76YJWU9e7THXDDF33aF4` | `9PF96Tyu7uKeHSWJTKV6M1DoAxYNWyh3QVBZyvFP59VZ` | `599NAPErV6kVi1hcPyGcZtBZxwpDYwLhqUjE7BFTJ2oL` |

Smoke test of 2026-11-12 on the main pool: a deposit of 1.8 tUSDC, a MXN 10 line in three weekly
installments requested, approved through Squads (draws until 2026-11-19 06:00:00Z, decision 59), MXN
5 drawn against a plain-text pagaré message with fictitious data signed by the borrower (the record's
`signed_pagare_hash`, `pagare_signature` and `signed_on` read back, and the client's
`verifyPagareSignature` true for them and the text), and one repayment. On the cotejo smoke pool the
council named one cotejo key: a draw without its co-signature was refused (`CotejoRequired`, 6082)
and the draw it co-signed went through. The earlier pools, `BfhH5R2mtQaJDeTY15CQth2B75xcYWzhwypvBYYdUTWb`
and `7ezjge…`, cannot be read by this build: the account layouts changed. The devnet authority is a single hot wallet, acceptable for a test deployment only. Before any
mainnet deployment the authority moves to a [Squads](https://squads.so) multisig, the IDL is
published on chain (`anchor idl init`), and the build is verified. The program embeds a
`security.txt` (see `SECURITY.md`).

Reproduce and compare the on-chain build. A deployed build is compared with a build of this
source **with its cluster's feature** (no feature for mainnet, `devnet` for devnet — the asset
allow-lists differ, so the bytes do; SECURITY.md), the SBPF arch `v0` and platform-tools `v1.54`,
built in [solana-verify](https://github.com/Ellipsis-Labs/solana-verifiable-build)'s Docker image
(solana-verify 0.5.2 or later, Docker running). `[workspace.metadata.cli]` in `Cargo.toml` pins
Solana 4.1.2, which picks the image. For the devnet program:

```sh
solana-verify build --library-name elote_pool --arch v0 --cargo-build-sbf-args="--tools-version v1.54" -- --features devnet
solana-verify get-executable-hash target/deploy/elote_pool.so
solana-verify get-program-hash -u devnet BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM
```

The deployed program must be the output of that Docker build: a native `anchor build` of the same
commit gives a different hash. Anyone can check a deployment against the public repository:

```sh
solana-verify verify-from-repo -u devnet --program-id BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM \
  https://github.com/soft-law/elote-contracts --commit-hash <COMMIT> --library-name elote_pool \
  --arch v0 --cargo-build-sbf-args="--tools-version v1.54" -- --features devnet
```

### Releasing the client

Bump `version` in `clients/js/package.json`, commit on `main`, tag `client-v<version>` and push the
tag. `release-client.yml` checks the tag is on `main`, regenerates the client against the IDL,
builds and publishes through npm trusted publishing (OIDC): no token in the repo, provenance
attached by npm. One-time setup by the package owner: publish the first version by hand from a
logged-in machine, then on npmjs.com add the trusted publisher (GitHub, `soft-law`,
`elote-contracts`, `release-client.yml`, environment `npm-publish`) and add required reviewers
to the `npm-publish` environment in this repository's settings.

## Demo

```sh
npx tsx scripts/demo-pool.ts --cluster local  --multisig <address> [--agreement-hash <64 hex>] [--sponsor] [--schedule]
npx tsx scripts/demo-pool.ts --cluster devnet --multisig <address> [--agreement-hash <64 hex>] [--sponsor]
```

Units: lines and pagarés are in MXN (centavos on chain; the scripts take pesos with up to 2
decimals), the vault holds the pool asset, here **tUSDC (devnet test token, no value)**, 6 decimals;
deposits, redemptions and repayments are in tUSDC. Draws deliver tUSDC at the FIX in force and
repayments are credited in MXN at it. The test asset has no value: nothing here is ever tested with
an asset of real value.

The council is vault 0 of a Squads v4 multisig (`--multisig`); its members' keys come from
`--council-keypairs <a.json,b.json>`, `ELOTE_COUNCIL_KEYPAIRS` or `.demo-keys/curator-<n>.json`, and every
council step runs as a Squads vault transaction (created, proposed, approved by the threshold,
executed with a 100,000 CU limit) through `scripts/lib/council.ts`, whose Squads instructions are
hand-built in `scripts/lib/squads.ts` (Squads v4 af94153, the pinned binary's sha256 in its header).
A Squads error and an elote_pool error can share a number (both count from 6000): the scripts name
the program that raised it from the logs.

The demo creates the test token, the share token and the pagarés' Metaplex Core collection (bare,
with the pool PDA as update authority, as `init_pool` requires) and a pool on the given multisig. The
council publishes a FIX (18.3740); two investors deposit (10,000 and 5,000 tUSDC); a MXN 60,000 line
at 12% is requested and approved by the council; the borrower draws it (pagaré 0, delivered as tUSDC
at the FIX) and repays what it holds; the sponsor pays the rest; then a partial redemption. It prints
every signature with an explorer link and a final table of NAV, supply and value per share.

With `--schedule` it then requests a MXN 6,000 line at 12% in six installments with 100% a year late
interest and collection after three missed installments, has the council approve it, draws it, lets
the first installment go unpaid past its due date, and repays it with the next
one: late interest first, then the interest accrued, then principal. A validator's clock cannot be moved forward,
so on `--cluster local` an installment falls due every 20 seconds (`--interval-seconds`).

Wallets are generated into `.demo-keys/` (ignored by git) unless `ELOTE_ADMIN_KEYPAIR`,
`ELOTE_INVESTOR_A_KEYPAIR`, `ELOTE_INVESTOR_B_KEYPAIR`, `ELOTE_BORROWER_KEYPAIR` or
`ELOTE_SPONSOR_KEYPAIR` point to keypair files. Every script prints the cluster and, unless the RPC is
this machine or `--yes` is given, asks before sending. `ELOTE_RPC` overrides the RPC URL.

`scripts/local-e2e.sh` runs every script below end to end on a validator of its own: it preloads the
program, the pinned Metaplex Core and Squads v4 binaries (checking their sha256) and a 2-of-3 Squads
multisig of three keys it makes (`scripts/squads-local-multisig.ts` writes the account file, so no
Squads ProgramConfig is needed), with the ledger and all keys in a temporary directory removed on
exit. It refuses any RPC that is not this machine and refuses to start when something already
answers on its port (`ELOTE_E2E_PORT`, default 18899). Build first
(`NO_DNA=1 anchor build --ignore-keys --arch v0 --tools-version v1.54 -- --features localnet`).

`scripts/request-line.ts --pool <address> --borrower-keypair <path> --principal <MXN>` requests one
line, with `--rate-bps` (1200), `--term-days` (90), `--late-rate-bps` (10000), `--sponsor-keypair` with
`--sponsor-role promoter|guarantor`, and `--agreement-hash` (or `--agreement-file` with
`--agreement-salt`: the hash is then `sha256(salt || file)`, the convention for agreement and receipt
hashes), `--expediente-root` and `--pagare-template-hash`. `--installments <n> --interval-days <n>`
request a scheduled line, with `--max-missed` (3, the program's minimum). The borrower and a payer
(the demo admin, standing in for Elote's backend) sign; the script prints the terms hash the council
approves.

`scripts/approve-line.ts --pool <address> --line-id <n>` has the council approve a requested line: it
recomputes the terms hash from the stored terms with the client's `termsHash`, refuses on a mismatch,
and runs `approve_line` through Squads with `valid_until` a day ahead.

`scripts/post-fix.ts --pool <address> --fix <18.3740> [--fix-date <yyyymmdd>]` publishes Banxico's FIX
(the date defaults to today in Mexico City by the chain clock): through the council, or by the pool's
feeder with `--feeder-keypair`.

`scripts/council-propose.ts --pool <address> --op <op>` runs one council decision on the pool's
settings through Squads: `set-fx-feeder --address`, `set-fix-bounds`, `set-max-line-bps
--bps`, `set-outstanding-cap --usdc`, `set-cotejo-keys --keys <a,b,c|none>` (decision 56), `reject-line
--line-id`, `resume`.

`scripts/draw.ts --pool <address> --line-id <n> --borrower-keypair <path> --amount <MXN>` draws one
tranche and mints its pagaré, with `--signature-method wallet|psc|efirma`; psc and efirma need
`--cert-fingerprint` and `--cert-salt`, and the draw records `sha256(salt || fingerprint)`, never the
fingerprint (the SAT publishes e.firma certificates by RFC). The borrower's key signs the pagaré
message (decision 55): the text in `--pagare-text <path>`, or without it a placeholder labelled DEMO,
which is no pagaré text; the draw records its SHA-256, the signature and today's date in Mexico City. The script prints the FIX and the tUSDC it
delivers before asking; the borrower signs (a FIX published in between fails the draw), and when the
pool names cotejo keys `--cotejo-keypair <path>` co-signs with one of them (decision 56; the script
refuses before sending without it). `--sealed-hash <64 hex>` records the sealed copy's hash
(decision 57; zero without it).

`scripts/repay.ts --pool <address> --line-id <n> --amount <tUSDC> | --all` pays on a line (the
borrower by default, `--payer-keypair` for anyone else; `--receipt-hash` or `--receipt-file` with
`--receipt-salt`, zero meaning "to follow"), credited in MXN at the FIX in force. A payment that would
touch more than three pagaré records is split into several `repay` calls (`MAX_SIDECARS_PER_REPAY`; the
program refuses more with `TooManySidecars`).

`scripts/endorse-pagare.ts --pool <address> --line-id <n> --draw-index <n>` endorses an outstanding
pagaré en procuración through the council. By default it makes a procurador key for the case,
`.demo-keys/procuradores/<pool>-<line>-<draw>.json`, to hand to the collection agency: one key per
case, never reused, or anyone could list the lines in collection from it. `--procurador <address>`
names an existing key and warns when it already holds a pagaré under procuración
(`pagaresUnderProcuracion` in the client). `--revoke` revokes the procuración in force. The procurador
can never be the pool, the council, a curator or the borrower.

`scripts/deliver-pagare.ts --pool <address> --line-id <n> --draw-index <n>` delivers a paid pagaré
to the line's borrower — or to the aval who settled it — once the line owes nothing (decision 49),
re-sending a delivery that did not land; anyone may send it. With `--burn --owner-keypair <path>`
the delivered título's owner burns the asset and takes the rent back; the record always stays.
(`repay.ts` delivers every título with the payment that settles the line: as many as fit beside
the `repay` (five or six beside a `repay` of three records, measured by size), and the rest right
after in transactions of up to nine deliveries.)

`scripts/key-to-base58.ts <keypair.json>` prints the base58 form of a keypair file's secret bytes,
the format a wallet's private key import accepts. The output is a secret: use it for devnet demo keys only.

`scripts/token-metadata.ts` gives an existing test token a name, symbol and icon in wallets by creating
its Metaplex Token Metadata account (`createMetadataAccountV3`, mutable, no creators, no collection):
`npx tsx scripts/token-metadata.ts --cluster devnet|local --mint <address> --authority-keypair <path>`,
with `--update-authority` (the demo admin of `.demo-keys/admin.json` if present, else the signer),
`--uri` (https://elote.baby/token/tmxn.json, served by the `elote-web` deploy), `--name` ("tMXN test token")
and `--symbol` (tMXN). The token's mint authority signs and pays; `ELOTE_TOKEN_AUTHORITY_KEYPAIR` can
hold the keypair path instead of the flag. Only addresses, the signature and explorer links are printed.

The script refuses if the metadata account already exists (change it with `updateMetadataAccountV2`,
signed by the update authority), if the key is not the token's mint authority, or if the URI does not
answer 200 with JSON whose name and symbol match (`--skip-uri-check` before the web deploy). `--dry-run`
prints the instruction's accounts and sends nothing; without a keypair it uses a placeholder signer. On a
local validator, load the Token Metadata program first with
`--clone-upgradeable-program metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s --url https://api.devnet.solana.com`.
