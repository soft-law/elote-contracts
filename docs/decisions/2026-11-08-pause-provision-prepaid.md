# Decisions 39–41 (2026-11-08)

Taken by Mario in conversation on 2026-11-08, closing audit findings M-1, M-4 and L-3 (P4 punto 4)
before the devnet deployment. Decision 38 (the paid pagaré is delivered to its debtor) was decided
2026-11-05 and implemented today; its record is the 2026-11-05 TODO and the pagaré-Core analysis.

## 39. The pause is the council's; the guardian is retired (closes M-1)

Every administrative decision goes through the council's Squads vote — the pause included. `pause`
becomes a council instruction (multisig check + `valid_until`), like `resume`; no member, and no
other key, pauses alone, so no single key can freeze redemptions indefinitely ("pause with no
exit"). The cost accepted: an emergency stop now needs the council's majority. The guardian role
existed only to pause alone and is removed (`Pool.guardian`, `set_guardian`, `GuardianSet`).

## 40. The mark provisions the NAV in full (closes M-4)

The moment a line enters collection — the objective mark or the council's acceleration — the NAV
loses `provision_bps` of that line's outstanding USDC cost, so nobody redeems at a NAV that still
counts a defaulted credit as good. A recovery reverses its share; the write-off books what remains.
`init_pool` starts the rate at 10,000 bps (100%) and only the council moves it
(`set_provision_bps`). The fields `impaired_principal` and `provision_bps`, reserved since PR 2,
are now wired.

## 41. A line paid ahead reaches collection after its last due date (closes L-3)

The collection requirement clamps to the installments that remain: `max_missed` missed
installments, or every remaining one when fewer than `max_missed` remain
(`missed >= min(max_missed, installments − covered)`). A borrower who prepaid early installments
and stopped is marked one second after the last due date; before, the line could only leave by
write-off after the term or by the council's acceleration. Option A of the 2026-11-05 report.

## 38-g. The judicial stage runs off chain (amendment, same day)

The embargo arises when the collection is enforced in court — and by then the caso is an off-chain
lawsuit; that stage is not recorded on the pagaré and the pool does not mention it. The judicial
hold introduced earlier today (a counter gating `deliver_pagare`) is removed: `court_annotation`
annotates and gates nothing. The pool's exits from delinquency are selling the portfolio (the
endoso en propiedad, kind 1, reserved for a future feature) or the write-off, at most at 3–6–12
months of arrears (the collection period and the write-off are the council's, with the 30-day
floor).

## 40b. Deposits price at the unprovisioned NAV (Maple's dual price)

The deposit gate tried first (refuse while impaired) was wrong — there will always be some
portfolio in collection. Instead, decision 40b: deposits are priced at `Pool::entry_nav` (the
vault plus the whole outstanding cost, the impaired book at par) and redemptions at the
provisioned NAV — Maple v2's dual exchange rate, adopted for Maple's own stated reason. Whoever
enters while a line is impaired buys that book at par, so a recovery hands the entrant nothing:
the sandwich — flash or patient, with any capital — nets zero or less, and deposits are never
gated. What an entrant does share pro rata is ordinary income (interest, late interest), like any
holder. The front end must show both prices. Residuals, documented: redeeming before a
foreseeable mark (inherent to objective marking) and the cash-basis interest jumps (minor at
pilot size). `PoolImpaired` retired.

## 38-f, clarified (same day)

The aval who settles a título receives it alive, but the pool asserts nothing about what the aval
may then collect from the suscriptor: at most what it paid, and a judge decides (LGTOC 115). The
program's whole role is to record the payments — every entry already commits the amount, the
balance and who paid (`registry::annotation_hash`), which is exactly the evidence that question
turns on. This closes the counsel question on the aval's claim, including the reviewer's variant
(an aval whose payment only completed a pagaré the suscriptor had nearly paid): the entries state
the split, the judge rules on it.

## 42 — A pinned FIX is exempt from staleness (same day)

A pool whose bounds pin the FIX (`min_fix_rate == max_fix_rate != 0`) skips the
`MAX_FIX_AGE_DAYS` check in `approve_line` and `draw` (`Pool::fix_pinned`). The peso series on
MEXe runs at parity by the issuer's promise (1 MEXe = 1 MXN): the pin leaves exactly one postable
rate, so an old FIX cannot price anything differently from a fresh one, and a council re-post
cadence — the only mainnet path, since the RedStone MXNe reference rejects parity on the feeder —
buys nothing. Mario: a council cadence is not an option.

What the staleness check really measured on a pinned pool was council liveness, and the decision
knowingly trades that: if the council goes quiet, lending continues at par instead of stopping.
The depeg response is governance, as at Maker and Compound in March 2023 (see
`docs/research/2026-11-08-fix-fijado-par.md`, the opus research: verdict sound-with-conditions):
pause → `set_fix_bounds` wider → council `post_fix` → resume. The sequence is tested
(`unpinning_restores_staleness_and_the_council_re_prices_under_pause`): unpinning restores the
5-day check at once, so approvals and draws stay blocked until the council re-prices. Repay takes
any FIX age, so a re-price reaches borrowers on their next payment. Conditions carried forward:
the bounds stay council-adjustable (they do), an off-chain monitor watches MEXe *redemption
working* rather than a price that is 1.0 by construction, the series is capped against confirmed
redemption capacity, and investors are told their position is in MEXe, not pesos. No layout, IDL
or client change.
