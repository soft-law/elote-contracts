# CETES as the first investment product: integration review

Date: 2026-10-31. Status: review, no code. Source: owner's request of 2026-10-31 ("review how we
can integrate the first investment product, MEX CETES", pointing at the Etherfuse docs). Read
through the Etherfuse docs MCP (v2 pages cited by path), the CETES mint read live from mainnet
today, and the stablebond SDK package `@etherfuse/stablebond-sdk` 3.0.19 from npm (the GitHub
repository the docs link to, `etherfuse/stablebond`, returns "not found" today).

"MEX CETES" is read as Etherfuse's **CETES stablebond**, the tokenized Mexican treasury bill, not
the rate-zero `MEX` token (`Cetesz…DXSn`) that the 2026-10-19 rail notes found hidden in their
docs. Say so if the other was meant.

## What CETES is, from the docs and the chain

| Fact | Source |
|---|---|
| A stablebond: a tokenized, yield-bearing sovereign bond; it earns the underlying yield while held on chain. Entering one in its own currency is free, leaving one costs 5 bps; partner, wallet-setup and bridge fees may apply on top. | `v2/glossary`, `v2/overview#fees`, `v2/fees` |
| Mainnet mint `CETES7CKqqKQizuSN6iWQwmTeFRjbJR6Vw2XRKfEDR8f`: Token-2022, 6 decimals, supply 10,257,087.43 tokens today, mint authority `A6nF…tHYP`, **no freeze authority**, extensions `interestBearingConfig` (current rate 555 bps, rate authority `CB8D…HJ4A`) and `metadataPointer` only. No transfer hook, no permanent delegate, no transfer fee, no close authority. | `getAccountInfo` on mainnet, 2026-10-31 |
| The price does **not** come from the interest-bearing extension, which is for wallet display. It comes from the `BondPrice` PDA, seeds `["bond_price", mint]` under the stablebond program `BondyhA24H696Y1HudTyBGzZH58PMPCeAoSinHdWMa1f`; priced instructions take it as an account. Example in the docs: 1 CETES = 1.177336 MXN at 554 bps annualised. | `v2/price-feeds#token-costs`; SDK `dist/types/generated/accounts/bondPrice.d.ts` |
| `MEXe` is a **principal token**: it wraps CETES, holds par (1 MEXe = 1 MXN) and its `current_basis_points` is always 0 "because the coupon accrues to the bond, not to the principal-token holder". | `v2/price-feeds#token-costs` |
| Sandbox CETES mint is different (`Avvet…LW4q`, devnet) and onramp quotes are capped at 500 MXN per quote. Identifiers must be discovered, never hardcoded. | `v2/guides/testing-onramps`, `v2/guides/chains/solana` |
| Public REST pricing: `GET /lookup/tokens/cost` (per token, with per-provider FX `sources` for oracle-safety checks) and `GET /lookup/stablebonds` (price, supply per chain, cached about 5 minutes). On-chain FX (MXN/USD) via Switchboard On-Demand on Solana; the bond price itself is the `BondPrice` account. | `v2/price-feeds`, `v2/api-reference/lookup/*` |
| On-chain purchase and redemption instructions (`purchaseBondV3`, `instantBondRedemptionV2`) require a `kycAccount` and a signing `userWallet`, plus `paymentFeedAccount`, `bondPriceAccount`, `sellLiquidityAccount` (redemption) and a fee collector. KYC users are added by the program's `addKycUser`; the SDK exposes `hasKYC()`. | SDK `dist/types/generated/instructions/purchaseBondV3.d.ts:11-32`, `instantBondRedemptionV2.d.ts:11-29`, `README.md:84-90` |
| Buying through the REST API: the customer's organization must be approved (KYC); register the bank account (CLABE) and the Solana wallet as payment accounts; quote MXN → CETES (2-minute validity); create the order; the customer pays the CLABE with the numeric reference; status `created → funded → completed`; `orderUpdated` webhook. The bond is priced **at the end of the funding window**, so the customer receives slightly fewer tokens than the quote showed. | `v2/guides/testing-onramps`, `v2/overview#the-flow`, `v2/fees#fees-on-a-quote` |
| Selling: offramp CETES → MXN from the wallet to the registered CLABE; Etherfuse builds a `preparedTransaction` the customer signs (about 60 s of validity on Solana); redemptions do not depend on how much was onramped. | `v2/guides/testing-offramps` |
| `walletSponsor`: a fixed fee, taken out of the delivered tokens, when the destination Solana wallet has no SOL or no token account. Registering a wallet does not prove the customer owns it. | `v2/fees#fees-on-a-quote`, `v2/api-reference/payment-accounts/register-payment-account` |
| Partner fee: a default per organization, up to 500 bps, overridable per quote, "settled offline via finance". | `v2/fees#partner-fees`, "Set your default partner fee" |
| Every call runs under one organization id: the partner's own, or a customer organization the partner created. Accounts, quote and order must be under the same id. | `v2/overview#organizations-in-the-route` |

## Why CETES matters beyond being a rail

If a credit series holds `MEXe` as its asset, the idle vault balance earns nothing: the coupon
accrues to the bond, not to the principal-token holder. CETES is where the peso yield lives. So
"CETES as an investment product" is a different thing from the pool: it is a savings product with
sovereign yield, sold next to the credit series, not inside them.

## Three shapes, and which one

| | A. Distribution: Elote as partner | B. Idle-liquidity yield inside a credit series | C. A CETES-denominated series |
|---|---|---|---|
| What the investor holds | CETES in their own wallet, bought through Elote's app | Series shares; the pool holds MEXe for lending and CETES for the idle part | Series shares over a vault of CETES |
| Program change | **None.** elote_pool is not involved. | Two assets in one pool, NAV priced from `BondPrice`, swap legs, a KYC PDA for the pool signer | Pool v2 with a Token-2022 asset whose unit appreciates against the peso |
| Who issues | Etherfuse issues the bond token; Elote distributes | Elote's series, with a yield component from Etherfuse | Elote's series |
| Identity | Etherfuse's own KYC on the investor's personal org is the gate; Elote runs no KYC of its own for a CETES-only buyer (owner review, decision 2). The Sumsub share token helps only an investor who already passed Elote's KYC for a credit series with Sumsub | The pool PDA would have to be an Etherfuse KYC user; whether a PDA can be one is **no verificado** | As A, plus the series gate |
| Revenue | Partner fee in bps on each order, settled offline | Yield on idle balance accrues to the series NAV | Spread, if any |
| Fit | Lightest: no custody by Elote, no new securities issued by Elote, the investor can sell any time at 5 bps | Real but second-order: idle balances in a small series are small, and it depends on pool v2 | None: borrowers want pesos, a loan denominated in an appreciating unit makes no sense, and the series-shelf plan already keeps CETES out as a pool asset (unit mismatch) |
| Verdict | **Do this first** | Design note after pool v2 | Drop |

### A in practice

1. The investor's Etherfuse personal organization is created by Elote's backend and the investor
   is launched into Etherfuse's `/idv`; that verification is the gate, and Elote runs none of its
   own for a CETES-only buyer. Only an investor who already passed Elote's credit-series KYC with
   Sumsub can skip the second capture through the share-token import
   (`v2/guides/kyc-sumsub-shared-token`, CURP sent on the import call).
2. Register the investor's CLABE (`registrationInfo` is filled from the verified identity; the
   RFC must match) and their Solana wallet. Read the rails until both directions are `ready`;
   `minAmount` and `maxAmount` appear there when known.
3. Buy: quote MXN → CETES, order, show the CLABE and numeric reference, wait for `orderUpdated`.
   Tokens land in the investor's wallet; the ATA is created by Etherfuse against the
   `walletSponsor` fee if missing.
4. Sell: quote CETES → MXN, order, have the investor sign the prepared transaction in the app,
   wait for the payout. 5 bps plus the partner fee.
5. Show the position from `GET /lookup/tokens/cost` or the `BondPrice` account, never from the
   interest-bearing extension's display amount.

Backend pieces: organizations and members, payment accounts, quotes, orders, two webhooks
(`kycUpdated`, `orderUpdated`), the partner fee setting. The app: a buy screen, a sell screen with
a transaction to sign, a position screen. Nothing in `elote_pool`, nothing in the series shelf.

### B, parked

Holding CETES inside a credit series needs: the pool to hold a second asset and to price it from
`BondPrice` in the NAV (the series-shelf plan's "interest-bearing is allowed only for display"
would be reversed for this one mint); a swap leg MEXe ↔ CETES per draw and per redemption, each
with a 2-minute quote and a prepared transaction signed by the pool PDA, which the pool cannot do
without a program-side CPI into the stablebond program; and the pool PDA registered as an Etherfuse
KYC user for `purchaseBondV3` / `instantBondRedemptionV2`. The on-chain instructions exist and
need no REST call, but whether Etherfuse would add a program-derived address as a KYC user is
**no verificado**. Revisit after pool v2, with real numbers on idle balances.

## Legal and tax, for counsel

- Who is the issuer of record of the bond token, and what Elote is when it distributes it
  (comisionista, promotor, or an intermediary that needs a licence) **no verificado**.
- Withholding on the yield: CETES interest to individuals carries ISR withholding at the
  financial institution in the traditional rail; who withholds on tokenized CETES bought through
  a partner, and whether price appreciation is treated as interest, **no verificado**. This is the
  same question the identity review raised for series distributions, with a different payer.
- Disclosure: the price at the end of the funding window, the 5 bps exit, the partner fee and the
  `walletSponsor` fee all need to be in the app before the order, per the fee itemisation the
  quote already returns.

## Unverified, in one place

- `GET /supported_assets?chain=solana` with a real sandbox and production key: still pending since
  2026-10-19. It decides whether CETES is actually deliverable to a Solana wallet in production
  and what MEXe looks like there.
- Whether a PDA may be a registered destination wallet and receive a delivery.
- Rail `minAmount` / `maxAmount` for SPEI onramps, visible only on a registered payment account.
- Partner-fee settlement terms (the docs say "offline via finance").
- The stablebond program's own source (the linked GitHub repository is not public today); the
  SDK's generated instruction definitions are the only public view of its accounts.

## Next steps

1. Create the Etherfuse sandbox business organization and API key (owner; the sandbox KYB is
   self-approved). Put the key in `poc/.env` as `ETHERFUSE_SANDBOX_API_KEY`, never in the repo.
2. Run `npm run etherfuse` in `poc/`: it lists the Solana assets, registers a dummy CLABE and a
   devnet wallet, quotes and orders 100 MXN of CETES, simulates the deposit and waits for
   `completed`. That closes the supported-assets question and measures the flow once.
3. Decide A with the partner fee level, and put the counsel questions above on the same list as
   the identity review's.
