# Multi-draw and revolving credit on Solana: how other programs model it

Prepared 2026-10-31 for the elote_pool multi-draw / revolving design. Read-only review: nothing from the downloaded material was built or run.

All repositories were cloned locally and read only. Citations are `path:line`, relative to the root of each artefact named in the "Source" block of each section. **"no verificado"** means the claim comes from an IDL, doc comment or a non-Solana port, not from Solana program source.

## What turned out to be public

| Protocol | Solana program source public? | What was actually read |
|---|---|---|
| Huma Finance | **No.** `00labs/huma-solana` does not exist; the 00labs org has no Solana credit program repo (`fluid-solana-programs` is a Jupiter Lend fork, overcollateralized, not a credit line). | (1) Anchor IDL of the Huma Solana program, with full doc comments, shipped in npm `@huma-finance/shared@0.0.81`. (2) The EVM Solidity source `00labs/huma-contracts-v2@21aaad5`, which has the same design and the same error names. |
| Jet Protocol v2 | **No longer.** `jet-lab/jet-v2` returns 404. The only copy found (`mattsse/jet-v2@74801fb`, May 2022) predates fixed-term. | Rust margin-pool from that copy; fixed-term **IDL** from a third-party IDL dump. |
| Credix | **No** program logic. `credix-permissioned-market` contains only a small program with 6 instructions. | IDL v0.1.0 in `credix-finance/credix-client@659f921` (Feb 2022) and IDL v3.13.0 in npm `@credix/credix-client@3.2.0`. |
| Maple (Solana) | **No.** The `maple-labs` org has 86 repos, all Solidity, JS, TS or Shell; none is a Solana program. | Nothing. |
| Others | Searches for Solana credit-line, revolving or drawdown programs found nothing usable. GitHub code search was rate-limited during the search. | — |

---

## 1. Huma Finance (Solana program "huma", IDL v2.2.0)

**Source**
- Solana IDL: `huma-shared-npm/package/dist/solana/idl/huma.json` (npm `@huma-finance/shared@0.0.81`; program id `EVQ4s1b6N1vmWFDv8PRNc77kufBP8HcrSNWXQAhRsJq9`; `metadata.version` 2.2.0). I cite the doc comments and error enum. **No Rust source.**
- EVM logic: `huma/` = `00labs/huma-contracts-v2@21aaad5` (Solidity). This is **verified EVM logic. Whether the Solana port does the same is no verificado**, except where a matching Solana error code or doc comment corroborates it.
- One known divergence: Solana `FeeStructure` has `front_loading_fee_flat`, `front_loading_fee_bps`, `yield_bps` and `late_fee_bps`, and **no** `min_principal_rate` (`huma.json:8463-8495`). EVM `FeeStructure` has `minPrincipalRateInBps` and no front-loading fields (`contracts/common/PoolConfig.sol:77-85`). The EVM rule that a mid-period draw adds principal due (`Credit.sol:242-246`, `CreditDueManager.sol:243-250`) therefore **cannot be assumed on Solana**.

### (a) Commitment or limit vs drawn balance
- Static config is in its own PDA, `CreditConfig`: `credit_limit: u128`, `committed_amount: u128`, `period_duration: PayPeriodDuration`, `num_periods: u32`, `yield_bps: u32` and `revolving: bool` (`huma.json:7870-7925`).
- `committed_amount` is a usage floor for yield: "If the used credit is less than this amount, the borrower will be charged yield using this amount" (`huma.json:7877-7878`).
- The drawn balance is not a single field. It is split across buckets:
  - `CreditRecord.unbilled_principal`, `next_due` (which includes the principal due) and `yield_due` (`huma.json:7927-7983`)
  - `DueDetail.principal_past_due` (`huma.json:8215`)
  - EVM outstanding principal = `unbilledPrincipal + nextDue - yieldDue + principalPastDue` (`CreditDueManager.sol:184-187`).
- Pool-level cap: `PoolSettings.max_credit_line` (`huma.json:10101`). EVM approval checks `committedAmount > creditLimit`, `creditLimit > maxCreditLine` and `remainingPeriods == 0` (`CreditManager.sol:188-204`). Solana has the same errors: 6603 `CreditLimitTooHigh`, 6604 `CommittedAmountExceedsCreditLimit`, 6601 `ZeroPayPeriodsProvided` (`huma.json:7401-7413`).
- The limit and the commitment can be changed after approval with `update_limit_and_commitment(new_credit_limit, new_committed_amount)`, signed by the EA (evaluation agent) (`huma.json:5636`).

### (b) Multiple drawdowns
- There is one `drawdown(amount: u128)` instruction, signed by `borrower` (`huma.json:2721`).
- The first draw moves the credit from `Approved` to `GoodStanding` and generates the first bill (`Credit.sol:192-200`).
- Later draws are allowed only when `revolving == true`. Otherwise the call reverts with `AttemptedDrawdownOnNonRevolvingLine` (`Credit.sol:202-203`); Solana error 6612 is `AttemptedDrawdownOnNonRevolvingCredit` (`huma.json:7445`).
- **Huma has no "multi-draw but non-revolving" mode:** non-revolving means exactly one draw.

### (c) Revolving availability
- The limit check on a repeat draw is `borrowAmount + unbilledPrincipal + (nextDue - yieldDue) > creditLimit` → `CreditLimitExceeded` (`Credit.sol:218-219`; Solana 6610 at `huma.json:7437`).
- Because the check uses the outstanding principal, repaid principal becomes drawable again automatically. There is no "total drawn" counter.
- The sum is written on the left-hand side on purpose, so a limit that was lowered below the outstanding principal does not underflow (`Credit.sol:214-217`).

### (d) Interest model
- Yield is time-based and simple, on the outstanding principal, with daily granularity under 30/360:
  - `principal * yieldBps * days / (10000 * 360)` (`CreditDueManager.sol:416-423`; `DAYS_IN_A_YEAR = 360` at `contracts/common/SharedDefs.sol:7`)
  - Solana doc: "We use the 30/360 day count convention" (`huma.json:9436-9439`).
- Periods are `Monthly`, `Quarterly` or `SemiAnnually` (`huma.json:9445-9451`).
- **Commitment fee:** there is no separate fee on the undrawn part. Instead, yield due = `max(committed, accrued) - paid` (`huma.json:8190`; EVM `CreditDueManager.sol:188-195`, `406-414`). This is a minimum-usage floor, not a fee on the undrawn amount.
- Per-draw upfront fee: on Solana this is `front_loading_fee_flat` + `front_loading_fee_bps` "charged ... when borrowing occurs" (`huma.json:8468-8469`). The borrower receives `net_amount_to_borrower` (`huma.json:8160-8161`). EVM equivalent: `distBorrowingAmount` (`CreditDueManager.sol:17-24`), which reverts `BorrowAmountLessThanPlatformFees` (Solana 6614, `huma.json:7453`). **This is the closest analogue to elote's flat interest per draw.**
- Late fee: `late_fee_bps` on the outstanding balance (`huma.json:8471-8472`). On EVM, the late-fee basis is the larger of the principal and the commitment (`CreditDueManager.sol:325`).

### (e) Draws vs the repayment schedule and due dates
- **Mid-period draw:** the new draw adds yield only for the days left in the current period, counting the draw day (`CreditDueManager.sol:238-241`). It is folded into `dd.accrued`, and `yieldDue` is recomputed only if accrued now exceeds committed (`Credit.sol:221-239`). The due date does not move.
- **Draw after `nextDueDate`:**
  - If there is unpaid due, the draw is blocked: `DrawdownNotAllowedAfterDueDateWithUnpaidDue` (`Credit.sol:601-606`; Solana 6616 at `huma.json:7461`).
  - If nothing is owed, the bill is first brought current with `getDueInfo`, then the program re-checks `remainingPeriods` and `GoodStanding` (`Credit.sol:205-212`).
- Billing is lazy: anyone can call `refresh_credit` ("Anyone can call", `huma.json:4086`). It emits `BillRefreshedEvent{new_due_date, next_due, total_past_due}` (`huma.json:7656-7692`).

### (f) Availability period and expiry
- `num_periods` sets when the credit expires (`huma.json:7880`).
- Draws are refused once `remainingPeriods == 0`, that is, in the final period and after it: `DrawdownNotAllowedInFinalPeriodAndBeyond` (`Credit.sol:588`, `208-209`; Solana 6615 at `huma.json:7457`).
- The EA can lengthen the term with `extend_remaining_periods(num_of_periods)` (`huma.json:3031`).
- Optional **designated start date:**
  - `approve_credit(..., designated_start_date, ...)` (`huma.json:582`). Before the first draw, `nextDueDate` stores the start date (`CreditManager.sol:225-231`).
  - Drawing earlier reverts `FirstDrawdownTooEarly` (`Credit.sol:596`; Solana 6611).
  - If the borrower never draws, `start_committed_credit` (EA or Sentinel) starts billing the commitment anyway (`huma.json:5145`; `CreditManager.sol:244-263`).
- A started credit with an unfulfilled commitment cannot be closed: `CreditHasUnfulfilledCommitment` (`CreditManager.sol:277-279`; Solana 6629).

### (g) Minimum draw size
- None found. The only checks are `ZeroAmountProvided` (`Credit.sol:185`) and draw ≥ upfront fees (`CreditDueManager.sol:22`). `min_deposit_amount` applies to lenders, not draws (`huma.json:10084-10086`). No minimum draw was found.

### (h) Repayment application order
`make_payment` (`Credit.sol:289-372`) applies a payment in this order:
1. Past due: all of it if the payment covers it. Otherwise yield past due, then late fee, then principal past due (`Credit.sol:305-334`).
2. Next due: yield first, then principal (`Credit.sol:344-352`).
3. Any surplus reduces `unbilledPrincipal`, and the bill is refreshed if the period has rolled over (`Credit.sol:353-371`).

Other rules:
- A payment at or above the payoff amount takes the payoff path, and any excess is refused (`Credit.sol:285-286`, `373-401`). The payoff amount is `unbilledPrincipal + nextDue + totalPastDue` (`CreditDueManager.sol:225-228`).
- Paying off in the final period closes the credit and emits `CreditClosedAfterPayOff` (`Credit.sol:396-400`).
- `make_principal_payment` is a principal-only path: GoodStanding only, gated by `principal_only_payment_allowed` (`Credit.sol:444-470`; `huma.json:3579`, `10133`).
- The Solana `PaymentMadeEvent` fields mirror the same buckets (`huma.json:9491-9562`).

### (i) State fields and types (Solana IDL)
- `CreditConfig` (`huma.json:7870-7925`): `bump u8`, `credit_limit u128`, `committed_amount u128`, `period_duration enum`, `num_periods u32`, `yield_bps u32`, `revolving bool`.
- `CreditState` (`huma.json:7985-8030`): `bump u8`, `borrower Pubkey`, `credit_record CreditRecord`, `due_detail DueDetail`, `receivable_available_credits u128`.
- `CreditRecord` (`huma.json:7927-7983`): `unbilled_principal u128`, `next_due_date u64`, `next_due u128`, `yield_due u128`, `total_past_due u128`, `missed_periods u32`, `remaining_periods u32`, `status CreditStatus`.
- `DueDetail` (`huma.json:8186-8236`): `late_fee_updated_date u64`, `late_fee u128`, `principal_past_due u128`, `yield_past_due u128`, `committed u128`, `accrued u128`, `paid u128`.
- `CreditStatus` = `Deleted | Approved | GoodStanding | Delayed | Defaulted` (`huma.json:8032-8053`).
- Amounts are `u128` and dates are `u64` unix seconds.

### (j) Events
- `CreditApprovedEvent{pool, borrower, credit_limit, period_duration, remaining_periods, yield_bps, committed_amount, designated_start_date, revolving}` (`huma.json:7761-7822`).
- `DrawdownMadeEvent{pool, borrower, borrow_amount, net_amount_to_borrower}` (`huma.json:8153-8184`). **Gap:** it carries no outstanding balance and no remaining availability.
- `PaymentMadeEvent` with a per-bucket breakdown (`huma.json:9491-9562`).
- `PrincipalPaymentMadeEvent` (`huma.json:10293-10345`).
- `BillRefreshedEvent` (`huma.json:7656-7692`).
- `CommittedCreditStartedEvent` (`huma.json:7694-7715`).
- `LimitAndCommitmentUpdatedEvent{old_*, new_*}` (`huma.json:9096-9137`).
- `CreditClosedAfterPayOffEvent` (`huma.json:7824-7845`).
- `DefaultTriggeredEvent{principal_loss, yield_loss, fees_loss}` (`huma.json:8076-8112`).

### (k) Who may draw, and when
- Only the borrower may draw (`huma.json:2721`).
- The program refuses a draw when:
  - the credit is not Approved or GoodStanding, that is, it is Delayed or Defaulted: `CreditNotInStateForDrawdown` (`Credit.sol:599-600`; Solana 6609)
  - there is unpaid due past the due date (6616)
  - the protocol is paused (6302 `ProtocolIsPaused`, `huma.json:7273`; `pause_protocol` at `huma.json:3994`)
  - the pool is disabled ("no money moves in or out", `huma.json:2527`)
  - the pool balance is insufficient (`Credit.sol:590-591`; 6613)
  - on EVM only, first-loss cover is insufficient (`Credit.sol:589`).
- `approve_credit`, `update_limit_and_commitment`, `trigger_default`, `waive_late_fee` and `extend_remaining_periods` are EA-only (`huma.json:582`, `5636`, `5423`, `6101`, `3031`).
- `make_payment` accepts the borrower or a Sentinel service account (`huma.json:3412`).

---

## 2. Jet Protocol v2

**Source**
- Rust (verified): `jet-v2/` = `mattsse/jet-v2@74801fb` (2022-05). It contains `programs/margin-pool` only, with no fixed-term program.
- Fixed-term: IDL only, `jet-fixed-term-idl/idl-full.json`, fetched from the third-party repo `staccDOTsol/openai-idl-generat00r` (`idl_data/JPTermEg2DwrV39xb1Fs7z1VUxcvdPT7mE7cyGsQ4xt.json`). **All fixed-term logic is no verificado.**
- Jet is an orderbook term-lending market, not a credit line. Only some patterns transfer to elote_pool.

### (a) Commitment or limit vs drawn
- There is no credit limit. The ceiling is set by margin collateral (margin account health).
- `Debt` (`idl-full.json:2259-2287`) has `pending u64` and `committed u64`. **`Debt.committed` is not a credit commitment:** its meaning is no verificado, but it is orderbook debt, either filled or posted. Do not read it as an undrawn line.

### (b) Multiple drawdowns
- Each fill creates its own `TermLoan` account (`idl-full.json:2088-2148`) with:
  - `sequenceNumber`, `maturationTimestamp`, `strikeTimestamp`
  - `principal u64`, `interest u64`, `balance u64`, `flags`.
- `Debt` tracks `nextNewTermLoanSeqno`, `nextUnpaidTermLoanSeqno` and `nextTermLoanMaturity` (`idl-full.json:2264-2275`).
- This is the "one account per draw" pattern (no verificado beyond the IDL).

### (c) Revolving
- No revolving line. The closest is auto-roll: `BorrowAutoRollConfig{limitPrice, rollTenor}` (`idl-full.json:2334-2347`) and the `autoRollBorrowOrder`, `toggleAutoRollLoan` and `configureAutoRollBorrow` instructions (`idl-full.json:429`, `718`, `636`). At maturity a new `TermLoan` refinances the old one (no verificado).

### (d) Interest model
- **Fixed-term (IDL):** the interest is fixed when the loan is created. `TermLoan.interest` is stored next to `principal` (`idl-full.json:2129-2134`), and the rate comes from the orderbook price. There is also a market-level `originationFee` (`idl-full.json:1997`).
- **Margin pool (verified Rust):** variable APR, compounded per second, on the total borrowed:
  - `accrue_interest` uses an `accrued_until` timestamp (`programs/margin-pool/src/state.rs:151-173`).
  - Catch-up is capped at `MAX_ACCRUAL_SECONDS = SECONDS_PER_WEEK` per call (`programs/margin-pool/src/util.rs:26-27`).
  - The rate is piecewise-linear in utilization (`state.rs:177-200`).
  - Borrowers hold `loan_notes` (`state.rs:79`, `127-138`).
- No commitment fee.

### (e) Schedule and due dates
- Each loan has a single bullet maturity, `maturationTimestamp` (`idl-full.json:2117-2121`). There are no installments.
- `Debt.nextTermLoanMaturity` holds the earliest maturity (`idl-full.json:2272-2275`).

### (f) Availability or expiry
- Not applicable. The tenor is per market (`borrowTenor`, `lendTenor` at `idl-full.json:1989-1994`).

### (g) Minimum draw
- The orderbook has `InitializeOrderbookParams.minBaseOrderSize` (`idl-full.json:2252`). Error `OrderRejected` reads "...or the order was too small" (`idl-full.json:2857`).

### (h) Repayment order
- `repay(amount)` takes both `termLoan` and `nextTermLoan` (`idl-full.json:1216-1229`).
- Combined with `nextUnpaidTermLoanSeqno` and `TermLoanHasWrongSequenceNumber` (`idl-full.json:2872`), this implies loans are repaid **FIFO by sequence number**: oldest draw first (no verificado).

### (i) State
- See `TermLoan` (`idl-full.json:2088-2148`), `MarginUser{debt, assets, borrowRollConfig, lendRollConfig}` (`idl-full.json:2028-2085`) and `Market` (`idl-full.json:1881-2005`).

### (j) Events
- No Anchor `events` in the IDL. The program uses its own orderbook event queue (`consumeEvents`) (no verificado).

### (k) Who may borrow, and pause
- The borrower borrows through a margin account (`marginBorrowOrder` accounts at `idl-full.json:825-840`).
- The market can be paused: `pauseOrderMatching` (`idl-full.json:362`) → `OrderbookPaused` (`idl-full.json:2852`).
- `NonZeroDebt` blocks closing a user that still has debt (`idl-full.json:2837`).

---

## 3. Credix Finance

**Source**
- `credix-permissioned-market/` is a different, minimal program: market, deposit and CredixPass, with no deals (`programs/credix/src/state.rs:1-18`, `lib.rs:29-62`). It is not relevant here.
- `credix-client/` = GitHub `credix-finance/credix-client@659f921` (2022-02). IDL v0.1.0 in `src/idl/credix.ts`, plus client helpers.
- `credix-client-npm-3.2.0/package/` = npm `@credix/credix-client@3.2.0`. IDL v3.13.0 in `dist/idl/credix.d.ts`.
- **No program logic is public. Everything below is no verificado (IDL and doc comments only).**

### v0.1.0 (2022): single draw, flat fee
- `Deal{principal u64, financingFeePercentage Ratio, principalAmountRepaid u64, interestAmountRepaid u64, timeToMaturityDays u16, goLiveAt i64, lateFees u64, lateFeesRepaid u64, defaulted bool, ...}` (`src/idl/credix.ts:638-716`).
- `activateDeal` takes no args. It sends the whole principal at once, so there is exactly one draw (`src/idl/credix.ts:266-341`; client `src/accounts/Deal.ts:242-276`).
- Interest is a flat fee for the whole term: `principal × fee × timeToMaturityDays/360`, computed **client-side** (`src/accounts/Deal.ts:155-161`).
- `makeDealRepayment(amount, repaymentType: Interest|Principal)` (`src/idl/credix.ts:342`, `400-424`, `812-824`). Error `InterestBeforePrincipal`: "Repay interest before principal" (`src/idl/credix.ts:1139-1143`).
- The client's `repayPrincipal` and `repayInterest` pass the **swapped** enum (`src/accounts/Deal.ts:283-294`), a reminder to test the repayment-type argument.
- Events: `DealCreationEvent`, `DealActivationEvent`, `DealInterestRepaymentEvent`, `DealPrincipalRepaymentEvent` (`src/idl/credix.ts:826-962`).

### v3.13.0 (npm 3.2.0): structured deals (all line numbers in `package/dist/idl/credix.d.ts`)

**(a) Commitment vs drawn**
- The commitment is the sum of funded tranche sizes: `Tranche{size u64, outstandingPrincipal u64, rates, amountsDue, amountsRepaid, ...}` (`8432-8470`).
- The drawn amount is `Deal.amountWithdrawn: u64`, documented as "The principal amount withdrawn from deal token account by borrower" (`7439-7443`).
- Funds sit in a deal token account after activation (`activateDeal` docs, `982-990`).

**(b) Multiple draws**
- `withdrawFromDeal(amount: u64)` (`2967`, args `3191-3194`), signed by `signer` with `borrower` passed separately (`2970-2975`), so a manager can sign for a borrower (inferred from `createManagedBorrower` at `3780`).
- Several partial withdrawals are possible because `amountWithdrawn` is a running total (no verificado).
- `returnFundsToBorrower` (`1658`) and `reduceOutstandingPrincipal(trancheIndex, amount)` (`6800`, args `6917-6924`) are admin corrections.
- Upscaling a deal: `upscaleSize` and `interestRepaidUntilLastUpscale` on `DealTranche` (`8357-8430`); `originalGoLiveAt` docs "When upscaling ... reset `go_live_at` ... requiring a new activation" (`7495-7501`).

**(c) Revolving**
- The waterfall has a phase type, `DistributionWaterfallType = Acceleration | Amortization | Revolving` (`9040-9051`).
- `repayDeal` doc: "If there is leftover budget and we are in not in revolving phase (i.e periods expect principal repayment) early principal repayment happens" (`1224`).
- `RepaymentPeriod.principalExpected: Option<u64>`: "If it's `None`, any amount of principal can be repaid in a period, we also don't incur early fees on the principal repaid" (`8776-8783`).
- So in Credix, revolving is **a phase of the repayment schedule** (interest-only periods during which principal may move freely), not a re-draw flag. Whether repaid principal can be withdrawn again is no verificado.

**(d) Interest**
- Interest is per tranche: `TrancheRates{interest, lateInterestFee, interestPerformanceFee, latePrincipalFee, principalPerformanceFee, earlyPrincipalFee, membershipFee}`, all `Fraction` (`8496-8547`).
- Day count is `Act360 | Act365` (`9026-9034`).
- Variable rate: `VariableRate = None | SOFR` (`8992-9004`); `interestRateUpdatedAt` lives on `DealTranche`.
- `RepaymentAllocation` includes `CompoundingInterest` and `EarlyPrincipalFee` (`9057-9090`).
- `arrangementFee: Fraction` is charged yearly (`timeLatestArrangementFeesCharged`, `7473-7485`, `7513-7517`).
- No fee on the undrawn commitment was found.

**(e) Schedule**
- `setRepaymentSchedule(totalPeriods, offset, repaymentPeriods: Vec<RepaymentPeriod>, startTs, daycountConvention, waterfallDefinitions)` (`619`, args `715-746`). It is written in chunks using `offset`.
- `adjustRepaymentSchedule(newLength, changeIndexFrom, repaymentPeriods)` (`5635`, args `5757-5770`).
- `RepaymentPeriod{waterfallIndex u64, accrualInDays u32, principalExpected Option<u64>, timeFrame{start,end i64}, calculationWaterfallIndex u64, calculationDate i64}` (`8757-8805`).
- "These calculations happen off-chain and we just store the result on-chain" (`8770-8771`). Accrual days are precomputed off-chain, so a mid-period draw is handled off-chain by rewriting the schedule (no verificado).
- `Deal.prevUpdateTs: Option<i64>` holds the last time due amounts were updated (`7504-7511`), and `updateTranchesAmountsDue` exists (`6359`).

**(f) Expiry**
- The only expiry found is on the funding side: `maxFundingDuration u8`, "number of days after tranche investors can call burn tranches if the deal does not go live" (`7454-7458`). No expiry of the borrower's undrawn amount was found.

**(g) Minimum draw**
- None found.

**(h) Application order**
- `DistributionWaterfall{waterfallType, tiers}`: "The order of the tiers determines the order of allocations of a repayment" (`8829-8855`).
- `WaterfallTier{trancheIndices, slash bool, charge bool, allocations}`: pro rata within a tier, or sequential (by seniority) when `slash` is set (`8857-8905`).
- The order is configurable per deal, not hard-coded.

**(i) State**
- `Deal` (`7426-7545`), `RepaymentSchedule{totalPeriods u16, startTs i64, daycountConvention, periods Vec<RepaymentPeriod>, waterfallDefinitions Vec<DistributionWaterfall>}` (`7800-7837`), `TrancheAmountsDue` / `TrancheAmountsRepaid` (`8549-8631`).
- `DealStatus = Pending | Structuring | OpenForFunding | InProgress` (`9006-9024`).

**(j) Events**
- No `events` section was found in v3.13.0 (the list holds only instructions, accounts, types and errors, `4`, `7226`, `7937`, `9112`). v0.1.0 had them (see above).

**(k) Who and when**
- `repayDeal`: the deal must be InProgress and the borrower needs an active CredixPass with `isBorrower` (`1218-1221`).
- `withdrawFromDeal` takes `credixPass` and `marketAdmins` (`2970-3141`).
- Market freeze: `freezeGlobalMarketState` → `MarketIsFrozen` (`2172`, `9135`).
- Errors `DealNotInProgress` and `CredixPassInactive` (`9220`, `9125`).

---

## 4. Maple Finance (Solana)

- No open-source Solana program. The `maple-labs` GitHub org lists 86 repos, all Solidity, JavaScript, TypeScript or Shell. No Solana or Anchor program was found.

## 5. Other Solana programs

- `00labs/fluid-solana-programs` is Jupiter Lend (liquidity, lending and vaults; overcollateralized), with no credit-line concept (`README.md:1-3`).
- No other open-source Solana credit-line or revolving program was found in the time box. Jia and Untangled have no public Solana source that I could locate.

---

## Patterns worth copying for elote_pool

1. **Split static terms from moving balances** (Huma `CreditConfig` vs `CreditState`, `huma.json:7870-8030`). Put `credit_limit`, `committed_amount`, `revolving`, `num_periods` and the rate in one PDA. Put `unbilled_principal`, `next_due`, `next_due_date`, `remaining_periods` and `status` in another.
2. **Check availability against the outstanding principal, summed on the left** (`borrow + outstanding > limit`, `Credit.sol:214-219`). This gives revolving behaviour for free and survives a lowered limit.
3. **One `revolving: bool`, set at approval.** If elote wants "multi-draw but non-revolving", add an explicit `total_drawn` counter checked against the limit, because Huma has no such mode (`Credit.sol:202-203`).
4. **Draw eligibility gates in one helper** (`Credit.sol:583-607`):
   - not before the start date
   - not in or after the final period
   - not Delayed or Defaulted
   - not after the due date while anything is unpaid
   - enough pool liquidity.
5. **Mid-period draw:** charge interest only for the days left in the current period, counting the draw day, and leave the due date alone (`CreditDueManager.sol:238-241`). A flat-per-draw variant would prorate the flat fee the same way.
6. **Bring the bill current before any draw or payment that happens after the due date** (`Credit.sol:205-212`, `282`). Expose a permissionless `refresh_credit` (`huma.json:4086`).
7. **Fixed repayment order, with the buckets emitted in the event** (`Credit.sol:289-372`; `PaymentMadeEvent` `huma.json:9491-9562`): yield past due → late fee → principal past due → yield due → principal due → unbilled principal. Accept at most the payoff amount.
8. **Per-draw upfront fee netted from the disbursement**, with `net_amount_to_borrower` in the event (`huma.json:8160-8161`, `8468-8469`). It maps directly onto elote's flat interest per draw if elote charges at draw time.
9. **Expiry by period count, extendable only by an admin role** (`num_periods`, `extend_remaining_periods`, `huma.json:7880`, `3031`).
10. **If installments must follow each draw separately, use one account per draw with a sequence number and FIFO repayment** (Jet `TermLoan` + `Debt.nextUnpaidTermLoanSeqno`, `idl-full.json:2088-2148`, `2264-2275`).

## Patterns to avoid

11. **Do not port EVM Huma rules blindly.** Solana `FeeStructure` drops `minPrincipalRateInBps` (`huma.json:8463-8495` vs `PoolConfig.sol:77-85`), so the principal-due-on-draw rule may not exist there.
12. **Avoid the commitment-as-floor charge unless the business really wants it.** It bills yield on money never drawn (`huma.json:8190`) and blocks closing the credit (`CreditManager.sol:277-279`), which is hard to explain to small borrowers.
13. **Do not compute interest or schedules off-chain and store only the result** (Credix `accrualInDays` "calculations happen off-chain", `credix.d.ts:8770-8771`; v0.1 interest computed in the client, `Deal.ts:155-161`). elote's on-chain due amounts should be derivable from state.
14. **Do not let the caller choose the repayment bucket** (Credix v0.1 `repaymentType` argument; the client swapped it, `Deal.ts:283-294`). Apply a fixed waterfall in the program.
15. **Do not emit a draw event without the resulting outstanding balance and remaining availability** (Huma `DrawdownMadeEvent`, `huma.json:8153-8184`). Indexers then have to replay every event to know the line's state.
