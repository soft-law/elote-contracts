# Document token for elote_pool: Solana-side options

Date: 2026-10-31. Scope: how the pool could lock a token that represents the signed loan documents
(credit agreement, one pagaré per draw, credit file) with only hashes, folios, amounts and dates
public. Research only; nothing in `elote-contracts` was modified and nothing in the cloned repos
was built or run.

Sources (cloned or unpacked under `scratchpad/repos-rwa/`, read only):

| source | version read |
|---|---|
| `mpl-core` (Metaplex Core) | commit `a5c35db436fdb6382ac75b1d59205d769c4762ea` (2026-10-24), program crate 0.2.0, Rust client `mpl-core` 0.12.0 |
| `token-2022` (solana-program) | commit `986b3aa58cbcc40dea12b369912017a4066804e7` (2026-10-30) |
| `huma/` = npm `@huma-finance/shared` 0.0.81 | IDL `dist/solana/idl/huma.json`, program `huma` 2.2.0 |
| `credix/` = npm `@credix/credix-client` 3.2.0 | IDL `dist/idl/credix.d.ts`, program `credix` 3.13.0 |
| `light-protocol` | commit `bec7b830250126e1f6a2993872599f45af6e0aa0` (2026-10-14) |
| `spl-token-metadata-interface` 0.8.0, `spl-token-group-interface` 0.7.1, `solana-rent` 4.5.0 | local cargo registry (`~/.cargo/registry/src/index.crates.io-*/`) |

Citation convention: `repo: path:lines`, paths relative to the repo root. "no verificado" = not
verifiable from the code or docs read here.

Rent arithmetic used everywhere: effective **6 960 lamports per byte on (128 + bytes)**, i.e.
6 960 lamports per account byte plus 890 880 lamports fixed overhead per account
(`solana-rent-4.5.0: src/lib.rs:58,86`). The two branches at `src/lib.rs:157-159` are equivalent
forms of the same total (SIMD-0194: 1 × (128 + bytes) × 6 960; legacy: 2 × (128 + bytes) × 3 480);
which threshold the 4.5.0 default uses was not read. Any future rent reduction: no verificado. USD figures: the
SOL price is not checked here; multiply by the day's price.

---

## 0. What elote_pool does today

- `ARCHITECTURE.md:12-45` ("0. One program, not two"): the legal text stays off chain; the program
  stores 32 bytes, checks they are non-zero and never interprets them; the borrower's signature on
  the `open_line` transaction binds the wallet to the hash; the web app reads the hash back and
  asks `GET /api/agreements/by-hash/:hash`.
- `programs/elote_pool/src/instructions/lines.rs:61-69`: `open_line(line_id, principal, rate_bps,
  term_end, agreement_hash: [u8; 32], sponsor, schedule)`; admin + borrower (+ sponsor) sign
  (`lines.rs:29-35`).
- `lines.rs:78`: `require!(agreement_hash != [0u8; 32])`; `lines.rs:94`: `line.agreement_hash =
  agreement_hash`; `lines.rs:126`: the hash is emitted in `LineOpened`.
- `programs/elote_pool/src/state.rs:96-134`: `CreditLine` (PDA `[LINE, pool, line_id]`,
  `lines.rs:47-53`) has `agreement_hash: [u8; 32]` at `state.rs:105-106`; there is no per-draw
  (pagaré) hash, no credit-file hash and no holder/assignee field. `INIT_SPACE` = 247 bytes, so the
  account is 255 bytes ≈ 0.00266 SOL rent.
- `LineStatus` = `Approved | Active | Repaid | WrittenOff | InCollection` (`state.rs:85-94`); there is
  no instruction that sells or assigns a line (no `sell`/`assign`/`transfer_line` in `lines.rs`).

So the pool already *locks* the agreement in the sense that matters for enforcement: the hash and the
economic terms live in a PDA only the program writes. A document token would add (1) a visible,
indexable handle in wallets/explorers, (2) a transferable object for a future sale/assignment of
the loan, and (3) a place for per-draw pagaré hashes. It adds no enforcement the pool lacks. The token
is a receipt/pointer to the documents, not the título de crédito itself (legal effect is outside this
report).

---

## 1. Metaplex Core (`CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d`)

### (a) What it represents, what is public

One `AssetV1` account per document (or per line). Everything in it is public:
`key, owner, update_authority, name, uri, seq` (`mpl-core: programs/mpl-core/src/state/asset.rs:19-32`)
plus a plugin header and registry appended to the same account
(`plugins/plugin_header.rs:10-20`, `plugins/plugin_registry.rs:30-32,144-150`). Public by design:
the pool PDA as owner, folio, document type, SHA-256 of the signed PDF/JSON, amount, issue/due
dates. Nothing personal goes in `name`, `uri` or attributes.

Collections: `CollectionV1 { update_authority, name, uri, num_minted, current_size }`
(`state/collection.rs:14-27`). An asset whose `update_authority` is `UpdateAuthority::Collection(pk)`
(`state/update_authority.rs:6-13`) belongs to it; collection plugins are checked first and asset
plugins of the same type override them (`utils/mod.rs:197-216`).

### (b) Holding, freezing, releasing, keeping, transferring

- **Pool PDA as owner.** `owner` is any pubkey (`state/asset.rs:23`); the pool PDA can be the owner
  from creation (`CreateV2` takes `name, uri, plugins, external_plugin_adapters`,
  `clients/rust/src/generated/instructions/create_v2.rs:141-147`). The PDA signs later CPIs with
  `TransferV1CpiBuilder::invoke_signed` / `UpdatePluginV1CpiBuilder::invoke_signed`
  (`clients/rust/src/generated/instructions/transfer_v1.rs:458,582`,
  `update_plugin_v1.rs:299`). The owner approves transfer (`state/asset.rs:286-297`) and burn
  (`state/asset.rs:272-283`).
- **Permanent plugins only at creation.** `PermanentFreezeDelegate` and `PermanentTransferDelegate`
  reject being added later (`plugins/internal/permanent/permanent_freeze_delegate.rs:64-77`,
  `permanent_transfer_delegate.rs:25-38`; `plugins/lifecycle.rs:83-97`); the processor runs the new plugin's own `validate_add_plugin` against itself, so the self-rejection fires on every `AddPluginV1` (`processor/add_plugin.rs:76`, collection `:164`). Their manager is the update
  authority (`plugins/mod.rs:265-287`). So the pool (or the backend naming the pool PDA as
  update authority and plugin authority) must mint the asset; the borrower cannot mint and then pledge.
- **Owner-managed delegates do not survive a transfer.** `TransferV1` resets every owner-managed
  plugin authority to `Owner` (`processor/transfer.rs:100-112`), so a plain `FreezeDelegate` or
  `TransferDelegate` (manager `Owner`, `plugins/mod.rs:270-272`) handed to the pool evaporates on the
  first transfer. Use the `Permanent*` variants.
- **Freeze.** `PermanentFreezeDelegate { frozen: bool }` rejects transfer and burn while frozen
  (`permanent_freeze_delegate.rs:15-18,41-62`) and blocks its own removal while frozen (`:79-89`).
  The pool toggles `frozen` with `UpdatePluginV1`; approval comes when the signer resolves to the
  plugin authority (`plugins/lifecycle.rs:335-351`; `check_update_plugin → CanApprove`,
  `lifecycle.rs:116-121`).
- **Release on repayment.** On `LineStatus::Repaid` the pool either (i) transfers the asset to the
  borrower as a paid receipt (unfreeze then `TransferV1`) or (ii) burns it (`BurnV1`; burn needs
  the asset unfrozen, `permanent_freeze_delegate.rs:42-51`). Burn leaves a 1-byte account and returns
  `rent(size) − rent(1)` (`utils/account.rs:10-35`).
- **Keep on default.** Do nothing: frozen asset stays with the pool PDA; optionally update an
  attribute `status=written_off`.
- **Transfer on sale of the loan.** `PermanentTransferDelegate` force-approves transfers signed by
  its authority (`permanent_transfer_delegate.rs:40-51`), and `ForceApproved` short-circuits the
  checks (`plugins/lifecycle.rs:729-745`, `utils/mod.rs:229-285`) while `Rejected` only sets a flag.
  Consequence: the delegate can move the asset **even while frozen** and before external adapters
  (Oracle) are evaluated (`utils/mod.rs:283-287`). Feature for a sale; risk if the delegate key is
  anything but the pool PDA. Assets with permanent delegates cannot be moved into another
  collection that has them (`processor/update.rs:199-202`).
- **State-derived lock (alternative).** The `Oracle` external adapter reads a `ValidationResult`
  for create/transfer/burn/update from an account at `base_address` or a PDA derived from it
  (`plugins/external/oracle.rs:11-24,172-180,195-205`). With `base_address` = `elote_pool` and the
  oracle account derived from the line, "no transfer unless `Repaid`" would follow from pool state
  instead of a freeze toggle. Not needed if the pool is the owner.

### (c) Metadata model

- On-chain key/value: `Attributes { attribute_list: Vec<Attribute { key: String, value: String }> }`
  (`plugins/internal/authority_managed/attributes.rs:7-31`), manager = update authority
  (`plugins/mod.rs:273`). Strings only: store the SHA-256 as 64 hex chars (or 44 base58).
  Example keys: `doc=pagare`, `folio=EL-2026-000123`, `sha256=<hex>`, `amount_mxn=150000.00`,
  `issued=2026-11-02`, `due=2027-05-02`, `line=<line PDA>`.
- Appendable data: `AppData { data_authority, schema }` (`plugins/external/app_data.rs:11-22`); the
  `data_authority` is fixed once added and only it can write (`processor/write_external_plugin_adapter_data.rs:184-187`);
  schema `Binary | Json | MsgPack` (`plugins/external_plugin_adapters.rs:783-791`). Good for "append one
  pagaré hash per draw" with `data_authority = Address(pool PDA)`; `LinkedAppData` + `DataSection`
  does the same at collection level (`plugins/external/data_section.rs:5-11`,
  `plugins/external/linked_app_data.rs:8-18`).
- `uri`: points to a JSON with the same public fields; the signed PDF should be an encrypted blob or
  not referenced at all. `uri` is changed by `UpdateV1/V2 { new_name, new_uri, new_update_authority }`
  (`processor/update.rs:26-38`), approved for the update authority (`state/asset.rs:258-269`).
- Immutability options, which are distinct:
  - `ImmutableMetadata` rejects `UpdateV1/V2` only, i.e. name/uri/update authority
    (`plugins/internal/authority_managed/immutable_metadata.rs:23-31`; `lifecycle.rs:154-160`). It does
    not hook `check_update_plugin`, so **Attributes stay mutable under it**.
  - To freeze the Attributes, give that plugin `Authority::None` at create
    (`state/mod.rs:51-53`); plugins with `None` cannot be removed (`lifecycle.rs:101-112`) nor updated
    (no resolved authority matches, `lifecycle.rs:335-351`).
  - `UpdateAuthority::None` makes the whole asset immutable (`state/update_authority.rs:7-8`); then the
    pool also loses the ability to add plugins.
- Signatures on chain: `VerifiedCreators { signatures: Vec<{address, verified}> }`
  (`authority_managed/verified_creators.rs:15-38`) and `Autograph { signatures: Vec<{address, message}> }`
  (`owner_managed/autograph.rs:16-38`, manager Owner, `plugins/mod.rs:281`). VerifiedCreators could
  record that the lender key (and borrower wallet) co-signed; never put names or RFC in `message`.

### (d) Cost

Layout for one pagaré asset, pool PDA as owner and update authority:

| part | bytes |
|---|---|
| `AssetV1` base 1+32+(1+32)+4+4+1 (`state/asset.rs:36-41`) | 75 |
| name `"elote L12 pagare 03"` + uri ~60 chars | ~80 |
| plugin header (`plugin_header.rs:18-19`) | 9 |
| PermanentFreezeDelegate (1 tag + 1), PermanentTransferDelegate (1), ImmutableMetadata (1) | 4 |
| Attributes, 7 pairs incl. 64-char hex hash (`attributes.rs:15-16,34`) | ~240 |
| registry base 9 + 4 records × (1 type + 1 authority + 8 offset) (`plugin_registry.rs:30-32,144-150`) | 49 |
| **total** | **~457** |

Rent ≈ (128 + 457) × 6 960 = 4 071 600 lamports ≈ **0.0041 SOL**, plus the flat **create fee
0.0015 SOL** (`state/collect.rs:9-17`, added in `processor/create.rs:127`) ⇒ ~**0.0056 SOL per asset**.
On burn ≈ 0.0032 SOL comes back; ~0.0024 SOL (fee + 1-byte rent) is not recoverable by the payer
(`utils/account.rs:10-35`). A collection per line adds one `CollectionV1` (~120 bytes ≈ 0.0017 SOL).
Compute units for `CreateV2`/`TransferV1`/`UpdatePluginV1` by CPI: no verificado (structural driver: every
lifecycle event deserializes each registered plugin, `utils/mod.rs:160-216`). Transaction fee 5 000
lamports per signature.

### (e) Composability and risks

- Wallet/explorer display and DAS indexing of Core assets: per Metaplex docs, not verifiable from
  this code (no verificado). The program logs through SPL Noop when a `log_wrapper` is passed
  (`processor/transfer.rs:36-40`).
- Upgrade authority: program data `9ZC25KLUrfgSoFVgjE1rrydZBbZns58UXi8A8ZhTdGfr`, authority
  `bfQVv6niKVgEURYqQ1beJmiEQQN7MrvLRvk3mZGFubb` (mainnet `getAccountInfo`, 2026-10-31). Upgradeable;
  multisig or not: no verificado. Audit noted in `README.md:28-30` (Mad Shield, 2024-05-03).
- Protocol fee is a constant in the program (`state/collect.rs:14,24`) and can change with an upgrade.
- Plugin authority misuse: a `PermanentTransferDelegate` held by anything other than the pool PDA is a
  backdoor that ignores freeze; an update authority that is a hot wallet can rewrite unprotected
  Attributes and `uri`.
- URI rot: the `uri` is just a string; keep the hash on chain so a dead URI does not break verification.

---

## 2. Token-2022 NFT (`TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`)

### (a) What it represents, what is public

A mint with supply 1 and decimals 0, plus one token account holding it. Public: mint extensions
(authorities, metadata name/symbol/uri/additional_metadata), token account owner and state. Metadata
lives inside the mint account itself (`token-2022: program/src/extension/token_metadata/processor.rs:57-62`)
and requires the `MetadataPointer` extension (`:75-78`).

### (b) Holding, freezing, releasing, keeping, transferring

Extension facts that decide the combination:

- `NonTransferableAccount` rejects transfers before the authority match, so even the permanent delegate
  cannot transfer (`program/src/processor.rs:393-399` vs the permanent-delegate arm at `:486-494`).
  `NonTransferable` also forces `ImmutableOwner` on token accounts (`processor.rs:1097-1109`). The
  permanent delegate can still burn (`processor.rs:1258,1277`).
- A frozen source account blocks transfer (`processor.rs:387-389`), and a frozen destination blocks
  receiving (`:553`); a frozen account also blocks burn (`:1230`). Freeze/thaw needs the mint
  `freeze_authority` (`processor.rs:1464-1477`).
- `DefaultAccountState::Frozen` requires a freeze authority (`processor.rs:139-142`;
  struct `interface/src/extension/default_account_state/mod.rs:16-19`).
- `PermanentDelegate { delegate }` can transfer or burn from any account of the mint
  (`interface/src/extension/permanent_delegate.rs:13-22`; `processor.rs:486-494`).
- Removing the mint authority fixes supply permanently (`processor.rs:828-834`, `TokenError::FixedSupply`).
- `MintCloseAuthority` closes the mint only at supply 0 (`processor.rs:1408-1419`;
  `interface/src/extension/mint_close_authority.rs:18-22`).

Resulting "one-of-one document token that only the pool can move":

| need | setting |
|---|---|
| one of one | decimals 0, mint 1, then `SetAuthority(MintTokens, None)` |
| pool holds it | token account owned by the pool PDA (ATA with `ImmutableOwner`); with `DefaultAccountState::Frozen` that ATA is born frozen and `MintTo` rejects a frozen destination (`processor.rs:1081-1083`), so the pool thaws its ATA before `MintTo`, then refreezes |
| nobody else moves it | owner = pool PDA already suffices; add `freeze_authority` = pool PDA and `DefaultAccountState::Frozen` so any new account of the mint starts frozen until the pool thaws it |
| sale of the loan | thaw buyer's account (born frozen; frozen destination rejects, `processor.rs:553`) → thaw pool's account (`:387`) → transfer (as owner or `PermanentDelegate` = pool PDA) → refreeze both |
| release on repayment | burn: thaw pool's account (`:1230`) → `Burn` → `CloseAccount` → close mint via `MintCloseAuthority` = pool PDA; or receipt: thaw borrower's and pool's accounts → transfer → leave or refreeze |
| keep on default | do nothing |
| **do not use** `NonTransferable` | it would make sale impossible; with it the pool can only burn |

### (c) Metadata model

- `TokenMetadata { update_authority, mint, name, symbol, uri, additional_metadata: Vec<(String, String)> }`
  and `Field = Name | Symbol | Uri | Key(String)` (local crate `spl-token-metadata-interface-0.8.0: src/state.rs:23-38,113-122`;
  the repo pins 1.0.0 at `program/Cargo.toml:50`, shape at 1.0.0 no verificado).
- `UpdateField` / `RemoveKey` need the metadata update authority as signer
  (`program/src/extension/token_metadata/processor.rs:108-160`, check at `:28-41`) and realloc the
  mint (the payer must top up rent, `:119-131`).
- Immutability: `UpdateAuthority` to `None` (`processor.rs` metadata `:166-186`); afterwards every update
  fails with `ImmutableMetadata` (`:35-36`). There is no per-key lock: it is all-or-nothing, so a
  pagaré appended later forces the authority to stay alive.
- Grouping: `GroupPointer` / `GroupMemberPointer` (`interface/src/extension/group_pointer/mod.rs:21-27`,
  `group_member_pointer/mod.rs:21-27`) with `TokenGroup { update_authority, mint, size, max_size }` and
  `TokenGroupMember { mint, group, member_number }` (local `spl-token-group-interface-0.7.1: src/state.rs:16-26,67-75`):
  one group mint per line, one member mint per pagaré.

### (d) Cost

| account | bytes | rent |
|---|---|---|
| mint: 165 padded base + 1 type (`interface/src/extension/mod.rs:311-314`, `interface/src/state.rs:147`) + TLV (4 each): MintCloseAuthority 32, MetadataPointer 64, PermanentDelegate 32, DefaultAccountState 1, TokenMetadata 32+32+4×4+name 20+symbol 5+uri 60+7 pairs ~235 | ~725 | ~0.0059 SOL |
| pool token account: 165 + 1 + ImmutableOwner 4 | 170 | ~0.0021 SOL |
| (optional) GroupMemberPointer 64 + TokenGroupMember 72 + TLV 8 | +144 | +0.0010 SOL |
| **total** | **~895-1 040** | **~0.0080-0.0090 SOL**, fully recoverable by closing both accounts |

No protocol fee. Eight to ten instructions to mint (create mint account, init extensions, init mint,
init metadata, add fields, create ATA, thaw ATA, mint, refreeze, remove mint authority); compute
units no verificado.

### (e) Composability and risks

- Wallet support for Token-2022 metadata and DAS indexing of it: no verificado from code.
- Upgrade authority: program data `DoU57AYuPFu2QU514RktNPG22QhApEjnKxnBcu4BHDTY`, authority
  `AeLmXCbPaQHGWRLr2saFsEVfmMNuKnxRAbWCT9P5twgz` (mainnet, 2026-10-31); multisig or not: no verificado.
- Authority misuse: the metadata update authority, freeze authority and permanent delegate must all be
  the pool PDA; any other key is an override.
- Footgun: `NonTransferable` silently disables the sale path (see b).

---

## 3. Huma Finance on Solana (program `EVQ4s1b6N1vmWFDv8PRNc77kufBP8HcrSNWXQAhRsJq9`, IDL 2.2.0)

### (a) Representation

A receivable is a **Metaplex Core asset plus a sidecar PDA**:

- `create_receivable(args: CreateReceivableArgs)`, "Creates a new receivable NFT", anyone can call
  (`huma: dist/solana/idl/huma.json:2220-2305`). Accounts: `asset` (new Core asset, signer), `owner`
  (signer), `huma_program_authority` PDA `["huma_program_authority"]`, `receivable_info` PDA
  `["receivable_info", asset]`, `mpl_core` = `CoREENx…` (`:2282-2284`).
- `CreateReceivableArgs { name, uri, currency_code, receivable_amount: u128, maturity_date: u64, reference_id }`
  (`huma.json:7717`); `reference_id` is "a unique internal reference ID used for de-duplication".
- `ReceivableInfo { bump, currency_code, receivable_amount, amount_paid, creation_date, maturity_date, creator, state }`
  (`huma.json:6880,10570-10660`), `ReceivableState = Deleted | Minted | Approved | PartiallyPaid | Paid | Rejected | Delayed | Defaulted`
  (`huma.json:10661`).
- `ReceivableCreatedEvent { owner, asset, reference_id, currency_code, receivable_amount, maturity_date }`
  (`huma.json:10526`).
- Public: amounts, dates, currency, creator, Core name/uri. No document hash field; the document
  binding is whatever the `uri` JSON contains.

### (b) Pledging

- `submit_receivable` (borrower; auto-approval) and `approve_receivable` (evaluation agent) both take
  `asset` writable, `receivable_info`, `credit_state`, `pool_authority` PDA `["pool_authority", pool_config]`
  and `mpl_core` (`huma.json:5259-5410`, `737-905`). Effect per docs: approval applies the advance
  ratio and raises `CreditState.receivable_available_credits` (`huma.json:7985-8030`), which `drawdown`
  then spends.
- `ReceivableOwnershipMismatch` (error 6623) shows the borrower must own the asset.
- **Which Core CPI happens on approval (transfer to `pool_authority` vs a plugin update) is no
  verificado from the IDL**; the program source is not in the npm package. The EVM v2 analogue
  transfers the receivable NFT to the credit line contract on drawdown
  (`huma-contracts-v2: contracts/credit/ReceivableBackedCreditLine.sol:276`, `safeTransferFrom(borrower, address(this), receivableId)`).
- `declare_payment(payment_amount)` and `update_receivable_metadata_uri(uri)`: only the Core update
  authority of the asset (`huma.json:2308-2360`, `5829-5856`; error `ReceivableUpdateAuthorityRequired` 6213);
  `declare_payment` is "for transparency only and is not guaranteed to be accurate".

### (c)-(e)

Metadata = Core `name`/`uri` (mutable by the update authority via the program) + typed fields in the
Huma PDA. Cost: one Core asset + one `ReceivableInfo` (~8+1+4+3+16+16+8+8+32+1 ≈ 97 bytes ≈ 0.0016 SOL).
Upgrade authority of the Huma program: `D6GEJYVtQGVrLVj1QSZbxWW73NnKJyHEzMBDAtCzraaF`
(program data `Du3J6xVp5ied1wkMMNqCNMF6ntmTvXbZYCHKeupvvmvY`, mainnet 2026-10-31). The pattern maps
directly onto elote: Core asset for display/ownership, `CreditLine` as the sidecar with the enforceable
fields.

---

## 4. Credix on Solana (`@credix/credix-client` 3.2.0, IDL `credix` 3.13.0)

Labels: (a) what is represented and public = first three bullets; (b) holding = no token: the
`receivable` PDA is program-owned, funded by `poolFundReceivable` and closed out by `receivableRepay`,
there is nothing to freeze or transfer; (c) metadata = typed IDL fields, identifiers as strings, tax
IDs hashed, no document hash or URI; (d) cost = one PDA, size no verificado; (e) composability = not
visible as a token in wallets, upgrade authority no verificado.

- **No token for documents.** A receivable is a plain PDA `receivable` with seeds
  `[duplicata_identifier_first, duplicata_identifier_last, "receivable"]`
  (`credix: dist/idl/credix.d.ts:6542-6606`, instruction `receivableCreate`).
- `ReceivableInfo { duplicataIdentifier, contractIdentifier, hashedTaxIdBuyer, hashedTaxIdSeller }`, all
  `string` (`credix.d.ts:8673-8695`); `CreateReceivableInput` adds `faceValue, advanceRate, discountRate,
  originationFeePercentage, expectedDueTs, daycountConvention` (`:8697-8756`). Account `receivable`
  adds `baseMint, market, fundedTs, sellerTokenAccount, amountsRepaid, latestPaymentTs, discountAmount`
  (`:7714-7800`). Instructions `poolFundReceivable`, `receivableRepay` (`:6638`, `:6734`).
- Personal data handling: tax IDs (Brazilian CNPJ, by context) are stored **hashed**, contract and
  duplicata identifiers in clear. Hashing a low-entropy tax ID without a salt is reversible by
  enumeration: a lesson for elote (never publish an unsalted hash of an RFC).
- `Deal` (`credix.d.ts:7426-7545`) has `name, borrower, amounts, dates, dealNumber…`: no document hash
  or URI. `CredixPass { releaseTimestamp, user, withdrawCap, flags }` (`:7344-7377`) is the KYC gate.
- `updateTokenMetadata` with `DataV2 { name, symbol, uri, sellerFeeBasisPoints… }` (`:2272-2370`,
  `:8190-8210`) is Token Metadata for LP/tranche tokens, not for loan documents.
- Cost: one PDA; size no verificado (strings unbounded in IDL). Upgrade authority: no verificado.

This is the "plain PDA record" column below, with citations.

---

## 5. Light Protocol ZK compression (time-boxed)

- `CompressedAccount { owner, lamports, address: Option<[u8;32]>, data: Option<CompressedAccountData> }`,
  `CompressedAccountData { discriminator, data, data_hash }`
  (`light-protocol: program-libs/compressed-account/src/compressed_account.rs:208-213,242-245`).
- "rent-free tokens and PDAs" (`README.md:12-13`); data lives in the ledger, the chain keeps tree roots,
  and each read/write carries a 128-byte Groth16 validity proof (`light-paper.md:30-32,139`).
- Liveness depends on Foresters emptying queues (`light-paper.md:70`); reads need the Photon indexer
  (`README.md:17`).
- `network_fee: Some(10000)` is a tree parameter (`program-libs/batched-merkle-tree/src/initialize_address_tree.rs:48`),
  not a per-account price. Per-commitment cost and CU: no verificado.
- Upgrade authority of the Light system program `SySTEM1eSU2p4BGQfQpimFEWWSC1XDFeun3Nqzz3rT7`:
  `7PeqkcCXeqgsp5Mi15gjJh8qvSLk7n3dgNuyfPhJJgqY` (mainnet 2026-10-31).
- Fit: a compressed PDA could hold `{line, kind, folio, sha256, amount, date}` for near-zero rent, but
  it is not a wallet-visible token, adds a CPI + proof to every elote instruction that touches it, and
  adds an indexer/forester dependency. Not worth it at pilot volumes (rent per plain PDA is ~0.002 SOL).
  It adds **no privacy**: compressed data is public on the ledger.

---

## 6. Comparison: document token owned by a pool PDA

| | Metaplex Core asset | Token-2022 NFT | plain PDA record (elote_pool) |
|---|---|---|---|
| accounts per document | 1 | 2 (mint + token account) | 1 (or 0: extra fields in `CreditLine`) |
| rent per document | ~0.0041 SOL + 0.0015 fee | ~0.0080 SOL | ~0.0019 SOL (≈149 B record) |
| recoverable on close | ~0.0032 SOL (fee + 1 B kept) | all | all |
| "only pool can move" | pool PDA owner + `PermanentFreezeDelegate` + `PermanentTransferDelegate`, set at `CreateV2` | pool PDA owner + `freeze_authority` + `DefaultAccountState::Frozen` + `PermanentDelegate`; no `NonTransferable` | by construction: only elote writes it |
| sale of the loan | `TransferV1` by permanent delegate, works even frozen | thaw buyer's and pool's accounts → transfer → refreeze both | new field `holder`/`assignee` + instruction |
| per-draw pagaré hashes | Attributes (per-plugin lock via `Authority::None`) or `AppData` with pool `data_authority`; or 1 asset per pagaré in a line collection | additional_metadata (all-or-nothing lock) or 1 member mint per pagaré in a token group | fixed-size array or one PDA per pagaré |
| immutability | `ImmutableMetadata` (name/uri) + `Authority::None` per plugin | metadata `update_authority = None`, mint authority `None` | program logic; no external authority |
| wallet/explorer display | yes per Metaplex (no verificado here) | yes for Token-2022 metadata (no verificado here) | no (custom UI) |
| external trust | Metaplex upgrade authority `bfQVv6…`, protocol fee constant | Token-2022 upgrade authority `AeLmXC…` | only elote's own upgrade authority |
| CPI / account budget per loan event | +1 CPI, +2-3 accounts | +1-3 CPIs, +3-4 accounts | none |
| who already does it | Huma (Core asset + `ReceivableInfo` PDA) | no lender seen in this research | Credix (`receivable` PDA), elote today (`agreement_hash`) |

---

## 7. Recommendation

1. **Keep the plain PDA as the source of truth** and extend it, not replace it: add a per-line
   document record (or extend `CreditLine`'s layout in a new version) with `credit_file_hash`, and one
   `PagareRecord` PDA per draw `[PAGARE, line, draw_index]` holding `{sha256, folio, amount, issued_at, due_at}`.
   This is where enforcement already lives (`lines.rs:78,94`), costs ~0.002 SOL per record, fully
   recoverable, adds no third-party upgrade risk and no CPI to `draw`/`repay`. Never publish an unsalted
   hash of an RFC/CURP (Credix's hashed tax IDs show the pattern; enumeration reverses it).
2. **If a visible token is wanted (investor display, future loan sale), use one Metaplex Core asset per
   line, minted by the pool program**, owner = pool PDA, update authority = pool PDA, with
   `PermanentFreezeDelegate` + `PermanentTransferDelegate` (authority = pool PDA) at `CreateV2`,
   `ImmutableMetadata`, an Attributes plugin with the agreement hash/folio/amount/dates whose authority is
   `Authority::None`, and an `AppData` adapter with `data_authority = pool PDA` to append each pagaré hash
   at `draw`. This is Huma's shape (Core asset + PDA sidecar), and the CreditLine stays the sidecar.
   One asset per line rather than per pagaré: pagarés are already bound to draws inside the line,
   one asset keeps the sale atomic, and it avoids 0.0015 SOL non-refundable fee per draw.
   Burn on `Repaid` (or transfer to borrower as a paid receipt), keep frozen on `WrittenOff`.
3. **Do not pick Token-2022** for this: twice the accounts and rent, metadata lock is all-or-nothing, and
   the obvious `NonTransferable` setting blocks the loan-sale path.
4. **Light compression**: not now; no privacy gain and an extra indexer/forester dependency.
5. Whatever the token, keep the signed documents off chain and the `uri` (if any) pointing to public,
   non-personal JSON; the on-chain hash is what makes a dead URI harmless.

### Ten most important citations

1. `elote-contracts: programs/elote_pool/src/instructions/lines.rs:78,94` (agreement_hash checked non-zero and stored).
2. `elote-contracts: programs/elote_pool/src/state.rs:96-134` (`CreditLine`, no per-draw hash, no holder).
3. `mpl-core: programs/mpl-core/src/plugins/internal/permanent/permanent_freeze_delegate.rs:41-77` (freeze blocks transfer/burn; only at creation).
4. `mpl-core: programs/mpl-core/src/plugins/internal/permanent/permanent_transfer_delegate.rs:40-51` + `plugins/lifecycle.rs:729-745` (force-approve beats freeze).
5. `mpl-core: programs/mpl-core/src/processor/transfer.rs:100-112` (owner-managed delegates reset on transfer).
6. `mpl-core: programs/mpl-core/src/plugins/internal/authority_managed/immutable_metadata.rs:23-31` + `plugins/lifecycle.rs:154-160` (ImmutableMetadata covers name/uri only).
7. `mpl-core: programs/mpl-core/src/plugins/external/app_data.rs:11-22` + `processor/write_external_plugin_adapter_data.rs:184-187` (append-only data by a fixed data authority).
8. `mpl-core: programs/mpl-core/src/state/collect.rs:9-17` + `utils/account.rs:10-35` (0.0015 SOL fee; burn keeps 1 byte).
9. `token-2022: program/src/processor.rs:387-399,486-494` (frozen and NonTransferable checks precede the permanent-delegate path).
10. `huma: dist/solana/idl/huma.json:2220-2305` (`create_receivable`: Core asset + `["receivable_info", asset]` PDA) and `credix: dist/idl/credix.d.ts:8673-8695` (plain PDA with hashed tax IDs).
