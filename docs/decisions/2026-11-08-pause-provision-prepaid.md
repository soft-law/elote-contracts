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
