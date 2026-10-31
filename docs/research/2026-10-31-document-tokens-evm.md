# How EVM credit protocols tokenize the loan or the receivable (for elote_pool)

Date: 2026-10-31. Purpose: decide how elote_pool should model a token that stands for the signed loan paper (credit agreement, per-draw pagarés, expediente) and is locked in the pool, with no personal data on chain.

Conventions:
- Citations are `<clone dir>/<path>:<lines>`. All clones are under `scratchpad/repos-rwa/`. Line numbers belong to the commit listed at the top of each section.
- **no verificado** marks anything that neither the code nor the docs in the clone confirm.
- Nothing in the clones was built or run. They were only read.

---

## 1. Centrifuge Tinlake (and the legacy Centrifuge document NFTs it was built for)

Repos and commits:
- `tinlake` @ 4584b0d (2022-12-29)
- `tinlake-title` (the `Title` submodule)
- `privacy-enabled-erc721` (2019)
- `centrifuge-ethereum-contracts` @ e595582 (2020-06-12)

The last two are not in the task list, but they answer its question about the NFT registry and the document hash. Tinlake's own `Title` is a bare counter ERC-721 with no metadata.

Tinlake uses two tokens:
- **The asset NFT.** Any ERC-721 registry, for example Centrifuge's `InvoiceUnpaidNFT`. This is the "collateral" that gets locked.
- **The loan token (`Title`).** An ERC-721 minted per loan. Its holder is the only one who can borrow, repay or unlock (`tinlake-title/src/title.sol:17-29,45`).

**(a) What the token represents.**
- The asset NFT stands for one off-chain document, for example an invoice whose status is "unpaid".
- The `Title` NFT stands for the borrower position on that loan.
- Legal title to the paper sits with the pool's SPV under off-chain agreements: **no verificado** (no legal docs in the repo). The README only says "tokenized non-fungible real-world assets as collateral" (`tinlake/README.md:5`).

**(b) Standard and custody.**
- Both tokens are ERC-721.
- `Shelf.issue` records `(registry, tokenId)` and mints a `Title` to the caller (`tinlake/src/borrower/shelf.sol:134-145`). The comment there says "everyone could add an nft". There is no registry whitelist.
- `lock` moves the asset NFT into the Shelf contract (`shelf.sol:263-269`).
- `nftLocked` means "the Shelf owns it" (`shelf.sol:291-293`). `borrow`, `withdraw` and `repay` require it (`shelf.sol:171,200,213`).

**(c) Metadata and privacy.** This is the legacy Centrifuge pattern, and it is the strongest match for the owner's idea.
- An off-chain document's merkle root is anchored on chain. `AnchorRepository.commit(anchorIdPreImage, documentRoot, proof)` stores only the root (`centrifuge-ethereum-contracts/contracts/AnchorRepository.sol:82-130`).
- `InvoiceUnpaidNFT.mint` takes selected plaintext fields with salts and merkle proofs (`contracts/erc721/InvoiceUnpaidNFT.sol:154-286`). It requires:
  - a proof that `invoice.status == "unpaid"` (179-192)
  - that the sender is a registered identity (195-201) and signed the document (205-214)
  - that the document is the **latest version**, meaning no `next_version` anchor exists yet (217-222; `UserMintableERC721.sol:320-350`)
  - that the document itself lists this registry and tokenId, so there is **one token per document** (225-230; `UserMintableERC721.sol:361-386`)
  - that the document grants a **read role to this NFT**, so the token holder may read the off-chain document (233-257; `UserMintableERC721.sol:398-437,491-526`)
- Only gross amount, currency, due date, sender, the anchor ids and the document root are stored on chain (`InvoiceUnpaidNFT.sol:17-25,271-279`).
- Every leaf is salted (`sha256(property, value, salt)`). Undisclosed fields cannot be guessed from the root.
- `tokenURI` is `base + registry + tokenId` (`privacy-enabled-erc721/src/nft.sol:139-141`). It is a pointer to a permissioned API, not to content.
- Privacy measures: hash-only anchor, salted field-level selective disclosure, and read access bound to the NFT inside the document's own access rules.

**(d) Lifecycle.**
- Origination: `issue` (`shelf.sol:134`), then `lock` (`shelf.sol:263`).
- Draw: `borrow` (`shelf.sol:170-193`), then `withdraw` (`shelf.sol:199-207`).
- Partial and full repayment: `repay`, capped at the debt (`shelf.sol:212-236`).
- Release: `unlock` returns the NFT when `debt == 0` **or** the loan is written off to zero PV (`shelf.sol:274-287`; `zeroPV` at `navfeed.sol:692`). The borrower also gets the NFT back after a 100% write-off.
- Close: `close` burns the `Title` and frees the NFT lookup (`shelf.sol:149-161`; `title.sol:27-29`).
- Default:
  - `writeOff` is permissionless once overdue, by write-off group (`tinlake/src/borrower/feed/navfeed.sol:419-438`).
  - `overrideWriteOff` is auth (`navfeed.sol:444-457`), reached through the pool admin at `level2` (`tinlake/src/lender/admin/pool.sol:155-158`).
  - `recover` lets a collector pay and zero the debt (`shelf.sol:243-258`).
  - `claim` sends the NFT to a collector (`shelf.sol:299-302`).
- Secondary transfer: the `Title` ERC-721 is transferable, and that moves borrower rights. The asset NFT stays in the Shelf.

**(e) Access control.**
- Anyone may `issue` and `lock`. Borrow, repay and unlock are gated by the `Title` owner (`title.sol:45`).
- Valuation, risk, maturity, recover and claim are `auth` (wards) (`navfeed.sol:241,595,605`; `shelf.sol:243,299`).
- Debt accounting in `Pile` is `auth` only: `incDebt`, `decDebt`, `setRate`, `changeRate` (`tinlake/src/borrower/pile.sol:84,100,139,150`).
- In the legacy registry, anyone holding valid proofs may mint (`privacy-enabled-erc721/README.md`, "The mint method").

**(f) Valuation.**
- The asset NFT has an oracle-set appraisal `nftValues` and a risk group (`navfeed.sol:37-42,595-618`).
- The borrow ceiling is `nftValue × ceilingRatio(risk)`. A credit-line variant subtracts current debt (`tinlake/src/borrower/feed/creditline.sol:10-14`).
- NAV is the discounted future value bucketed by maturity date (`navfeed.sol:312-341,500`).
- Without an `update`, the ceiling is 0. The gate is the valuation, not the lock.

**(g) Known problems.**
- NAV depends entirely on an auth oracle (`navfeed.sol:595`), and maturity is set by `file` (`navfeed.sol:241-246`).
- No registry whitelist in the Shelf.
- The legacy proofs depend on Centrifuge identity contracts and a specific off-chain node (precise-proofs). That stack is abandoned (2019-2020 code).
- Tinlake is deprecated in favour of v2/v3.
- Publicly reported Tinlake pool defaults: **no verificado aquí**.

---

## 2. Centrifuge protocol v3

Repo and commit: `protocol-v3` @ 1835217 (2026-10-26). The README points to `centrifuge/protocol`.

**(a) What the token represents.**
- There is **no per-loan or per-document token**.
- Pools hold "assets": ERC-20s, or ERC-6909 ids (`asset`, `tokenId`) registered with `Spoke.registerAsset` (`protocol-v3/src/core/spoke/Spoke.sol:84-111`).
- Investors get ERC-20 share tokens (`README.md`, "Standards-based composability").
- Legal title sits with the issuer and its SPV off chain: **no verificado**.
- Why Centrifuge dropped the per-asset NFT model: **no verificado**.

**(b) Standard and custody.**
- Assets sit in a per-pool `Escrow` as fungible amounts per share class, `holding[scId][asset][tokenId].total` (`src/core/spoke/Escrow.sol:20,33-36`).
- `noteDeposit` books an asset deposit **without any token transfer** (`Spoke.sol:138-146`). Off-chain loans are recorded by manager attestation.
- `AccountingToken` is an ERC-6909 used only for in-flight requests and liabilities, not loans (`src/managers/spoke/AccountingToken.sol:14-20`).

**(c) Metadata.**
- Only name, symbol and decimals of the asset (`Spoke.sol:95-103`).
- No document hash, URI or maturity on chain.

**(d) Lifecycle.**
- Deposit, withdraw and reserve per holding (`Spoke.sol:120-164`; `Escrow.sol:33-48`).
- A loan has no origination, repayment or write-off of its own. All of that is expressed as holding amount and price changes.

**(e) Access control.**
- A single per-pool `manager` role gates every balance-sheet operation (`src/core/spoke/README.md`, Spoke section).

**(f) Valuation.**
- Each holding carries an `IValuation` (`src/core/hub/Holdings.sol:47-62,79-87,191-199`).
- `OracleValuation` takes prices from trusted feeders with "Quorum ... always 1" (`src/valuations/OracleValuation.sol:19-25,71-72`).
- `IdentityValuation` always returns 1.0 (`src/valuations/IdentityValuation.sol:16,27`).

**(g) Known problems.**
- Single-feeder price trust (`OracleValuation.sol:21`).
- ERC-6909 async vaults are unsupported (`docs/audits/out-of-scope/cantina-bug-bounty.md:19`).

---

## 3. Huma Finance v2

Repo and commit: `huma-contracts-v2` @ 21aaad5 (2024-12-31).

**(a) What the token represents.**
- `Receivable` is an ERC-721 for one off-chain receivable.
- The code disclaims legal effect: *"The NFT itself does not assert ownership of the real world asset. It needs to be accompanied by off-chain legal agreements"* (`huma-contracts-v2/contracts/credit/Receivable.sol:16-23`).
- Legal title stays with the off-chain agreements.

**(b) Standard and custody.**
- One shared ERC-721 collection, "HumaReceivable"/"HREC", enumerable, URI-storage, burnable, UUPS-upgradeable (`Receivable.sol:24-33,88-100`).
- The pool points at it through `PoolConfig.receivableAsset` (`contracts/common/PoolConfig.sol:147,498-501`).
- On drawdown the NFT moves borrower → credit contract:
  - `ReceivableBackedCreditLine.sol:276`
  - `ReceivableFactoringCredit.sol:71`
- **No code path releases or burns it afterwards.** No `transferFrom(address(this)…)` exists in `contracts/`, and the comment at `ReceivableFactoringCredit.sol:177` reads "Don't delete the paid receivable". The only exit is the holder calling `burn`, and the holder is the credit contract.

**(c) Metadata.**
- On chain, `ReceivableInfo` holds `receivableAmount`, `creationDate`, `paidAmount`, `currencyCode`, `maturityDate`, `creator` and `state` (`contracts/credit/CreditStructs.sol:112-120`). States run Minted → Approved → PartiallyPaid → Paid, or Rejected, Delayed or Defaulted (`CreditStructs.sol:91-100`).
- `referenceId` (the originator's internal id) is stored only as `keccak(address(this), referenceId, creator)` for de-duplication (`Receivable.sol:160-166,193-198`; `ReceivableStorage.sol:16-17`).
- The plaintext `referenceId` is still in the transaction calldata. Hashing it in storage is **not** a privacy measure.
- `tokenURI` is a free-form URI (`Receivable.sol:180`).
- Off-chain storage, encryption and permissioned reads: **no verificado**. The repo has nothing on them.

**(d) Lifecycle.**
- Mint: `createReceivable`, `MINTER_ROLE` only (`Receivable.sol:151-185`).
- Approval:
  - Credit lines: the Evaluation Agent or auto-approval runs `approveReceivable`, and credit grows by `advanceRateInBps × receivableAmount` (`ReceivableBackedCreditLineManager.sol:38-41,97-114`).
  - Factoring: `approveReceivable` is EA-only (`ReceivableFactoringCreditManager.sol:74-109`).
- Draw: `drawdownWithReceivable`. It requires that the borrower owns the NFT, that the amount does not exceed the face value, and that the receivable is not matured and is in state Minted or Approved (`ReceivableBackedCreditLine.sol:93-112,250-277`; status check at `ReceivableBackedCreditLineManager.sol:87-95`).
- Repayment: `makePaymentWithReceivable` only checks that the credit contract holds the NFT (`ReceivableBackedCreditLine.sol:117-133,279-291`).
- Payment status on the NFT is **self-declared** by the owner or creator through `declarePayment` (`Receivable.sol:103-120`).
- Default: `triggerDefault`, EA-only, distributes the loss to tranches (`ReceivableFactoringCreditManager.sol:120-128`; `CreditManager.sol:317-341`).
- Secondary transfer: the NFT is a free ERC-721 until it is pledged.

**(e) Access control.**
- Minting needs `MINTER_ROLE` (`ReceivableStorage.sol:10`).
- Metadata URI and payment declarations can be changed by the **owner or the original creator** at any time (`Receivable.sol:131-141`; the comment at 122-130 accepts this as a "transparency layer").
- Upgrades need `DEFAULT_ADMIN_ROLE` (`Receivable.sol:230-232`).

**(f) Valuation.**
- No oracle. Credit equals face value times the advance rate (`ReceivableBackedCreditLineManager.sol:106`).
- Pool value comes from credit records (principal plus accrued yield) minus losses taken at `triggerDefault`.

**(g) Known problems.**
- Mutable metadata.
- Self-reported payments.
- Burning an approved but unused receivable does not reduce available credit (`Receivable.sol:245-251` comment).
- The NFT is never released after repayment.
- Not legally binding by the code's own admission.

---

## 4. Untangled Finance

Repos:
- `untangled-protocol` @ d90872b (2026-10-09). This is a **third-party mirror** (`fsinbad/untangled-protocol`). The `untangledfinance` GitHub org publishes no contract repo, so the code may not match the deployed contracts: **no verificado**.
- Docs: `untangled-docs`.

**(a) What the token represents.**
- `LoanAssetToken` (LAT) is *"The representative for ownership of a Loan"* (`untangled-protocol/contracts/tokens/ERC721/LoanAssetToken.sol:12-15`).
- The docs: originators "lock up collaterals (in a SPV)" and "a financing token (LAT) is minted for each collateral" (`untangled-docs/docs/credio/Onchain-Private-Credit/How-protocol-works.md:38,58`).
- Legal title sits with the SPV.

**(b) Standard and custody.**
- ERC-721 (an OpenZeppelin preset minter/pauser) (`contracts/tokens/ERC721/UntangledERC721.sol:8`).
- Minted **directly to the pool** in `fillDebtOrder` (`contracts/protocol/loan/LoanKernel.sol:367-368`).
- The pool accepts only LATs (`contracts/protocol/pool/Pool.sol:139-143`).
- It stays in the pool for the whole life of the loan.

**(c) Metadata.**
- tokenId = `agreementId = keccak256(version, debtor, termsParam, salt)` (`LoanKernel.sol:252-265`), checked at 374-377. This is a salted commitment to the loan terms.
- Stored per loan: `debtor`, `principalTokenAddress`, packed `termsParam`, `salt`, issuance and expiration timestamps, `riskScore` and `assetPurpose` (`contracts/libraries/DataTypes.sol:47-56`). Valuation fields sit in `NFTDetails` (`DataTypes.sol:70-84`).
- `tokenURI` is `baseURI + hex(tokenId) + "?chain_id="`, a pointer to Untangled's API (`UntangledERC721.sol:46-61`).
- No document hash.
- Investor KYC is off chain (Shuftipro). Only a UID ERC-1155 goes on chain, and "no personal information is stored on-chain" (`untangled-docs/.../verification-process.md:8-11`).

**(d) Lifecycle.**
- Mint at drawdown.
- Partial repayment through `repayInBatch`, which looks up the pool from `ownerOf(LAT)` (`LoanKernel.sol:408-448`).
- Full repayment **burns** the LAT (`LoanKernel.sol:200-203,223-232,173-175`). The pool pre-approves the kernel with `setApprovalForAll` (`Pool.sol:63`).
- Write-off by risk-score groups after maturity plus grace (`contracts/libraries/logic/PoolNAVLogic.sol:401-423`). It is gated to `POOL_ADMIN_ROLE` (`Pool.sol:152-156`).
- Secondary transfer: `exportAssets` moves LATs to another pool, admin or owner only (`Pool.sol:100-104`). `withdrawAssets` is owner only (`Pool.sol:109-113`).

**(e) Access control.**
- Only `LoanKernel` holds `MINTER_ROLE` (`LoanAssetToken.sol:39-40`).
- When the pool sets `validatorRequired`, every mint needs an **EIP-712 validator signature** over `(tokenIds, nonces, validator)`, with per-token nonces, from an address holding the pool's `VALIDATOR_ROLE` (`contracts/tokens/ERC721/LATValidator.sol:18-31,43-52,67-80`; `LoanAssetToken.sol:52-54`).
- Only `ORIGINATOR_ROLE` may receive the drawdown (`Pool.sol:405-409`).
- Risk scores are set by `POOL_ADMIN_ROLE` (`Pool.sol:89-93,237-241`).

**(f) Valuation.**
- A Tinlake-style discounted NAV with risk scores and write-off groups (`contracts/libraries/logic/PoolNAVLogic.sol`; `Pool.sol:237`).

**(g) Known problems.**
- The code is not published by the team, so it is unclear what is deployed.
- Valuation is admin-set.
- No document hash, so the token cannot be checked against the paper.

---

## 5. Goldfinch

Repo and commit: `goldfinch` (sparse checkout `packages/protocol/contracts`) @ bb25167 (2025-11-30).

**(a) Confirmed: there is no per-loan document token.**
- `PoolTokens` are **lender (Backer) positions**: `TokenInfo{pool, tranche, principalAmount, principalRedeemed, interestRedeemed}` (`goldfinch/packages/protocol/contracts/interfaces/IPoolTokens.sol:9-15`). They are minted only by pools (`protocol/core/PoolTokens.sol:69-88`).
- The loan is a contract: `CreditLine` *"represents the agreement between Backers and a Borrower"* (`protocol/core/CreditLine.sol:20-27`).
- A grep across the contracts finds no document hash and no URI for the loan.
- How the borrower is bound legally (off-chain loan agreement with an SPV or trust): **no verificado**. The repo holds no legal documents.

**(b) Custody.**
- The borrower is an address, or the optional `Borrower` contract (`protocol/periphery/Borrower.sol:21-31`). It gets `LOCKER_ROLE` on its `TranchedPool` (`protocol/core/TranchedPool.sol:85`).
- Nothing is locked.

**(c) Metadata.**
- Terms only: limit, APR, schedule and late fee (`TranchedPool.sol:56-74`).
- Identity: a non-transferable UID ERC-1155 minted with a `SIGNER_ROLE` signature after off-chain KYC (`protocol/core/UniqueIdentity.sol:16,51-52,78-93,128`). The `Go` gate checks it (`protocol/core/Go.sol:79,164`). No PII goes on chain.

**(d) Lifecycle.**
- Drawdown by the locker (`TranchedPool.sol:200`).
- Writedown by days late (`protocol/core/Accountant.sol:43-60`; `protocol/core/SeniorPool.sol:709`).

**(e) Access control.**
- The borrower holds `LOCKER_ROLE`, and governance or owner roles do the rest.

**(f) Valuation.**
- Principal less a time-based writedown (`Accountant.calculateWritedownFor`).

**(g) Known problems.**
- Public borrower defaults: **no verificado aquí**.
- Enforcement depends entirely on off-chain contracts that the chain never references.

---

## 6. Other protocols (10-minute scan)

**Maple open-term loan** (`maple-open-term-loan` @ 2025-05-25)
- (a, b) Each loan is its own contract (`MapleLoan`) with `borrower`, `lender`, `principal` and so on (`contracts/MapleLoanStorage.sol:9-27`). There is no token and no document reference.
- (d) Lifecycle: `fund`, `makePayment`, `callPrincipal`, `impair`/`removeImpairment` and `repossess`, all lender-gated (`contracts/MapleLoan.sol:180,260,276,292,332,343`).
- **Useful pattern.** Amendments use a commitment hash:
  - the lender calls `proposeNewTerms`, which stores `refinanceCommitment = hash(refinancer, deadline, calls)` (`MapleLoan.sol:304-318`)
  - the borrower must call `acceptNewTerms` with matching data before the deadline (`MapleLoan.sol:95-118`)
  - the commitment is cleared once it is used
- This gives two-party consent on chain to the new terms.

**Provenance / Figure** (`provenance`, sparse `x/metadata` @ 2026-10-30). **Not EVM** (Cosmos SDK), but it is the clearest model of a loan file on chain.
- A `Scope` (the loan) holds records, and each record is a hash plus data type and ownership (`proto/provenance/metadata/v1/scope.proto:30`).
- `owners` must sign every change (`scope.proto:79-81`).
- `data_access` lists the addresses "authorized to receive off-chain data" (`scope.proto:82-83`).
- `value_owner_address` is the economic owner, tracked as a bank coin `nft/<scope_id>`, so value moves separately from the data (`scope.proto:84-98`).
- Record inputs and outputs carry only hashes (`scope.proto:151-152,169-170,189-193`).
- `ObjectStoreLocator{owner, locator_uri, encryption_key}` points to each party's **encrypted object store** (`proto/provenance/metadata/v1/objectstore.proto:14-23`).

**Not checked (out of time):** Credix (Solana, not EVM), Jia, OpenEden and Backed. Their designs are **no verificado**.

---

## Patterns worth copying for a document-backed token on a Solana pool

1. **[Privacy] Anchor a salted merkle root of the expediente, not the files.** Build one leaf per field or document as `hash(field_id ‖ value ‖ salt)`. Put only the root on chain, and keep the salts off chain with the parties. This copies Centrifuge's `AnchorRepository` and precise-proofs (`centrifuge-ethereum-contracts/contracts/AnchorRepository.sol:82-130`, `InvoiceUnpaidNFT.sol:179-192`).
2. **[Privacy] Selective disclosure at mint.** Reveal on chain only what the pool must enforce (amount, currency, maturity, "status = vigente"), each value with a merkle proof against the anchored root (`InvoiceUnpaidNFT.sol:154-279`). RFC, CURP, names and addresses never leave the off-chain file.
3. **Bind the token to the document both ways.** The document lists the token (mint address or PDA) and the token stores the document root. That enforces one token per document (`UserMintableERC721.sol:361-386`).
4. **Version chain for amendments and per-draw pagarés.** Store `doc_root` plus `next_version`, and refuse to lock a superseded version (`UserMintableERC721.sol:320-350`). Each pagaré is either its own leaf or record under the credit-agreement scope, or a new version.
5. **[Privacy] Read access follows the token.** The document's access rules grant read rights to the current token holder, and an off-chain service checks on-chain ownership before serving the encrypted file. Centrifuge does this with read roles (`UserMintableERC721.sol:491-526`), Provenance with `data_access` and encrypted object stores (`scope.proto:82-83`, `objectstore.proto:14-23`).
6. **Deterministic, salted id.** Derive the token or PDA seed from `hash(program, borrower, terms, salt)`, as Untangled's agreementId does (`LoanKernel.sol:252-265`). It is unique, opaque and checkable against the paper by whoever holds the salt.
7. **Escrow in the pool, gate every draw on custody.** Treat "the pool owns it" as the lock, and have `borrow` and `repay` require it (`tinlake/src/borrower/shelf.sol:171,263-269,291-293`). On Solana that means a pool-PDA-owned account, or Token-2022 with a frozen account or a permanent delegate.
8. **Validator co-signature on mint.** An independent validator (for example Soft.Law or the notary) signs `(token ids, nonces)`, so the originator cannot mint alone (`LATValidator.sol:18-31,67-80`).
9. **Close explicitly at the end of life.** Burn or close the token on full repayment (`LoanKernel.sol:200-203`). Keep a separate auth-only path that hands the token to a collector (`shelf.sol:299-302`), so the cobranza agent holds the paper in a default.
10. **Two-party commitment for term changes.** The lender proposes `hash(new terms)` and the borrower accepts the same hash before a deadline (`MapleLoan.sol:95-118,304-318`). That gives a "convenio modificatorio" on chain consent.
11. **Separate the data owner from the value owner.** Keep who may amend the record (the parties) apart from who holds the economic right (the pool) (`scope.proto:79-98`). This matters if the credit right is later assigned (cesión).

## Patterns to avoid

12. **Mutable URI or self-declared status set by the originator.** Huma lets the creator rewrite `tokenURI` and declare payments at any time (`Receivable.sol:103-141`). Make status changes come from the pool program, and freeze metadata once the token is locked.
13. **[Privacy] Plaintext identifiers in instruction data, or a URI that leaks.** Huma hashes `referenceId` in storage, but the plaintext travels in calldata (`Receivable.sol:151-166`). On Solana, instruction data is equally public. Never pass the folio, RFC or a document name in an instruction, and never put them in an Arweave or IPFS URI.
14. **A token with no release or burn path, or one that comes back after a write-off.** Huma never releases the receivable (`ReceivableBackedCreditLine.sol:276`, `ReceivableFactoringCredit.sol:177`). Tinlake's `unlock` returns the NFT to the borrower after a 100% write-off (`shelf.sol:278`), which hands back the paper the cobranza needs.
15. **Assuming the token is the legal title, or trusting one appraiser.** Huma says so outright (`Receivable.sol:16-23`), and Tinlake's and v3's NAV is a single trusted feeder (`navfeed.sol:595`; `OracleValuation.sol:21`). The pagaré is a título de crédito that is enforced by **physical or electronic possession under Mexican law** (legal point: **no verificado**, ask counsel). The token must record who holds it and where. It cannot replace it. Also, v3's move away from per-asset NFTs to priced fungible holdings (`Spoke.sol:84-146`) suggests per-loan tokens are mainly worth it for proof and custody, not valuation (the reason is **no verificado**).
