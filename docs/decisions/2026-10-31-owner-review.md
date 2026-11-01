# Owner review, 2026-10-31

Status: decided. Scope: everything produced for Elote between 2026-10-30 and 2026-10-31, reviewed
with the advisor model and closed by the acting owner (Claude, on Mario's delegation of
2026-10-31: "you are the new owner, review everything with advisor"). Where a decision needs a
human's hands or a licence, it says so and names the person.

## What was reviewed

| Artefact | Where |
|---|---|
| KYC/KYB and on-chain identity evaluation (verdict, scorecard, borrower KYB, permissionless scenario, counsel table) | https://claude.ai/artifact/VGUzRWXes1DYau2x1zQA7z (private; share before sending) |
| Series-shelf contract plan (pool v2, Token-2022, Token ACL) | `docs/plans/2026-10-24-series-shelf-contract-plan.md` |
| Tranched credit lines plan (line v3) | `docs/plans/2026-10-31-multi-draw-plan.md` |
| CETES investment product review | `docs/plans/2026-10-31-cetes-investment-product.md` |
| Credit-line surveys of eight protocols | `docs/research/2026-10-31-credit-lines-*.md` |
| Proofs of concept: Sumsub, Didit, SAS (ran on devnet), SNS, Etherfuse CETES (unrun) | `poc/` and `poc/README.md` |
| Program architecture reference | `ARCHITECTURE.md` (582 lines, never committed before today) |

## Decisions

| # | Item | Decision | Rationale or trigger | Lands in |
|---|---|---|---|---|
| 1 | KYC/KYB vendor | **Didit** for borrower KYB and for investors of the credit series, starting now. **Sumsub** replaces it for investors only if two facts arrive together: Etherfuse confirms in writing which share service it imports and its `forClientId`, and the Sumsub Custom quote (KYB, KYT, CURP/RFC) is acceptable. | SME borrowers get no Etherfuse reuse with either vendor; Didit's KYB is priced ($2 to $9) and its sandbox is free. The share-token advantage is investor-side and depends on two unverified facts. | identity artifact, softlaw-backend |
| 2 | KYC for the CETES product | **None of Elote's own.** Etherfuse's personal-organization KYC is the gate; Elote only creates the organization and launches `/idv`. | Etherfuse is the issuer and the regulated counterparty; a second KYC adds drop-off and no control. | CETES review, shape A |
| 3 | On-chain identity | **SAS attestations under Elote's own credential** (Squads vault authority, rotatable backend signer), schema `jurisdiction` + `verified_at`, no PII. **Thaw gate: Token ACL with the ABL gate in Allow mode**, fed by the backend. The SAS-reading gate is a post-audit upgrade. | Verified 2026-10-30: Token ACL mainnet address and Accretion audit; ABL gate two audits; Moonsong gate says not for production. | series-shelf plan 2c, identity artifact |
| 4 | SNS | **Dropped from pilot scope.** If a display name is ever wanted, `.sns` only, and only when `stale == false`. | SDK v4 refuses `.sol` on mainnet; no product needs it. | identity artifact |
| 5 | Investor posture | **A, private placement**, for every series until counsel clears B. C never ships to mainnet. | Withholding needs the holder's RFC; LMV and Ley Fintech questions each force identity back in. | identity artifact, series-shelf plan |
| 6 | Third-party repayments | **Allowed** (Mario, 2026-10-31). Control: screen the payer of every `LineRepaid` against sanctions lists. | Sponsors, guarantors and relatives pay in practice. | ops runbook |
| 7 | Sponsor verification | **KYC if a natural person, KYB if an entity** (Mario, 2026-10-31), same flow as borrowers. | A co-signer on the line is a party to it. | identity artifact |
| 8 | Credit instruments | **Tranched, non-revolving lines**; revolving **deferred**. | Mario, 2026-10-31. | tranched plan |
| 9 | Interest model | **(a) Interés ordinario per disposición**: annual rate on each tranche from its draw to the last due date, fixed at the draw, split across installments. Interés moratorio stays a separate penalty. `rate_bps` becomes annual on version-3 lines. | Mario's choice relayed by theOffice on 2026-10-31, now owner-confirmed. (b), interest on the saldo insoluto, stays a possible follow-up. | tranched plan D1, Q1 |
| 10 | Draw window | **Closes at the first due date** on installment lines, at maturity on bullet lines, strictly before the second. | Keeps the installment math exact; Mario's "due date finish all". | tranched plan D3 |
| 11 | Late interest basis | **Overdue principal only** on every scheduled line, v2 included, since `math::arrears` is shared; in the same batch, with the v2 vector tests regenerated. | Charging interest on overdue interest is capitalisation and needs express agreement in Mexico; cheaper to fix pre-mainnet than to defend. | tranched plan D11, `math::arrears` |
| 12 | Moratorio granularity | **Per second** (the continuous form of a daily rule); the template describes it as daily. | Strictly lower than any whole-period rounding; no math change. | tranched plan Q7 |
| 13 | Documents per draw | **Contrato at opening + one pagaré per disposición**; `draw` takes the signed pagaré's hash and emits it; only the master hash is stored. | Mario's document set via theOffice. Custody of the electronic título is an agreements-engine item, see counsel list. | tranched plan D9, Q8 |
| 14 | Other tranched defaults | Repaid-in-full forfeits the remainder; `LineOpened.interest_due` reports 0; no reservation of undrawn commitments; devnet redeployed with pool v2; admin `suspend_draws`. | Plan defaults, now decided. | tranched plan Q2 to Q5, D10 |
| 15 | Series-shelf open questions | Permissioned from the first mainnet pool: **yes**. Gate: **ABL**. Permanent delegate: **no**. Asset freeze: **accept and record** (MEXe has one, CETES none). Squads: **one multisig, one vault per series**. | See the plan's updated list. | series-shelf plan |
| 16 | CETES product | **Shape A, distribution as Etherfuse partner** into investors' own wallets; no program change. B (idle yield in a series) parked until pool v2; C dropped. Placeholder partner fee for the sandbox: **50 bps**, a placeholder until the business sets it. | Lightest footprint, where peso yield actually lives. | CETES review |
| 17 | Repository | **Commit** `ARCHITECTURE.md`, `docs/`, `poc/` on branch `poc/kyc-identity-eval`, local only, no push. | `ARCHITECTURE.md` had no history; the plans are the working record. Pushing is Mario's call. | this commit |
| 18 | Peso stablecoin | **MEXe stays the candidate pool asset**, pending the production `supported_assets` check. The loans-gov-mexico brief prefers a peso stablecoin with SPEI redemption and names MXNB; it is evaluated in the same check, and public money (a public payer of interest, for example) moves only in MXN through SPEI and bank accounts. | Unchanged since 2026-10-19; the sandbox PoC closes the check. | series-shelf plan |
| 19 | Sponsor roles | **Two roles on the line: promoter and guarantor.** A public sponsor is a promoter only (referral, training, data, no payment obligation); a guarantor carries full liability and is private, or development banking through a partner SOFOM. | loans-gov-mexico `research-brief-tokenized-securities.md` Finding 2: under the LDF an aval, a public first-loss reserve, a revenue pledge or vehicle control makes a government a debtor. Co-signers with halved liability cut repayment by about a fifth in the RCT evidence. | tranched plan D12; Rosarito gets a no-liability collaboration agreement |
| 20 | Concentration cap | **`max_line_bps` at `init_pool`, default 1,000 bps of NAV per line**, enforced at `open_line`. | Nafin caps exposure at 30% per entity and 10% per natural person; the register's P1-1 already flags 100%-of-vault lines. | Phase 1 hardening, tranched plan D13 |
| 21 | Posture A narrowed | Private placement means **institutional or qualified investors under LMV art. 8 fracc. I, and nothing else**: the series documents are private and Elote seeks no CNBV authorisation or prudential supervision for the offering (Mario, 2026-10-31, via theOffice; the emisora simplificada route is noted for completeness only). Precision from the legal brief of 2026-11-01 (C26): "sin CNBV" cannot be read literally if the vehicle is a SOFOM E.N.R., which needs CONDUSEF registration, a CNBV PLD dictamen and PLD/FT supervision; an SPV falls under LFPIORPI art. 17 fr. IV. The fewer-than-100-persons route of fracc. II applies to shares of capital stock only, not to a debt-like participation. | loans-gov-mexico `research-brief-tokenized-securities.md` Finding 3 (textual reading, counsel to confirm). Corrects the identity page's counsel table, republished today. | identity artifact, counsel item 2 |
| 22 | Loss assumptions for series documents | Base PD 12 to 20% a year, LGD 80 to 90%, expected loss 10 to 18%; stress PD 35%, LGD 95%. No initial grace period; weekly, quincenal or monthly cadence as offered; postponement vouchers as a later parameter. | loans-gov-mexico `M-pagares-recovery.md` §3.7 and `sr/synthesis.md`; first-loss tranches in Goldfinch and Maple were exhausted by off-chain conduct. | series term sheets, tranched plan evidence section |
| 24 | Pagaré token | **The token is the pagaré** (Mario, 2026-10-31: Elote's legal position and innovation). One Metaplex Core asset per disposición, issued by the pool at `draw` with the attestor's signature, held frozen in the pool PDA as the series vehicle's custody, burned against full payment (art. 129), endorsed only through the program to wallets with Elote's SAS attestation, with the endoso log in the asset's `AppData`. The sistema de información is Solana plus the program plus the agreements engine, designated in the pagaré text. Only hashes, salted commitments, amounts, dates and keys on chain; `expediente_root` on the line. Token-2022, Light and a token-less record rejected for the título. | Centrifuge and Tinlake proved the locked document token; Huma, Tinlake's unlock and Credix's unsalted hashes show what to avoid. The pagaré research reads the LGTOC more narrowly; its objections are the items counsel must defend (item 16). `docs/plans/2026-10-31-expediente-token-design.md`. | tranched plan D9, D14, D15; counsel item 16 |
| 26 | Legal brief, 2026-11-01 | **Posture moves from H1 to H2, pending Mario's word:** the electronic pagaré is the signed data message issued through the sistema de información (program + agreements engine); the Core asset is its único registro de emisión, tenencia y circulación. H1 (the asset alone is the título) is not sustained (LGTOC 14, 170). Mechanics adopted: cancellation is an act of the holder with a receipt (`cancel_pagare(k, receipt_hash)` signed by holder and attestor, sidecar kept as the persistent PAGADO mark, burn last); double signature recorded at `draw` (`cert_fingerprint`, `signature_method`); neutral asset and collection names; the new attestor co-signs `set_attestor`; control must pass with an endoso en propiedad (C9), technical verification ordered. C19/C20 decided in 28: one pagaré per draw with partial payments and acceleration at the third missed installment. | loans-gov-mexico draft brief; mapping in `docs/plans/2026-11-01-pagare-token-legal-conditions.md`. | expediente design, dev brief, counsel brief |
| 27 | Control after the endoso (C9), 2026-11-01 | **A, decided by Mario:** "los endosos transfieren la propiedad del pagaré y se lleva el historial de endosos, así lo trackeas de dónde viene, pero un pagaré es independiente, vive por sí mismo." At the first endoso the program thaws the asset and relinquishes every permanent delegate it holds (revoke, then approve `None`; irreversible), so the vehicle keeps no power to move, freeze or burn an endorsed pagaré. Consequences accepted: no court-ordered transfer on chain (a court orders the holder, as with paper); later endosos may happen as plain Core transfers outside the program, so the SAS gate (C28) and the signed endoso log (C5) bind the first endoso and any later one routed through `endorse_pagare`, and the contract text binds the rest; the sidecar's holder of record follows Core's owner by a fixed-offset read; `cancel_pagare` on an endorsed pagaré burns as the owner with the attestor co-signing and the sidecar as the PAGADO mark; after the first endoso the asset can never be frozen again; `AddBlocker` goes into the creation set. What Core does not let the vehicle give up: the update authority through the collection (it can still add or remove adapters) and, as things stand, the AppData data authority. Whether the endoso log can travel with the owner (adapter with data authority `Owner`) is being pinned; if not, the on-chain history is Core's ownership record plus the signed log for program-routed endosos. B (retain the delegates, bound by instructions) is the discarded alternative. Residual risk: if counsel finds the retained collection authority fatal to the endoso en propiedad, the only Core escape is burn and re-mint outside the collection, which breaks one asset per pagaré. **Amendment 2026-11-02 (research supplement):** the registry is a program-owned, append-only hash chain in the sidecar, not a holder-owned adapter (payment and court entries cannot depend on the holder; rewriting is alteration of the title); holder endosos are signature-verified entries with no attestor or gate; the procurador's burn power is routed through a program PDA; `court_annotation` and late entries replace the lost `court_order_transfer`; en propiedad code gated on C32. A itself stands. | Mario, 2026-11-01; dev commits `1024104`, `5092994` (`tests/core_capabilities.rs`) | legal-conditions mapping C9, dev brief (6d spec), counsel brief Q7 reworded |
| 28 | Pagaré shape and acceleration (C19/C20), 2026-11-01 | **Decided by Mario: one pagaré per draw, with partial payments (the installments) and a vencimiento anticipado clause that fires at the third missed installment** ("pagaré vencimiento anticipado en pagarés con pagos parciales a la 3a parcialidad incumplida"). Program mapping: the schedule's `max_missed` already exists and the templates fix it at 3; `mark_in_collection` (anyone, after the third missed installment plus grace) becomes the on-chain record of the acceleration and `LineInCollection` its event; from that moment the whole unpaid principal is exigible, so late interest should run on all remaining principal from the mark, not only on the missed installments (a change to the arrears math and the vectors; accepted as PR 2 step 7: `accelerated_at` on the line, all installments due after the mark). Working rule on ordinary interest, pending Mario's and counsel's confirmation: at the mark the borrower owes the outstanding principal, the ordinary interest accrued to that date (the installments due under the calendar) and moratory interest from then on; the unaccrued ordinary interest is forgiven and the forgiven amount is emitted. Per-installment pagarés are discarded (cost and dozens of Core CPIs per draw). Art. 130 annotation of partial payments (C20): the on-chain record of each payment is the `Repaid` event plus the engine's signed receipt that references it; no new instruction unless counsel requires the receipt hash on the título itself. Fits 1a./J. 85/2011 (pagaré with amortisations is not "a la vista") provided the clause is express; counsel confirms the acceleration of an endorsed pagaré (reg. 913247 concerns series, not a single title) and who declares it. | Mario, 2026-11-01; research brief §5-§6 (C19, C20, 85/2011 vs 194/2005) | legal-conditions mapping C19/C20, dev brief (impact assessment), templates via theOffice, counsel brief Q6 |
| 29 | Cash flows after an endoso (C32), 2026-11-02 | **Pilot: procuración only; no endoso en propiedad by the vehicle in v1.** Collection agencies receive endosos en procuración (asset stays with the pool, registry entry, program-held burn key, revocable by the pool); the merchant always pays inside the app by the same path with one receipt and the same interest split, whatever happens to the title; no holder names or addresses in payment flows; legally required notices live in Avisos and the expediente. First extension, designed later: whole-line sale with repay redirection, one NAV step event and an "Administrada por" row. Per-pagaré sale rejected for any visible version. Program consequence: 6d v1 = registry hash chain, procuración, revocation, court annotation, payment entries, cancellation by the procurador; the thaw-and-relinquish path and holder-initiated entries are specified but not built in v1; the creation set stays A-compatible so later versions endorse en propiedad without re-minting. | theOffice design note `theOffice/docs/product/2026-11-02-elote-endoso-cashflow-design-note.md`; research C32 (Phase 6) | 6d scope in the dev brief, counsel brief pregunta 15 (Q9, C32), templates |
| 25 | The signed document is the source | **Everything on chain is born from the signed document** (Mario, 2026-10-31, answer "2"). The payment calendar is fixed in the credit agreement: `open_line` takes `first_due_at` from the contract and installments count from it, not from the first draw; the draw window is known at signing; a late drawer pays on the agreed dates. The attestor co-signs `open_line`, `draw` and `endorse_pagare`, tying every on-chain term to its signed document. `cancel_line` closes lines never drawn by their first due date. The minimum-days question from the UX review is closed as unnecessary. | Matches how Mexican credit is signed; removes the late-drawer trap; one anchor change in the program if done before the window tests. | tranched plan D3, D16; expediente design "Opening"; dev brief |
| 23 | Etherfuse on-chain routes | **Shape A stays REST-only.** The SDK has disabled every user buy and redeem instruction since 3.0.13 ("contact Etherfuse for OTC"); a PDA may hold CETES and may fund an off-ramp by transfer, which keeps shape B possible later through a KYB'd entity, not through on-chain CPIs. | loans-gov-mexico `N-guarantees-etherfuse-liquidity.md` "Corrections" and §B6, §B8; verified in the 3.0.19 bundle. | CETES review |

## Corrections made today

- The series-shelf plan said the ABL gate has "Allow, Block, Composite and AllowAllEoas" modes;
  the code has Allow and Block only. Fixed, with the audit facts and the Chinampa reasoning
  restated for Allow mode.
- The tranched plan charged each tranche to `term_end`; on a scheduled line the last payment is
  due at `schedule_end`, which can be earlier. Fixed in D1 and test 2.
- The scorecard's third row (open series) was recomputed by hand: 3.35 / 3.85 / 2.35 / 1.15.
- The identity artifact's verdict now says the Sumsub reuse covers investors and sole traders
  only; SME borrowers do Etherfuse's KYB with either vendor.

## Consolidated questions for licensed counsel

One list, to send once. The legal agent in theOffice has given drafting guidance on 6; the rest
are new to it.

1. Elote's status under the Ley Fintech: VASP, ITF, or neither; what that implies for KYT, the
   Travel Rule, and the virtual assets an ITF may operate with.
2. Series shares as valores under the LMV: which private-offering exemption posture A relies on,
   and what an open series (posture B) would be.
3. Withholding: per-holder ISR on series distributions (RFC, CFDI de retenciones), and who
   withholds on tokenized CETES yield bought through a partner.
4. Elote's role when distributing CETES: comisionista, promotor, or an intermediary that needs a
   licence; issuer of record.
5. Qualified-investor tests vs the provider's accredited-investor steps.
6. Credit documents: apertura de crédito simple con disposiciones parciales (article cites and
   the express non-revolving clause); one pagaré per disposición and the custody and conservation
   of an electronic título; CAT and total-cost disclosure with variable draw dates and the SPEI
   fee; day count (the program uses a 365-day year); late interest on overdue principal only
   (decided) and its disclosure; sponsor guarantee form (aval, fiador, obligado solidario);
   no-subrogation clause for third-party repayment; notice and cure before collection; KYB
   expectations for a public-sector sponsor.
7. Data-sharing agreement with Etherfuse if Sumsub share tokens are used.
8. Personal data: provider processing agreements, and confirmation that an on-chain attestation
   holding a country code and a timestamp is not personal data under the LFPDPPP.

Added 2026-10-31 from the legal agent's annex to the counsel brief (theOffice
`docs/product/2026-10-31-elote-counsel-brief.md`), so the two records agree:

9. Whether the lending activity itself is regulated: captación irregular under the LIC, SOFOM or
   habitual-lender status, CONDUSEF, LFPIORPI vulnerable activity, usury limits, and ISR/IVA on
   Elote's own fees.
10. Electronic signatures and the on-chain hash as a mensaje de datos under the Código de
    Comercio, NOM-151 conservation, and whether the admin powers the program holds
    (`suspend_draws`, `write_off`, freeze through Token ACL, attestation revocation) are
    consistent with investors' and borrowers' contractual rights.
11. The Etherfuse partner contract itself: contracting party, liability allocation, and the real
    fee schedule behind the partner fee.
12. Legal capacity of a public-sector sponsor to give an aval or any other guarantee form.

Added from the loans-gov-mexico research (2026-10-31):

13. Ley de Disciplina Financiera: confirm that a municipal or state sponsor that only refers,
    trains and shares data, with no aval, guarantee, first-loss reserve, revenue pledge or
    vehicle control, is outside the LDF (research reading, no SHCP criterion), and draft the
    no-liability collaboration agreement with Rosarito accordingly.
14. LMV art. 8: confirm that a private placement under fracc. I (institutional or qualified
    investors) suffices for a tokenised participation in a loan book **without any CNBV route**,
    since the series documents are private and Elote seeks no authorisation, registration or
    trámite; and whether checking eligibility at every transfer, not only at entry, is required.
15. Electronic pagaré specifics beyond item 6: whether a wallet signature anchored on chain plus a
    PSC NOM-151 constancia satisfies CCom art. 97 and LGTOC art. 170 fr. VI without SAT e.firma;
    whether the contractual designation of the Softlaw system as the "sistema de información"
    (LGTOC arts. 17, 27) is enough while no CNBV rules exist; the art. 5o Bis gate in a juicio
    ejecutivo; and which persona (SOFOM, fiduciario or SPV) is the tenedor, since a PDA cannot be.
16. Pagaré token, Elote's position to defend (H2, decision 26): the electronic pagaré is the
    signed data message issued through the sistema de información (program plus agreements engine,
    designated in the pagaré text); the Core asset is its único registro de emisión, tenencia y
    circulación. Counsel confirms or corrects: whether H2 holds and whether art. 5o paras. 2-3 is
    the "express" permission art. 5o Bis asks for (Q2); the designation under arts. 17 and 27
    without an act of authority (Q3); the signature method so the borrower's signature is
    attributable (CCom art. 97, tesis 2031391), the double signature of C6a, and whether the
    attestor plus a NOM-151 constancia suffice (Q1); that the series vehicle holding through the
    pool PDA is the tenedor and how a judge verifies the last holder (art. 39) from the endoso
    log; cancellation by the holder against a receipt with the asset burnt last as the art. 129
    cancellation (Q8); post-maturity endoso effects and notice; the art. 5o Bis gate in a juicio
    ejecutivo. **Q7, control after the endoso (decision 27):** Core lets the pool either keep its
    permanent delegates on an endorsed pagaré and bound their use by instruction (B), or give them
    up irreversibly at the endoso (A), in which case no return from procuración, court-ordered
    transfer or gated endoso is possible on chain. Counsel says which reading satisfies "the
    endorsee holds free of the vehicle's control" and whether a documented, instruction-bounded
    delegate held by the vehicle defeats the endoso en propiedad
    (`docs/plans/2026-11-01-pagare-token-legal-conditions.md`, C9).

Decided facts the brief may state for its scope section (from this record and `ARCHITECTURE.md`
§8): the design is non-custodial throughout, since investors hold series shares and CETES in
their own wallets, the pool vault is a program-derived account from which the admin cannot
withdraw, and the admin is a Squads vault; Etherfuse is the counterparty of every onramp,
offramp and swap, and MEXe is the candidate pool asset pending the production `supported_assets`
check; remittances and a swap product are outside this record and undecided.

## Human actions that remain

| Who | Action |
|---|---|
| Mario | Create the Etherfuse sandbox business organization and API key; put it in `poc/.env`; run `npm run etherfuse`. This closes the Solana `supported_assets` check open since 2026-10-19. |
| Mario | Didit sandbox API key and workflow id; Sumsub sandbox app token only if decision 1's trigger fires. A private mainnet RPC URL for the SNS primary-domain lookup is no longer needed (decision 4). |
| Mario | Ask Etherfuse which share service it imports and its `forClientId`; ask Sumsub for the Custom quote. Both feed decision 1. |
| Mario | Engage licensed counsel with the list above; set the partner fee with Etherfuse. |
| Mario | Decide whether to push `poc/kyc-identity-eval` and open the PR. |
| theOffice | Keep the design on (a); pagaré signing ceremony as described; separate "Interés" and "Interés por atraso" rows. |
| agreements engine (softlaw-backend) | Electronic pagaré storage and conservation; template variant per the legal agent; `LineDrawState` indexing. |

## Implementation order

1. **Phase 1 hardening** on a fresh branch from `main`: the register items the series-shelf plan
   already lists (R-11, R-21/R-27, R-24, R-25, decimals-derived `MIN_FIRST_DEPOSIT`, reject
   `borrower == admin`). Small, no layout change. This is the first code step.
2. **Pool v2 + line v3, one breaking batch**: Token-2022 interface, asset-mint policy with
   `asset_freezable` recorded, admin-supplied share mint under Token ACL, `permissioned` flag,
   dead-shares thaw, `NavUpdated`; tranched lines per the plan (D1 to D11) with late interest on
   overdue principal only; tests; Codama regen; docs. Then the external review and the devnet
   redeploy.
3. **In parallel, no program change**: CETES distribution (backend: organizations, payment
   accounts, quotes, orders, two webhooks; app: buy, sell, position) and the identity
   integration in softlaw-backend (Didit webhooks, SAS credential and schema on devnet then
   mainnet, ABL list operations).
4. **Mainnet** after the review, with the first CR series permissioned and the ops runbook for
   revocation (close attestation, remove from the list, freeze accounts) in place.
