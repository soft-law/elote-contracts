# Multi-draw and revolving credit on Ethereum: Huma v2, Goldfinch, Maple v2

Read-only review. Nothing was built or executed from the cloned repos. All paths are relative to each repo root.

| Repo | Clone dir | Commit read |
|---|---|---|
| 00labs/huma-contracts-v2 | `repos-credit/huma` | `21aaad53023eb3f448ff09820fee1a3936b6175d` (2024-12-31) |
| goldfinch-eng/mono (sparse: `packages/protocol/contracts/{protocol/core,interfaces,library}`) | `repos-credit/goldfinch` | `bb251675d8a28d046f4d4763e1cf8874ee7c2723` (2025-11-30) |
| maple-labs/fixed-term-loan | `repos-credit/maple-ftl` | `8fb3675288897cb0b5748a5bbe90a12055bf9fa1` (2025-05-25) |
| maple-labs/open-term-loan | `repos-credit/maple-otl` | `453cfb0e48b2db22727d901b5340e56f0e41ea26` (2025-05-25) |

Goldfinch: the CreditLine read is the current `ISchedule`-based one on `main` (`packages/protocol/contracts/protocol/core/CreditLine.sol`, Solidity 0.6.12, with `PaymentSchedule`), not the legacy v1 `Accountant.assess` line. `TranchedPool.assess()` is now an empty no-op (`TranchedPool.sol:107-109`). Below, `GF/` stands for `packages/protocol/contracts/`.

---

## 1. Huma Finance v2

### (a) Commitment/limit versus drawn balance
- `CreditConfig` (static, set at approval) holds `uint96 creditLimit` and `uint96 committedAmount`, plus `periodDuration`, `uint16 numOfPeriods`, `uint16 yieldInBps`, `bool revolving` (`contracts/credit/CreditStructs.sol:24-33`).
- The drawn balance is not stored as one number. Outstanding principal = `unbilledPrincipal + (nextDue - yieldDue) + dd.principalPastDue` (used in `contracts/credit/CreditDueManager.sol:184-187` and `321-324`).
- Approval rejects `committedAmount > creditLimit`, `creditLimit == 0`, `remainingPeriods == 0`, and `creditLimit > poolSettings.maxCreditLine` (`contracts/credit/CreditManager.sol:186-205`).
- The limit and commitment can change after the start through `updateLimitAndCommitment`, which only the evaluation agent may call (`contracts/credit/CreditLineManager.sol:143-152`; `CreditManager.sol:415-451`). Setting `creditLimit = 0` freezes further draws while keeping the commitment (`CreditManager.sol:420-425`).

### (b) Multiple drawdowns
- `CreditLine.drawdown(borrowAmount)` leads to `Credit._drawdown` (`contracts/credit/CreditLine.sol:15-23`; `contracts/credit/Credit.sol:180-257`).
- First draw (state `Approved`): it sets `unbilledPrincipal = borrowAmount`, generates the first bill and moves the state to `GoodStanding` (`Credit.sol:192-200`).
- Later draws are allowed only if `cc.revolving`. Otherwise the call reverts with `AttemptedDrawdownOnNonRevolvingLine` (`Credit.sol:201-203`). **In Huma, "non-revolving" means one draw only. There is no "several draws, but no re-draw of repaid principal" mode.**

### (c) Revolving versus non-revolving
- Revolving: the limit check counts only outstanding principal, `borrowAmount + unbilledPrincipal + (nextDue - yieldDue) > creditLimit`, and reverts if exceeded (`Credit.sol:218-219`). Repaid principal therefore becomes drawable again. The sum is written on the left-hand side so a lowered limit cannot underflow (`Credit.sol:214-217`).
- Non-revolving: a single draw, as described in (b).

### (d) Interest model
- **Time-based, day-granular, 30/360.** `_computeYield = principal * yieldInBps * numDays / (10000 * 360)` (`CreditDueManager.sol:416-423`; `DAYS_IN_A_YEAR = 360` in `contracts/common/SharedDefs.sol:7`). Days are 30/360 with the 31st clamped to the 30th (`contracts/common/Calendar.sol:17-33`, `69-87`).
- Interest is billed per period: monthly, quarterly or semi-annually (`SharedDefs.sol:17-20`). At each period roll, `accrued` = yield on outstanding principal and `committed` = yield on `committedAmount`, and `yieldDue = max(committed, accrued)` (`CreditDueManager.sol:184-196`). `DueDetail` stores `committed`, `accrued` and `paid` for the current period only (`CreditStructs.sol:71-81`).
- **Yield on the committed part.** This is not a separate fee on the undrawn amount. It is a minimum-utilization floor: interest is charged on `max(drawn, committedAmount)`. The same max is the basis for yield in skipped periods (`CreditDueManager.sol:116-119`) and for the late fee (`CreditDueManager.sol:321-325`).
- **Committed credit with a designated start date.** `approveBorrower(..., designatedStartDate, ...)` stores the start date in `cr.nextDueDate` while the state is `Approved` (`CreditManager.sol:225-233`). This requires a non-zero commitment and `remainingPeriods > 1` (`CreditManager.sol:190-200`). At that date, the pool owner or sentinel calls `startCommittedCredit` and billing on the commitment starts **without any drawdown** (`CreditLineManager.sol:77-84`; `CreditManager.sol:244-263`). `_isLate` treats a zero-balance credit as late if `committedAmount > 0` (`CreditDueManager.sol:376-381`).
- **Per-draw flat charge.** The front-loading fee is `frontLoadingFeeFlat + amount * frontLoadingFeeBps` and is deducted from the disbursement (`CreditDueManager.sol:17-24`, `280-286`; `Credit.sol:250-255`; struct `contracts/common/PoolConfig.sol:70-75`). This is the closest thing to a per-draw flat cost. Interest itself is never flat per draw.
- Optional amortization: `minPrincipalRateInBps` bills a percentage of unbilled principal each period (`PoolConfig.sol:77-85`; `CreditDueManager.sol:198-219`). All remaining principal falls due in the final period (`CreditDueManager.sol:214-218`).

### (e) Draws versus the schedule and due dates: mid-period proration (special attention)
- **First draw.** It creates a partial "stub" first period that runs from the draw to the start of the next calendar period. Yield is charged for exactly those days (`CreditDueManager.sol:164-176`, `daysUntilNextDue = getDaysDiff(timestamp, nextDueDate)`). `nextDueDate` is set to the start of the next period (`CreditDueManager.sol:166-168`).
- **Later draw in the middle of a period.** `computeAdditionalYieldAccruedAndPrincipalDueForDrawdown` takes `daysRemaining = calendar.getDaysRemainingInPeriod(nextDueDate)`. The count is inclusive of the draw day ("if the borrower draws down at 11:59 PM on October 30th, the yield for October 30th must be paid") and uses 30/360 (`CreditDueManager.sol:232-252`; `Calendar.sol:17-33`). It returns:
  - `additionalYieldAccrued = borrowAmount * yieldInBps * daysRemaining / (10000*360)`, which is added to `dd.accrued` (`Credit.sol:221-229`);
  - `additionalPrincipalDue = borrowAmount * minPrincipalRate * daysRemaining / (10000 * daysInFullPeriod)` when amortization is on. This moves from `unbilledPrincipal` into `nextDue` (`CreditDueManager.sol:242-250`; `Credit.sol:240-246`).
- **Three-case rule for yield due** (`Credit.sol:230-239`). If the new `accrued` is above `committed`, then `yieldDue = accrued - paid` and `nextDue` is rebuilt. Otherwise the yield due stays on the committed floor, so a draw that stays under the commitment adds no extra interest that period.
- **Mirror on early principal repayment.** `_makePrincipalPayment` removes yield for the remaining days on the repaid amount: `accrued -= min(accrued, computeYieldForRemainingDaysInPeriod(...))`, then re-applies `max(accrued, committed)`. Yield already paid is not refunded (`Credit.sol:489-521`). Principal-only payments must be enabled by the pool flag `principalOnlyPaymentAllowed` and are allowed only in `GoodStanding` (`Credit.sol:450-464`).
- **Draws after the due date.** A draw is blocked if `nextDue != 0 && block.timestamp > nextDueDate` (unpaid due, in grace or later), with `DrawdownNotAllowedAfterDueDateWithUnpaidDue` (`Credit.sol:601-605`). If the due was paid and the period has rolled, the bill is first refreshed with `getDueInfo`. The draw then reverts if the refreshed state is not `GoodStanding` or if `remainingPeriods == 0` (`Credit.sol:205-212`). A draw in `Delayed` or `Defaulted` reverts (`Credit.sol:599-600`).
- Bill refresh is lazy (done on read or write) through `getDueInfo`. Skipped periods are rolled forward in one step (`CreditDueManager.sol:58-149`).

### (f) Availability period / expiry of the undrawn commitment
- There is no separate "availability end" field. Draws are blocked once `remainingPeriods == 0` ("final period and beyond") (`Credit.sol:588`, `208-209`). The final period is used for repayment only.
- The first draw cannot happen before the designated start date (`Credit.sol:592-598`).
- `closeCredit` (borrower or evaluation agent) requires a zero balance. It is refused while a started commitment still has periods left (`CreditHasUnfulfilledCommitment`). It sets `creditLimit = 0` (`CreditLineManager.sol:111-119`; `CreditManager.sol:270-290`).
- The evaluation agent can add periods with `extendRemainingPeriod` (`CreditLineManager.sol:131-140`).

### (g) Minimum draw size
- The only floors are `borrowAmount != 0` (`Credit.sol:185`) and `borrowAmount >= platformFees`, enforced as `BorrowAmountLessThanPlatformFees` (`CreditDueManager.sol:22`). There is no configurable minimum draw. Other draw guards: enough free pool liquidity and a sufficient first-loss cover (`Credit.sol:589-591`).

### (h) Repayment application order
- Payoff when `amount >= payoff`: everything is cleared. The credit is deleted if `remainingPeriods == 0` (`Credit.sol:373-401`).
- Otherwise past due comes first. If the amount covers all past due, everything past due is cleared (`Credit.sol:291-304`). If not, the order is **yield past due, then late fee, then principal past due** (`Credit.sol:305-340`).
- Then next due: **yield due first, then principal due** (`Credit.sol:343-352`). Any excess reduces `unbilledPrincipal` (`Credit.sol:353-371`).
- A payment larger than the payoff amount is capped at the payoff (`Credit.sol:285-286`).
- Late fee: `lateFeeBps * max(principal, committed) * days / (10000*360)`, refreshed up to the end of the current day. It starts at `nextDueDate`, after the grace period `latePaymentGracePeriodInDays` (`CreditDueManager.sol:289-333`, `267-277`; `PoolConfig.sol:27-30`). The evaluation agent can waive it (`CreditLineManager.sol:155-163`).

### (i) State fields and types
- `CreditConfig { uint96 creditLimit; uint96 committedAmount; PayPeriodDuration periodDuration; uint16 numOfPeriods; uint16 yieldInBps; uint16 advanceRateInBps; bool revolving; bool receivableAutoApproval; }` (`CreditStructs.sol:24-33`).
- `CreditRecord { uint96 unbilledPrincipal; uint64 nextDueDate; uint96 nextDue; uint96 yieldDue; uint96 totalPastDue; uint16 missedPeriods; uint16 remainingPeriods; CreditState state; }` (`CreditStructs.sol:47-56`).
- `DueDetail { uint64 lateFeeUpdatedDate; uint96 lateFee; uint96 principalPastDue; uint96 yieldPastDue; uint96 committed; uint96 accrued; uint96 paid; }` (`CreditStructs.sol:71-81`).
- `CreditState { Deleted, Approved, GoodStanding, Delayed, Defaulted }` (`CreditStructs.sol:83-89`).
- Credit key: `keccak256(abi.encode(address(this), borrower))`, so each borrower has one line per credit contract (`CreditLine.sol:79-81`).

### (j) Events
- `CreditLineApproved(borrower, creditHash, creditLimit, periodDuration, remainingPeriods, yieldInBps, committedAmount, revolving)` (`CreditLineManager.sol:28-37`).
- `DrawdownMade(borrower, borrowAmount, netAmountToBorrower)` (`Credit.sol:57-61`).
- `BillRefreshed(creditHash, newDueDate, nextDue, totalPastDue)` on every due-info write (`Credit.sol:44-49`, `169`).
- `PaymentMade(borrower, payer, amount, yieldDuePaid, principalDuePaid, unbilledPrincipalPaid, yieldPastDuePaid, lateFeePaid, principalPastDuePaid, by)`: one field per bucket of the waterfall (`Credit.sol:76-87`).
- `PrincipalPaymentMade` (`Credit.sol:101-111`) and `CreditClosedAfterPayOff` (`Credit.sol:118`).
- Admin events: `CommittedCreditStarted`, `CreditClosedByAdmin`, `DefaultTriggered`, `RemainingPeriodsExtended`, `YieldUpdated`, `LimitAndCommitmentUpdated`, `LateFeeWaived` (`CreditManager.sol:22-105`).

### (k) Who may draw, and when
- Only the registered borrower of the credit hash (`CreditLine.sol:20-21`, `CreditManager.sol:131-133`), and only while the protocol and pool are on (`CreditLine.sol:18`).
- Repayment: the borrower, or the sentinel (auto-pay pulls from the borrower) (`CreditLine.sol:30-31`; `Credit.sol:647-649`). The pool-owner treasury can pay on behalf of the borrower (`CreditLine.sol:40-51`).
- Approval: evaluation agent only (`CreditLineManager.sol:49-50`). Re-approval is allowed only before the first draw (`CreditManager.sol:207-212`).

---

## 2. Goldfinch (TranchedPool + CreditLine; CallableLoan)

### (a) Commitment/limit versus drawn balance
- `CreditLine` storage: `uint256 currentLimit`, `uint256 maxLimit`, `uint256 interestApr`, `uint256 lateFeeApr`, `uint256 balance` (drawn and outstanding), `uint256 totalInterestPaid`, `uint256 lastFullPaymentTime`, `_totalInterestAccrued`, `_totalInterestOwed`, `_checkpointedAsOf`, `PaymentSchedule schedule` (`GF/protocol/core/CreditLine.sol:40-69`).
- `currentLimit` is set when the pool locks, to `min(limit() + deposits, maxLimit)`. The limit is therefore the capital actually raised, not an approved number (`GF/protocol/core/TranchedPool.sol:639-655`). `setLimit` must stay `<= maxLimit` (`CreditLine.sol:180-183`).
- Available limit: `limit() = currentLimit - totalPrincipalOwed()` (`CreditLine.sol:254-256`). It **shrinks as scheduled principal comes due**.

### (b) Multiple drawdowns
- `CreditLine.drawdown(amount)` requires `amount + balance <= limit()` and `amount > 0`. The schedule must be either not started or before `termEndTime` (`CreditLine.sol:152-174`).
- `TranchedPool.drawdown` draws only from the **current slice** (`TranchedPool.sol:206-220`). It lowers the tranche `principalSharePrice` to reflect the money left (`TranchedPool.sol:224-235`).
- CallableLoan allows "as many times as they want" during `DrawdownPeriod`, capped by undrawn deposits (`GF/interfaces/ICallableLoan.sol:17-18`; `GF/protocol/core/callable/structs/CallableCreditLine.sol:165-182`).

### (c) Revolving versus non-revolving
- There is no revolving flag. At the credit-line level, `balance + amount <= currentLimit - totalPrincipalOwed()` (`CreditLine.sol:157`, `254-256`). Principal prepaid ahead of schedule could in principle be drawn again. **Whether that happens in practice is not verified**: liquidity comes only from the current slice (`TranchedPool.sol:208-220`), and backers can withdraw redeemable principal after `lockedUntil` (`TranchedPool.sol:599`, `611-615`).
- In effect it is a non-revolving term loan with a drawdown window. CallableLoan caps draws at `totalPrincipalPaid()` (deposits not yet drawn) and only during `DrawdownPeriod` (`CallableCreditLine.sol:174-180`).

### (d) Interest model
- **Per-second simple interest on the drawn balance, 365-day year**: `balance * interestApr / 1e18 * secondsElapsed / SECONDS_PER_YEAR` (`CreditLine.sol:34-36`, `342-348`).
- Late fee: an extra `lateFeeApr` on `balance`. It accrues per second from `oldestUnpaidDueTime + latenessGracePeriodInDays` and is folded into accrued interest (`CreditLine.sol:350-365`).
- No commitment fee and no yield on undrawn capital (none found in `CreditLine.sol`).
- `interestOwed` versus `interestAccrued`. Interest becomes "owed" only when an interest due time is crossed. Within a period, owed stays fixed (`CreditLine.sol:232-251`). After `termEndTime`, all accrued interest is owed (`CreditLine.sol:234-238`).
- **Gotcha: principal owed is scheduled on `currentLimit`, not on the drawn balance.** `totalPrincipalOwedAt = currentLimit * principalPeriod / totalPrincipalPeriods` (`CreditLine.sol:279-287`), and `totalPrincipalPaid = currentLimit - balance` (`CreditLine.sol:259-261`). Undrawn limit therefore counts as "already paid principal".

### (e) Draws versus the schedule: drawdown after the term started (special attention)
- **First draw starts the clock.** If `balance == 0`, it sets `lastFullPaymentTime = now`. If the schedule is inactive, it calls `schedule.startAt(now)` (`CreditLine.sol:160-165`). The schedule creates a "stub period" from the draw to the end of the current calendar period, with no payment due at its end, treated as an extension of period 0 (`GF/protocol/core/schedule/Schedule.sol:42-44`). The schedule config is `periodsInTerm`, `periodsPerInterestPeriod`, `periodsPerPrincipalPeriod`, `gracePrincipalPeriods` (`Schedule.sol:63-111`).
- **Later draw: checkpoint, then mutate.** `_checkpoint()` runs **before** `balance` changes (`CreditLine.sol:167-172`). The comment there reads "The balance is about to change.. checkpoint amounts owed!". `_checkpoint` snapshots `_totalInterestOwed`, `_totalInterestAccrued` and `_checkpointedAsOf = now` (`CreditLine.sol:332-336`).
- Why the order matters: `_interestAccruedOverPeriod(start, end)` always multiplies by the **current** `balance` (`CreditLine.sol:342-348`). Checkpointing first locks the old balance's accrual up to now. After the draw, accrual runs on the new balance from now. There is no proration table, only the integral of balance over time, made exact by the checkpoints.
- Payments do the same: `pay()` checkpoints first (`CreditLine.sol:122-123`).
- **Draws while late are blocked.** `require(!_isLate(block.timestamp))` is evaluated **after** `balance` is incremented (`CreditLine.sol:173`). It works because the revert rolls everything back, but the check-after-mutate order is fragile. `_isLate` = `balance > 0 && now > nextDueTimeAt(lastFullPaymentTime)` (`CreditLine.sol:367-370`).
- Draws after `termEndTime` are blocked (`CreditLine.sol:153-156`). The admin can pause draws (`TranchedPool.sol:381-390`, `201`).

### (f) Availability period / expiry of the undrawn commitment
- TranchedPool: locking sets `lockedUntil = now + DrawdownPeriodInSeconds` (`GF/protocol/core/TranchingLogic.sol:52-55`). The comment says "We start the drawdown period, so backers can withdraw unused capital after borrower draws down" (`TranchedPool.sol:652-654`). After `lockedUntil`, backers withdraw redeemable principal (`TranchedPool.sol:599`, `611-616`). So the undrawn part effectively expires when backers pull it.
- CallableLoan makes this explicit: `loanPhase()` returns `DrawdownPeriod` while `now < termStartTime + DrawdownPeriodInSeconds`, then `InProgress`. Drawdown in any other phase reverts (`CallableCreditLine.sol:277-285`, `174-176`; `GF/interfaces/ICallableLoan.sol:10-25`).

### (g) Minimum draw size
- Only `amount > 0` (`CreditLine.sol:158`; `GF/protocol/core/callable/CallableLoan.sol:339-341`). There is no minimum draw size.

### (h) Repayment application order
- `Accountant.allocatePayment`: **(1) interest owed, (2) principal owed, (3) interest accrued but not yet owed, (4) remaining balance**. A principal payment while owed interest is unpaid reverts with "IO", and similarly "PO" and "AI" for the later steps (`GF/protocol/core/Accountant.sol:138-210`).
- `splitPayment` applies the same order to a lump sum (`Accountant.sol:113-136`).
- Late fees are part of interest (`CreditLine.sol:346-347`), so they have no separate bucket.
- The pool takes only what is owed (`TranchedPool.sol:318-323`, `341-351`).

### (i) State fields and types
- Listed in (a): all `uint256`, plus `PaymentSchedule { ISchedule schedule; uint64 startTime; }` (`CreditLine.sol:374-377`).
- `PaymentAllocation { owedInterestPayment; accruedInterestPayment; principalPayment; additionalBalancePayment; paymentRemaining }` (`GF/interfaces/ILoan.sol:103-109`).

### (j) Events
- `CreditLine` emits nothing. The pool emits `DrawdownMade(borrower, amount)`, `DrawdownsPaused`/`DrawdownsUnpaused` and `PaymentApplied(payer, pool, interest, principal, remaining, reserve)` (`GF/interfaces/ILoan.sol:117-124`, `141-143`; emitted at `TranchedPool.sol:241`, `525-532`).
- The pool also emits `SharePriceUpdated` (`TranchedPool.sol:242-257`).
- CallableLoan also emits `DepositsLocked` on the first draw (`CallableCreditLine.sol:172`).

### (k) Who may draw, and when
- `onlyLocker`. `LOCKER_ROLE` is granted to both the borrower and the owner (`TranchedPool.sol:85-87`, `200`, `694-696`). The pool must not be paused and drawdowns must not be paused (`TranchedPool.sol:200-201`).
- Anyone may `pay` (`TranchedPool.sol:314-357`).

---

## 3. Maple Finance v2

### 3a. Fixed-term loan (`maple-ftl`)

**(a) Commitment versus drawn balance.** Storage holds `uint256 _principalRequested`, `_principal` (outstanding), `_drawableFunds` (funded but not withdrawn), `_endingPrincipal`, `_nextPaymentDueDate`, `_paymentsRemaining`, `_paymentInterval`, `_gracePeriod`, the rates, `_collateralRequired` and `_collateral` (`contracts/MapleLoanStorage.sol:17-38`). `fundLoan` sets `_principal = principalRequested_` and `_drawableFunds += principalRequested - originationFees` (`contracts/MapleLoan.sol:376-386`).

**(b) Multiple drawdowns.** `drawdownFunds(amount_, destination_)` takes any partial amount from `_drawableFunds`. Each call is checked to keep collateral proportional to the drawn amount (`MapleLoan.sol:151-176`, `730-743`).

**(c) Revolving.** Yes, at the level of drawable funds. `returnFunds` adds any unaccounted funds back to `_drawableFunds` (`MapleLoan.sol:263-274`). Any excess sent with a payment also flows into `_drawableFunds` (`MapleLoan.sol:196`). This does **not** reduce `_principal` or interest. The modifier `limitDrawableUse` lets only the borrower consume drawable funds to pay; anyone else cannot reduce them (`MapleLoan.sol:41-53`).

**(d) Interest model.**
- **Interest runs on the full `_principal` from funding, whether drawn or not.** Installments are computed from `_principal` (`MapleLoan.sol:510-545`, `830-838`). Undrawn money therefore pays full interest; the commitment is the loan.
- Installments are amortizing, with a balloon set by `_endingPrincipal` (`MapleLoan.sol:746-781`).
- Periodic rate = `rate * interval / 365 days` (`MapleLoan.sol:863-865`).
- Late: `lateInterestPremiumRate` over whole days late (rounded up) plus a flat `lateFeeRate * principal` (`MapleLoan.sol:788-809`).
- Fees: an origination fee at funding (`MapleLoan.sol:376`), and delegate/platform service fees each payment (`MapleLoan.sol:851-859`, `910-922`).

**(e) Draws versus the schedule.** The schedule is fixed at funding: `_nextPaymentDueDate = now + paymentInterval` (`MapleLoan.sol:385`), then `+= _paymentInterval` per payment (`MapleLoan.sol:205-213`). A draw has no effect on interest or dates. `drawdownFunds` has **no lateness check** (verified by absence in `MapleLoan.sol:151-176`).

**(f) Availability period.** There is no expiry: undrawn funds stay in the loan until drawn or used to pay. A refinance can increase principal with `MapleRefinancer.increasePrincipal`, which raises `_principal`, `_principalRequested` and `_drawableFunds` (`contracts/MapleRefinancer.sol:25-34`). It is proposed by the borrower and accepted by the lender, with interest prorated up to the refinance (`MapleLoan.sol:239-252`, `294-357`).

**(g) Minimum draw.** None. Only an implicit underflow check on `_drawableFunds -= amount_` (`MapleLoan.sol:172`). Not even `amount > 0` is checked.

**(h) Repayment application order.** Each payment must be the whole installment: principal, interest (regular + late + refinance), and fees (`MapleLoan.sol:188-198`). There is no partial waterfall. `closeLoan` pays `principal * closingRate` plus fees for all remaining payments, and is refused once late (`MapleLoan.sol:112-149`, `496-508`).

**(i) Fields.** See (a). Rates are scaled by `HUNDRED_PERCENT = 1e6` (`MapleLoan.sol:37`).

**(j) Events.** `Funded(lender, amount, nextPaymentDueDate)`, `FundsDrawnDown(amount, destination)`, `FundsReturned(amount)`, `PaymentMade(principalPaid, interestPaid, fees)`, `LoanClosed(principalPaid, interestPaid, feesPaid)` (`contracts/interfaces/IMapleLoanEvents.sol:32`, `46`, `52`, `155`, `115`).

**(k) Who.** Draws: the borrower or a whitelisted `BORROWER_ACTIONS` contract (`MapleLoan.sol:155`, `933-935`). Funding, impairment and repossession: the lender (`MapleLoan.sol:359`, `389`, `413`). The borrower must `acceptLoanTerms` before funding (`MapleLoan.sol:104-110`, `360`).

### 3b. Open-term loan (`maple-otl`)

**(a)/(b)/(c)** There is **no drawdown function** (no "drawdown" match in `contracts/`). `fund()` sends the whole `principal` straight to the borrower (`maple-otl contracts/MapleLoan.sol:276-290`). Principal goes down through `makePayment(principalToReturn_)` (`MapleLoan.sol:180-215`). It goes up only by refinance (`contracts/MapleRefinancer.sol:30-31`), with the lender sending the difference (`MapleLoan.sol:171-177`). It is therefore not a revolving line.

**(d) Interest model.** Continuous per-second accrual from `max(datePaid, dateFunded)`: `principal * rate * seconds / (365 days * 1e6)` (`MapleLoan.sol:417-449`, `596-598`). Delegate and platform **service fees accrue the same way on principal** (`MapleLoan.sol:585-587`). Late: per-second premium plus a flat `lateFeeRate * principal` (`MapleLoan.sol:589-593`).

**(e) Due dates.** The normal due date is `max(dateFunded, datePaid) + paymentInterval` (rolling) (`MapleLoan.sol:566-570`). The effective due date is the minimum of the normal, called (`dateCalled + noticePeriod`) and impaired (`dateImpaired`) dates (`MapleLoan.sol:473-477`, `518-522`, `550-560`). The default date is the due date plus `gracePeriod` (the call has no grace) (`MapleLoan.sol:510-516`, `546-564`).

**(f)** Not applicable. **(g)** Not applicable.

**(h) Repayment.** One call pays all accrued interest, late interest and both fees, plus a borrower-chosen principal. That principal must be `>= calledPrincipal` (`MapleLoan.sol:193-199`). Any payment clears the call and the impairment (`MapleLoan.sol:207-210`).

**(i) Fields.** `uint32 gracePeriod, noticePeriod, paymentInterval`; `uint40 dateCalled, dateFunded, dateImpaired, datePaid`; `uint256 calledPrincipal, principal`; `uint64 delegateServiceFeeRate, interestRate, lateFeeRate, lateInterestPremiumRate, platformServiceFeeRate`; `bool loanTermsAccepted` (`contracts/MapleLoanStorage.sol:17-35`). The state is represented by dates, with no enum.

**(j) Events.** `Funded`, `Impaired`, `PaymentMade(lender, principalPaid, interestPaid, lateInterestPaid, delegateServiceFee, platformServiceFee, paymentDueDate, defaultDate)`, `PrincipalCalled`, `PrincipalReturned` (`contracts/interfaces/IMapleLoanEvents.sol:26`, `34`, `117`, `146`, `153`; emitted at `MapleLoan.sol:220-229`, `269-273`).

**(k) Who.** The lender funds, calls, impairs and repossesses (`MapleLoan.sol:260-301`, `343`). Anyone can `makePayment` (no role modifier, `MapleLoan.sol:180-181`).

---

## 4. Day-count and granularity at a glance

| Protocol | Accrual basis | Granularity |
|---|---|---|
| Huma | outstanding principal, floored at `committedAmount` | days, 30/360, inclusive of draw day (`CreditDueManager.sol:416-423`, `Calendar.sol:17-33`) |
| Goldfinch | drawn `balance` | seconds / 365d (`CreditLine.sol:342-348`) |
| Maple FTL | full `_principal` (incl. undrawn) | per interval / 365d; late in whole days (`MapleLoan.sol:863-865`, `805`) |
| Maple OTL | `principal` | seconds / 365d (`MapleLoan.sol:596-598`) |

None of the four charges **flat interest per draw**. The nearest analogs are Huma's per-draw front-loading fee (flat + bps) and Maple's flat `lateFeeRate * principal` and `closingRate`.

---

## Patterns worth copying for a Solana pool with installment lines

1. **Split static terms from dynamic state**, as Huma does with `CreditConfig` / `CreditRecord` / `DueDetail` (`CreditStructs.sol:24-81`). On Solana this maps to an immutable-ish `CreditLine` account plus a mutable bill/state. Keep amounts as `u64` with `u16` bps and `u16` period counters.
2. **Make the multi-draw policy explicit with an enum** (`SingleDraw | MultiDrawNonRevolving | Revolving`). Huma's `bool revolving` cannot express "several draws, no re-draw" (`Credit.sol:201-203`). For non-revolving, track `total_drawn` and check `total_drawn + amount <= limit`. For revolving, check `outstanding_principal + amount <= limit`, written as a sum to survive limit cuts (`Credit.sol:214-219`).
3. **Checkpoint accrued interest before every balance change** (draw, repay, rate change), like Goldfinch's `_checkpoint()` (`CreditLine.sol:167-172`, `332-336`). With time-based interest this is the single rule that makes mid-period draws exact.
4. **If you keep period bills, prorate a mid-period draw by the remaining days inclusive of today** and add it to the current bill. Mirror it when principal is prepaid (`CreditDueManager.sol:232-264`; `Credit.sol:221-246`, `489-521`).
5. **Block draws when anything is due and unpaid past the due date, and in the final period**, checking *before* mutating state (`Credit.sol:588`, `599-605`). Avoid Goldfinch's check-after-mutate order (`CreditLine.sol:172-173`).
6. **Have an explicit draw window**: an `availability_end` timestamp or "no draws in the last N installments", like CallableLoan's `DrawdownPeriod` (`CallableCreditLine.sol:277-285`) and Huma's `remainingPeriods == 0` gate.
7. **Use a strict repayment waterfall and emit one field per bucket**: past-due interest, then late fee, then past-due principal, then current interest, then current principal, then prepayment (`Credit.sol:291-371`; event `Credit.sol:76-87`). Cap payments at the payoff amount (`Credit.sol:285-286`).
8. **Lazy bill refresh**: roll skipped periods on the next touch (`CreditDueManager.sol:58-149`) instead of needing a crank. Add a permissionless `refresh` instruction for indexers (`CreditLineManager.sol:87-92`).
9. **Use a per-draw origination fee (flat + bps) deducted from the disbursement**, which fits elote's flat-fee economics better than a flat interest per draw (`CreditDueManager.sol:17-24`, `280-286`).

## Patterns to avoid

10. **Charging interest on undrawn funds by default** (Maple FTL charges interest on the full `_principal` from funding: `MapleLoan.sol:384`, `830-836`). If you want a commitment fee, make it an explicit, separately named field. Huma's `max(drawn, committed)` floor is opaque to borrowers (`CreditDueManager.sol:188-196`).
11. **Scheduling principal on the limit instead of the drawn amount** (Goldfinch `totalPrincipalOwedAt = currentLimit * period / total`, `CreditLine.sol:279-287`). For installment lines, re-amortize the drawn principal over the remaining installments.
12. **Calendar-period billing with 30/360 and month-boundary dates** (`Calendar.sol`) is a lot of date code for a Solana program. Prefer epoch-free unix-second due dates (`start + k * interval`), like Maple OTL (`MapleLoan.sol:566-570`).
13. **Requiring the exact full installment with no partial waterfall** (Maple FTL `makePayment`, `MapleLoan.sol:188-198`) does not fit micro-borrowers who pay in pieces.
14. **Late fees folded into "interest"** (Goldfinch `CreditLine.sol:346-347`) lose the audit trail. Keep `late_fee` as its own accumulator with a `late_fee_updated_at` (`CreditStructs.sol:72-73`).
15. **Relying on lender withdrawals to "expire" the commitment implicitly** (Goldfinch TranchedPool, `TranchedPool.sol:599`, `652-654`). Store the expiry on the line itself.
