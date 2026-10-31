# Ethereum revolving-credit protocols: how they model draws, limits and accrual

Written for: elote_pool multi-draw / revolving credit line design. Date: 2026-10-31.
Method: shallow clones, grep and read only. Nothing was built or run. Every code claim cites `path:line` relative to the repo root. "no verificado" means the code read does not settle the point.

## Repos and commits read

| Protocol | Repo used | Commit | Note |
|---|---|---|---|
| Wildcat v2 | `wildcat-finance/v2-protocol` | `f5a2614` (2026-08-19) | as requested |
| Atlendis | `Atlendis/protocol-v1` | `237457c` (2024-06-05) | GitHub org `atlendis-labs` does not exist; the `Atlendis` org holds only `protocol-v1`, whitepapers and forks. The v2 "RCL" contracts are not public there: **no verificado**. |
| TrueFi ALOC | `TrueFi-Protocol/contracts-beryllium` | `2dad731` (2024-04-16) | `trusttoken/trufi-automated-lines-of-credit` returns 404. ALOC lives at repo root `AutomatedLineOfCredit.sol`. An older copy is in `TrueFi-Protocol/contracts-helium@631e68b` (not cited). |
| TrueFi LoC Agency | `trusttoken/contracts-pre22` | `b118cc1` (2025-02-17) | `contracts/truefi2/LineOfCreditAgency.sol` |
| Aave v3 | `aave/aave-v3-core` | `782f519` (2024-09-03) | delegation only |

---

## 1. Wildcat v2 (`v2-protocol`)

Wildcat is an undercollateralised market with one borrower. Lenders deposit into the market and receive a rebasing token. The borrower may withdraw ("borrow") anything above a required reserve. The borrower's debt **is** the lenders' token supply.

**(a) Commitment/limit vs drawn.** There is no borrower credit limit. `maxTotalSupply` is a **deposit cap**: `maximumDeposit = maxTotalSupply − totalSupply` (`src/libraries/MarketState.sol:59-61`), applied to deposits (`src/market/WildcatMarket.sol:64`). Its setter says it "only limits deposits and does not affect interest accrual" (`src/market/WildcatMarketConfig.sol:94-111`). The draw limit is `borrowableAssets = satSub(totalAssets, liquidityRequired)` (`src/libraries/MarketState.sol:123-128`). `liquidityRequired` is 100% of pending and unclaimed withdrawals, plus `reserveRatioBips` × the remaining supply, plus protocol fees (`MarketState.sol:87-98`). `totalAssets` is the market's token balance (`src/market/WildcatMarketBase.sol:298-300`). Nothing stores the drawn amount. It is implicit: `totalDebts − totalAssets` (`MarketState.sol:138-143`).

**(b) Multiple draws.** `borrow(amount)` can be called any number of times, each time limited by `borrowableAssets` (`WildcatMarket.sol:146-166`).

**(c) Revolving.** Yes, implicitly. `repay` only transfers assets into the market (`WildcatMarket.sol:188-201`), which raises `totalAssets` and therefore `borrowableAssets`. The borrower can draw them again unless withdrawal batches claim them first (see h).

**(d) Interest model. This is the key mechanic.**
- Interest accrues on the **whole lender supply, not on the drawn balance**. `borrow()` changes no supply field (`WildcatMarket.sol:154-165`), and the scale factor multiplies `scaledTotalSupply` (`MarketState.sol:51-53, 66-71`). Liquid reserves sitting in the market therefore pay the full APR. In effect, the "fee on undrawn" is 100% of APR.
- The rate is a fixed `annualInterestBips` (uint16) that the borrower can change (`WildcatMarketConfig.sol:123-162`). It is capped at 10,000 bips (`WildcatMarketConfig.sol:138-140`). The constraint hook makes an APR cut of more than 25% temporarily raise the reserve ratio for two weeks (`src/access/MarketConstraintHooks.sol:160-186, 188-240`; the `2 weeks` expiry is at `:259`). There is no utilisation curve.
- Per-second math: `rateRay = bips × 1e23`, then `interestRay = rateRay × dt / 365 days`, with floor division (`src/libraries/MathUtils.sol:12-14, 30-39`; duplicated in `src/libraries/FeeMath.sol:19-28`). `dt = now − lastInterestAccruedTimestamp` (`FeeMath.sol:30-38`).
- Checkpoint: `scaleFactor += scaleFactor.rayMul(baseInterestRay + delinquencyFeeRay)` and `lastInterestAccruedTimestamp = now` (`FeeMath.sol:164-169`). Growth is linear inside an interval and compounds across checkpoints, because each delta multiplies the previous scale factor. `scaleFactor` starts at RAY (1e27) (`WildcatMarketBase.sol:198-199`).
- Rounding: `rayMul` and `rayDiv` round half up (`MathUtils.sol:149-175`). Normalisation is `scaled.rayMul(scaleFactor)` and scaling is `amount.rayDiv(scaleFactor)` (`MarketState.sol:66-78`). Withdrawal payouts deliberately round **down** with `mulDiv` so that closed markets can always finish paying (`WildcatMarketBase.sol:663-665`).
- Protocol fee: `protocolFeeBips` × base interest, charged **on top of** lender interest. It accrues as a normalised amount in `accruedProtocolFees` (`FeeMath.sol:40-50`). It is capped at 1,000 bips (`WildcatMarketConfig.sol:164-166`).
- Every state-touching entry point calls `_getUpdatedState()` first. That accrues up to an expired batch's expiry, processes the batch, then accrues up to `block.timestamp` (`WildcatMarketBase.sol:406-457`). `_writeState` recomputes `isDelinquent = liquidityRequired > totalAssets` (`WildcatMarketBase.sol:528-530`).

**(e) Payment cycles, delinquency, draws while delinquent.** There is no fixed payment schedule. The "due date" is the withdrawal-batch expiry: `withdrawalBatchDuration` (`WildcatMarketBase.sol:63-64`) and `hasPendingExpiredBatch` (`MarketState.sol:130-136`). The market is delinquent when reserves fall below `liquidityRequired` (`WildcatMarketBase.sol:529`).
- Penalty APR: `delinquencyFeeBips` is added to the base rate only for seconds beyond `delinquencyGracePeriod`. The tracker `timeDelinquent` (uint32) counts **up** while delinquent and **down** while healthy, so the penalty keeps running after the borrower cures until the grace credit has been "paid back" (`FeeMath.sol:52-122`). Both values are immutable per market (`WildcatMarketBase.sol:57-61, 211-212`), and the grace period is at most 90 days (`MarketConstraintHooks.sol:40-41`).
- Draws while delinquent: **no explicit check**. `borrow()` has no `isDelinquent` test (`WildcatMarket.sol:146-166`). The block is arithmetic: when delinquent, `liquidityRequired > totalAssets`, so `borrowableAssets = 0` (`MarketState.sol:123-128`, `satSub` at `MathUtils.sol:59-65`), and any `amount > 0` reverts `BorrowAmountTooHigh` (`WildcatMarket.sol:157-158`).

**(f) Availability period, expiry, maturity.** Markets are open-ended. `closeMarket()` by the borrower collects the remaining debt, sets the APR to 0 and the reserve ratio to 100%, and sets `isClosed` (`WildcatMarket.sol:212-293`). Borrowing from a closed market reverts (`WildcatMarket.sol:155`). `FixedTermHooks.fixedTermEndTime` blocks **lender withdrawals** before the term ends (`src/access/FixedTermHooks.sol:315-317`), not borrower draws. Closing early is blocked unless allowed (`FixedTermHooks.sol:405-418`), and the APR cannot be cut before the term ends (`FixedTermHooks.sol:445-451`).

**(g) Minimum draw.** None for borrowing. `onBorrow` is empty in both shipped hooks (`FixedTermHooks.sol:390-394`, `src/access/OpenTermHooks.sol:329-333`). `minimumDeposit` applies to lenders only (`OpenTermHooks.sol:226`, `FixedTermHooks.sol:280`). A zero repay reverts (`WildcatMarket.sol:189`).

**(h) Repayment application order.** The borrower does not split payments into principal and interest; the money just enters the market. When state is next updated, liquidity goes first to the pending withdrawal batch, after earlier unpaid batches, processed withdrawals and fees are set aside (`src/libraries/Withdrawal.sol:45-57`; `WildcatMarketBase.sol:443-456`). Unpaid batches are then paid FIFO through `repayAndProcessUnpaidWithdrawalBatches` (`src/market/WildcatMarketWithdrawals.sol:279-341`). Payment burns scaled supply, which stops interest on that part (`WildcatMarketBase.sol:674-675`). Fees are collectable only after unclaimed withdrawals (`MarketState.sol:105-111`).

**(i) State fields and types.** `MarketState` (`MarketState.sol:12-38`):

| Field | Type |
|---|---|
| `isClosed` | bool |
| `maxTotalSupply` | uint128 |
| `accruedProtocolFees` | uint128 |
| `normalizedUnclaimedWithdrawals` | uint128 |
| `scaledTotalSupply` | uint104 |
| `scaledPendingWithdrawals` | uint104 |
| `pendingWithdrawalExpiry` | uint32 |
| `isDelinquent` | bool |
| `timeDelinquent` | uint32 |
| `protocolFeeBips` | uint16 |
| `annualInterestBips` | uint16 |
| `reserveRatioBips` | uint16 |
| `scaleFactor` | uint112 |
| `lastInterestAccruedTimestamp` | uint32 |

`Account { uint104 scaledBalance }` (`MarketState.sol:40-42`). The immutables are `borrower`, `delinquencyFeeBips`, `delinquencyGracePeriod` and `withdrawalBatchDuration` (`WildcatMarketBase.sol:48-64`).

**(j) Events.** All are in `src/interfaces/IMarketEventsAndErrors.sol`:
- `Borrow(uint256 assetAmount)` (:108)
- `DebtRepaid(address indexed from, uint256 assetAmount)` (:110)
- `StateUpdated(uint256 scaleFactor, bool isDelinquent)` (:116)
- `InterestAndFeesAccrued(fromTimestamp, toTimestamp, scaleFactor, baseInterestRay, delinquencyFeeRay, protocolFees)` (:118-125)
- `MaxTotalSupplyUpdated` (:85) and `AnnualInterestBipsUpdated` (:89)
- `MarketClosed` (:112)

**(k) Who may draw, and when.** Only the immutable `borrower`, through `onlyBorrower` (`WildcatMarketBase.sol:222-233`). The borrower cannot draw while flagged by Chainalysis (`WildcatMarket.sol:150-152`) or after the market is closed (:155). **Anyone** may repay; there is no `onlyBorrower` on `repay` (`WildcatMarket.sol:188`). A global pause for borrowing: **no verificado** (none in `borrow`; a SphereX guard exists at `WildcatMarket.sol:146`).

---

## 2. Atlendis v1 (`protocol-v1`)

Atlendis v1 is a per-borrower pool with a rate-tick order book. Borrowing sells zero-coupon "bonds" to lenders' ticks, cheapest rate first. Each pool has one loan cycle at a time, and a cycle can contain several draws.

**(a) Limit vs drawn.** `MAX_BORROWABLE_AMOUNT` (uint128) is checked against `state.normalizedBorrowedAmount + loanAmount` (`src/BorrowerPools.sol:489-491`). Governance sets it (`src/PoolsController.sol:398`). Liquidity must also exist: `loanAmount ≤ normalizedAvailableDeposits` (`BorrowerPools.sol:499-501`), and it must be fillable within the rate ticks (`:503-516`).

**(b) Multiple draws.** Yes, within one cycle. The first draw sets `currentMaturity = now + LOAN_DURATION` and emits `Borrow`. Later draws emit `FurtherBorrow` (`BorrowerPools.sol:517-522`). A draw after maturity reverts `BP_MULTIPLE_BORROW_AFTER_MATURITY` (`:482-484`).

**(c) Revolving.** Not inside a cycle. `repay()` takes **no amount** and repays all bonds (`BorrowerPools.sol:536`, the loop at `:558-571`), then resets `currentMaturity` and `normalizedBorrowedAmount` to 0 (`:610-612`). Repaid funds go back into `normalizedAvailableDeposits` (`:581`). A new cycle can start only after `nextLoanMinStart = now + COOLDOWN_PERIOD` (`:577`, enforced at `:493-495`). So the line revolves cycle by cycle, not draw by draw.

**(d) Interest model.** Interest is a discount at issue, not running accrual.
- Bond price is `1 / (1 + rate × duration / 365 days)` in WAD (`src/lib/PoolLogic.sol:59, 633-635`).
- A later draw is priced over the **remaining** time to maturity (`PoolLogic.sol:232-238`). That gives a simple-interest, pro-rata charge for late draws.
- Bonds bought are `usedAmount / price` (`PoolLogic.sol:238`). The rate is the volume-weighted average of the ticks consumed (`BorrowerPools.sol:155-194`).
- Fees: an establishment fee is taken off the drawn amount (`BorrowerPools.sol:487-488, 524`). The repayment fee is charged on interest only, `(repayAmount − borrowed) × REPAYMENT_FEE_RATE` (`PoolLogic.sol:117-125`).
- No fee on undrawn liquidity: **no verificado**. Undrawn deposits earn the yield-provider rate plus liquidity rewards that the borrower tops up (`BorrowerPools.sol:620-637`).

**(e) Due dates and delinquency.** There is a single due date: maturity, plus a `REPAYMENT_PERIOD` grace window.
- Repaying early requires `EARLY_REPAY` and is priced at the discounted time value (`BorrowerPools.sol:545-548`; `PoolLogic.sol:157-158`).
- After maturity the amount owed keeps growing as `bonds / price(now − maturity)` (`PoolLogic.sol:160-164`).
- After `maturity + REPAYMENT_PERIOD` a per-second late fee is added: `(now − lateTs) × LATE_REPAY_FEE_PER_BOND_RATE` (`PoolLogic.sol:107-115`).
- Governance may call `setDefault` only after that window (`PoolsController.sol:375-392`). Draws are blocked when the pool is defaulted or closed (`BorrowerPools.sol:476-481`) and after maturity (`:482`).

**(f) Availability, maturity.** `LOAN_DURATION` per cycle, `COOLDOWN_PERIOD` between cycles, and `REPAYMENT_PERIOD` grace (`src/lib/Types.sol:43, 45-46`). There is no global line expiry beyond governance `closePool` (`PoolsController.sol:336`).

**(g) Minimum draw.** None explicit: **no verificado**.

**(h) Repayment order.** Payment is all-or-nothing: notional plus interest plus late fee plus repayment fee, in one transfer (`BorrowerPools.sol:563-576`; the breakdown view is at `:239-253`).

**(i) State.**
- `PoolParameters` (`Types.sol:34-52`): `MAX_BORROWABLE_AMOUNT`, `LOAN_DURATION`, `COOLDOWN_PERIOD`, `REPAYMENT_PERIOD`, `LATE_REPAY_FEE_PER_BOND_RATE`, `ESTABLISHMENT_FEE_RATE`, `REPAYMENT_FEE_RATE` (all uint128), and `EARLY_REPAY` (bool).
- `PoolState` (`Types.sol:54-68`): `active`/`defaulted`/`closed` (bool); `currentMaturity`, `bondsIssuedQuantity`, `normalizedBorrowedAmount`, `normalizedAvailableDeposits`, `nextLoanMinStart` and `defaultTimestamp` (uint128).
- Amounts are normalised to WAD 1e18 regardless of token decimals (`src/lib/Scaling.sol:10-24`; `BorrowerPools.sol:486`).

**(j) Events.** All in `src/interfaces/IBorrowerPools.sol` and `src/interfaces/IPoolsController.sol`:
- `Borrow(poolHash, normalizedBorrowedAmount, establishmentFees)` (`IBorrowerPools.sol:21`)
- `FurtherBorrow(...)` (:29)
- `Repay` (:39-45), `EarlyRepay` (:55-61) and `LateRepay` (:73-80). The repay events carry `nextLoanMinStart`.
- `TickBorrow` (:134) and `TickRepay` (:188)
- `BorrowerAllowed` and `BorrowerDisallowed` (`IPoolsController.sol:23,30`)
- `Default` (`IPoolsController.sol:52`) and `SetMaxBorrowableAmount` (`IPoolsController.sol:57`)

**(k) Who/when.** Only `BORROWER_ROLE`, which governance grants. There is one pool per borrower (`PoolsController.sol:291-307`), resolved through `borrowerAuthorizedPools[msg.sender]` (`BorrowerPools.sol:474`). Borrowing requires `whenNotPaused` (`:473`), and governance controls `freezePool`/`unfreezePool` (`PoolsController.sol:210-217`).

---

## 3a. TrueFi Automated Line of Credit (`contracts-beryllium/AutomatedLineOfCredit.sol`)

The borrower owns an ERC4626 vault. Lenders deposit, and the borrower draws idle cash. Interest follows a utilisation curve.

**(a) Limit vs drawn.** `maxSize` caps **total assets on deposit**, not borrowing (`AutomatedLineOfCredit.sol:128`). The borrower can draw all idle cash minus the due fee: `assets + fee ≤ virtualTokenBalance` (`:256-257`). The drawn amount is tracked in `borrowedAmount` (`:38, :260`).

**(b) Multiple draws.** Yes. `borrow(assets)` can be repeated (`:251-265`).

**(c) Revolving.** Yes. Principal repaid adds back to `virtualTokenBalance` (`:281`) and can be drawn again.

**(d) Interest model.** Simple interest, utilisation-based, accrued per second at checkpoints.
- Utilisation: `borrowedAmount × 10000 / (borrowedAmount + virtualTokenBalance − unpaidFee)` (`:303-313`).
- Rate: a piecewise-linear curve with three kinks, `InterestRateParameters` as six uint32 values (`interfaces/IAutomatedLineOfCredit.sol:18-25`). It is evaluated in `interestRate()` (`AutomatedLineOfCredit.sol:328-361`) with `solveLinear` (`:416-424`).
- Accrual: `unincludedInterest = interestRate × borrowedAmount × (now − lastUpdateTime) / YEAR / 10000`, with floor division (`:324-326`, `YEAR = 365 days` at `:24`).
- Checkpoint: `update()` does `accruedInterest += unincludedInterest(); lastUpdateTime = now` (`:369-373`). Debt is `borrowedAmount + accruedInterest + unincludedInterest` (`:320-322`). **No compounding**: interest accrues on `borrowedAmount` only.
- Each path that changes balances calls `update()` **before** changing `borrowedAmount` or `virtualTokenBalance` (deposit `:130`, withdraw `:171`, borrow `:259`, repay `:271`). The rate computed at checkpoint time is therefore the rate that applied throughout the elapsed interval, a piecewise-constant charge.
- Protocol fee: `lastProtocolFeeRate × totalAssets × dt / YEAR / 10000`, on **all** assets including idle (`:396-414`). Lenders bear it, not the borrower. A shortfall carries forward as `unpaidFee` (`:375-389`).

**(e) Payment cycles and delinquency.** None. There is no schedule, no overdue state and no penalty rate (a verified absence in `AutomatedLineOfCredit.sol`). With no delinquency state in the contract, there is nothing that could block a draw.

**(f) Expiry.** `endDate = block.timestamp + _duration` (`:79`). After it, both `borrow` (`:254`) and deposit (`:125`) revert. **Repay has no `endDate` check** (`:267-286`). `getStatus` returns `Closed`, `Full` or `Open` (`:293-301`). Forced repayment at `endDate`: **no verificado**.

**(g) Minimum draw.** Only `assets > 0` (`:255`). Repay also requires `assets > 0` (`:272`).

**(h) Repayment order.** Interest first, then principal (`:275-280`). Overpaying is rejected: `assets ≤ borrowedAmount + accruedInterest` (`:273`). `repayInFull` asserts the debt reaches 0 (`:288-291`).

**(i) State** (`:29-46`):

| Field | Type |
|---|---|
| `endDate` | uint256 |
| `maxSize` | uint256 |
| `borrower` | address |
| `interestRateParameters` | struct of 6 × uint32 |
| `virtualTokenBalance` | uint256 |
| `borrowedAmount` | uint256 |
| `accruedInterest` | uint256 |
| `lastProtocolFeeRate` | uint256 |
| `unpaidFee` | uint256 |
| `lastUpdateTime` | uint256 (internal) |

**(j) Events.** `Borrowed(uint256)`, `Repaid(uint256)`, `FeePaid(address indexed, uint256)` and `MaxSizeChanged(uint256)` (`:52-55`). No accrual event.

**(k) Who/when.**
- Only `borrower` can draw and repay (`:253, :269`). Note that repay is restricted to the borrower here.
- Both are `whenNotPaused` (`:251, :267`), and the pauser comes from `ProtocolConfig` (`:75`).
- The borrower is the `msg.sender` of the factory's `createPortfolio`, which requires `MANAGER_ROLE` on the factory, an allow-list (`AutomatedLineOfCreditFactory.sol:31-41, 58`).
- The borrower also receives `MANAGER_ROLE` on the vault and can change `maxSize` alone (`AutomatedLineOfCredit.sol:76, 480-484`).

## 3b. TrueFi LineOfCreditAgency (`contracts-pre22/contracts/truefi2/LineOfCreditAgency.sol`)

This is a credit-scored, revolving line drawn from shared pools, with monthly interest-only payments.

**(a) Limit vs drawn.** The limit is computed, not stored. `borrowLimit = saturatingSub(poolBorrowMax(...), totalBorrowedInUsd)` (`contracts/truefi2/RateModel.sol:341-352`). `poolBorrowMax` is the minimum of an oracle `maxBorrowerLimit` scaled by score, plus staked TRU, a TVL share and a pool-value share (`RateModel.sol:364-378`). Drawn principal is `borrowed[pool][borrower]` (`LineOfCreditAgency.sol:90-91`). The draw check is `tokenToUsd(amount) ≤ borrowLimit(...)` (`:396-400`).

**(b) Multiple draws.** Yes (`:386-420`).

**(c) Revolving.** Yes. Principal repayment lowers `borrowed` (`:817-826`), which raises `borrowLimit`. If the line ends up over its limit, an `overBorrowLimitTime` timer starts (`:540-548`) and can lead to default (`:503-506`).

**(d) Interest model.** A cumulative per-share accumulator, simple interest, scaled by 1e27.
- Each credit-score bucket has a `rate`, `timestamp` (uint128), `cumulativeInterestPerShare`, `totalBorrowed` and `savedInterest[borrower] = {total, perShare}` (`:56-77`).
- Increment: `newInterestPerShare = rate × dt × (1e27 / 10000) / 365 days`, with floor division (`:626-628`). `poke` adds it to the accumulator and recomputes the bucket rate (`:609-623`).
- Borrower interest: `saved.total + borrowed × (cumulative − saved.perShare + pending) / 1e27` (`:773-782`). Interest due is that amount minus `borrowerTotalPaidInterest` (`:791-797`).
- Every principal change runs through `_rebucket`. It snapshots the interest earned so far into `savedInterest.total` and resets `perShare` to the current accumulator (`:679-692, 702-757`). This is the checkpoint.
- Rate = pool base rate (secured oracle APY + risk premium + utilisation adjustment) + credit-score adjustment, capped (`RateModel.sol:222-224, 241-273`).
- No fee on undrawn: **no verificado**.

**(e) Payment cycle and delinquency.**
- `interestRepaymentPeriod` defaults to 31 days (`:113, :193`). `nextInterestRepayTime` is set on the **first** draw only, when `currentPrincipal == 0` (`:412-414`).
- The timer resets only when a payment covers **all** accrued interest (`:446-451`). It is cleared when principal reaches 0 (`:454-456`).
- **Draws are blocked while interest is overdue**: `require(!_hasOverdueInterest)` (`:393`; definition at `:667-669`).
- Default needs the owner, and is allowed once overdue time exceeds the oracle `gracePeriod` (`:477-510`). On default the debt is converted into a `DebtToken` and the borrower is banned through the mutex (`:512-538`).

**(f) Expiry.** No line maturity in the code: **no verificado**. Lines are open-ended while the borrower stays eligible.

**(g) Minimum draw.** Only `amount > 0` (`:388`).

**(h) Repayment order.** Accrued interest first, then principal (`:432-452`). Payments larger than the debt revert (`:443`).

**(i) State.**
- `borrowed`, `borrowerTotalPaidInterest` and `nextInterestRepayTime` are uint256 mappings per pool and borrower (`:90-103`).
- `isBorrowerAllowed`: bool (`:110`).
- `overBorrowLimitTime` (`:141`).
- The bucket struct (`:63-77`).

**(j) Events.** `InterestPaid`, `PrincipalRepaid` and `EnteredDefault` (`:166-174`), and `BorrowerAllowed` (`:157`). The agency emits no draw event of its own; the pool emits `Borrow(msg.sender, amount)` (`contracts/truefi2/TrueFiPool2.sol:584`).

**(k) Who/when.** Draws need:
- the owner allow-list `onlyAllowedBorrowers` (`:197-200, 237-240`);
- oracle status `Eligible` (`:389-392`);
- `score ≥ minCreditScore` (`:395`);
- a `borrowingMutex` lock, which forbids a fixed-term loan and a line at the same time (`:401-407`).

A pause for draws: **no verificado**.

---

## 4. Aave v3 credit delegation (`aave-v3-core`)

Delegation is a limit-plus-allowance pattern. `approveDelegation(delegatee, amount)` **overwrites** `_borrowAllowances[delegator][delegatee]`; it does not add to it (`contracts/protocol/tokenization/base/DebtTokenBase.sol:22, 38-40, 84-87`). The signed variant `delegationWithSig` uses a nonce and a deadline (`:43-68`). The delegator's collateral backs the debt. The delegatee borrows with `onBehalfOf = delegator`.

On each draw, `VariableDebtToken.mint` calls `_decreaseBorrowAllowance(onBehalfOf, user, amount)` whenever `user != onBehalfOf` (`contracts/protocol/tokenization/VariableDebtToken.sol:97-99`; the same at `StableDebtToken.sol:133`). That is a checked subtraction, so an insufficient allowance reverts with a 0.8 underflow panic rather than a custom error (`DebtTokenBase.sol:95-101`). Each change emits `BorrowAllowanceDelegated(delegator, delegatee, asset, newAllowance)`. `burn` (repay) does not touch the allowance (`VariableDebtToken.sol:104-111`; `contracts/protocol/tokenization/base/ScaledBalanceTokenBase.sol:99` and onward have no allowance reference). **The allowance is therefore not revolving**: repaid principal does not restore it, and the delegator must approve again. Interest accrues through a per-reserve compounded `variableBorrowIndex` and scaled balances, `amount.rayDiv(index)` (`ScaledBalanceTokenBase.sol:66-88`; `contracts/protocol/libraries/logic/ReserveLogic.sol:308-315`; `contracts/protocol/libraries/math/MathUtils.sol:50,93`).

---

## Patterns worth copying for a Solana pool

1. **Checkpointed simple interest on the drawn balance.** Use TrueFi ALOC's form: `accrued += principal × rate_bps × (now − last_ts) / (365d × 10_000); last_ts = now`, and call it **before** any change to principal (`AutomatedLineOfCredit.sol:324-326, 369-373`, with call sites `:259, :271`). It is one u64 timestamp and one u128 accumulator, with no index, which suits a per-line PDA.
2. **Derive availability as `limit − principal_outstanding`, so repaying principal restores headroom** (`RateModel.sol:351`). Do **not** decrement a one-way allowance (the Aave behaviour, `VariableDebtToken.sol:97-111`) unless the line is meant to be non-revolving.
3. **Apply payments to interest before principal** (`AutomatedLineOfCredit.sol:275-280`; `LineOfCreditAgency.sol:446-452`). Reject overpayment, or cap it explicitly (`:273`, `:443`).
4. **Interest-period timer**: start it on the first draw, reset it only when all accrued interest is paid, clear it when principal is 0, and **block new draws while overdue** (`LineOfCreditAgency.sol:393, 412-414, 446-456, 667-669`). This maps directly to elote's installment due dates.
5. **Grace, then penalty APR, with a `time_delinquent` counter** that rises while late and falls while current (`FeeMath.sol:88-122`). It is a cleaner model than a one-off late fee.
6. **Separate the draw window from the repay window.** `endDate` blocks draws only, and repay stays open (`AutomatedLineOfCredit.sol:254` versus `:267-286`). Atlendis adds a post-maturity `REPAYMENT_PERIOD` grace before default (`PoolLogic.sol:107-115`; `PoolsController.sol:383`).
7. **Pro-rate late draws in a fixed-maturity line** by pricing interest over the remaining time to maturity (`PoolLogic.sol:232-238`). This fits elote's flat-interest model if each draw's flat interest becomes `draw × rate × (maturity − now)/year`.
8. **Use compact, typed state**: bips as u16, timestamps as u32 or i64, scaled amounts as u104/u128 (`MarketState.sol:12-38`). Put the rounding direction in the code comment (`WildcatMarketBase.sol:663-664`).
9. **Emit an accrual event carrying `from_ts`, `to_ts`, rate and amount** (`InterestAndFeesAccrued`, `IMarketEventsAndErrors.sol:118-125`), plus distinct first-draw and further-draw events (`IBorrowerPools.sol:21,29`). Indexers can then rebuild the balance without replaying the math.

## Patterns to avoid

10. **Charging interest on the whole commitment through a supply-wide scale factor** (Wildcat, `MarketState.sol:51-53`). It makes the borrower pay APR on undrawn cash. If a commitment fee is wanted, make it an explicit, separate `undrawn × fee_bps` term.
11. **All-or-nothing repayment followed by a cooldown** (Atlendis `BorrowerPools.sol:536, 577`). It is incompatible with installments and with revolving.
12. **A utilisation-based rate where the single borrower controls utilisation** (ALOC `:303-361`). It also makes the borrower's cost depend on lender deposits. Use a fixed rate per line instead.
13. **Per-bucket or per-tick loops in a hot path** (Agency `poke` `:554-569`; Atlendis tick loops `BorrowerPools.sol:505-513, 558-571`). Compute-unit cost grows with the number of buckets or ticks.
14. **Half-up rounding on borrower-owed amounts without stating its direction** (`MathUtils.sol:158, 175`). Choose one rule, for example round interest owed **up** in the pool's favour and payouts **down**, and test it.
15. **Letting the borrower raise its own limit** (ALOC `setMaxSize` under the borrower's `MANAGER_ROLE`, `:76, :480`). In elote, only the admin should change `limit`.
