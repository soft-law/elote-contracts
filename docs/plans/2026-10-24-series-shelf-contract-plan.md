# Series shelf: contract plan (issue #25)

Date: 2026-10-24. Status: plan, not started. Source: issue #25 and its three comments (the review, the
"locked order" with the Chinampa rename, and the peer table). Checked against `main` 3c1debd.

## What the new approach asks of the program

- Every Elote series (ELOTE-CR-01, CR-02, later IN/GR/EQ) is its own `Pool` with its own NAV.
- The series list ("shelf") lives outside `elote_pool`.
- Shares of a private-placement series must only reach admitted investors, so a later secondary
  layer (Chinampa) cannot carry them past the gate.
- Chinampa, EQ, IN and GR are not built now.

## Facts from the code that shape the plan

| Fact | Where | Consequence |
|---|---|---|
| Pool seeds are `["pool", admin, asset_mint]` | `state.rs:7`, `pool.rs:40` | One pool per (admin, mint). A new series = a new admin key. No seed change needed. |
| `admin` is fixed forever; there is no `set_admin` | `lib.rs` (9 instructions, none rotates admin) | The admin must be the right key at `init_pool`. Make it a Squads vault: members rotate inside Squads, the pool never needs a rotation instruction. |
| Squads v4 gives one multisig several vaults (index 0, 1, 2, ...), each a distinct PDA | Squads docs | One Softlaw multisig can run every series: vault *n* is the admin of series *n*. Same signers, separate pools. If counsel wants different signers per SPV, use separate multisigs. |
| The share mint is classic SPL, created by the program, **freeze authority = none** | `pool.rs:44-53`, `Token` program | Shares of an existing pool can never be restricted. Permissioned shares require new pools with a new share mint. The devnet demo pool stays as it is. |
| Asset and share transfers use `anchor_spl::token` (`Token`, `token::transfer`) | `pool.rs:4`, `lines.rs` | No Token-2022 asset (CETES, MEX) and no Token-2022 share mint today. |
| `reserved` has 28 free bytes (0..4 hold the grace period) | `state.rs` Pool | Small flags fit (e.g. `permissioned: u8`); a Pubkey (32) does not. The expediente token design (2026-10-31) adds `attestor: Pubkey` and `tenedor: Pubkey` to the Pool, so pool v2 grows the Pool layout by 64 bytes in the same breaking batch (item 2d). |
| `anchor-spl` 1.2.1 already has the `token_2022` and `token_2022_extensions` features on | `Cargo.toml:31` | No new dependency for the Token-2022 work. |

## Research summary

- **Token ACL (sRFC 37)** is the Solana standard for permissioned tokens: a Token-2022 mint with
  `DefaultAccountState = Frozen`, freeze authority handed to the Token ACL `MintConfig` PDA, and a
  gate program that decides who may thaw. Ordinary transfers carry no extra cost, unlike transfer
  hooks, which many protocols blacklist. The reference gate (ABL, `token-acl-gate`) has Allow and
  Block modes only; `AllowAllEoas` is an `Unused` placeholder in the current code
  (`program/src/state/list_config.rs:73-78`, read 2026-10-30). Program
  `TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP` is on mainnet (`token-acl/README.md:5`) with an
  Accretion audit (`token-acl/audits/2025-accretion-solana-token-acl-audit-A25SFR2.pdf`); the ABL
  gate has two Accretion audits in `token-acl-gate/audit/`. Verified 2026-10-30; see the identity
  evaluation.
  https://solana.com/developers/guides/advanced/acl
- **Gating comes for free with a frozen-by-default share mint.** `mint_to` into a frozen account
  fails, so `deposit` already refuses an investor whose share account was never thawed. The
  program needs no allowlist of its own. Side effect: a frozen holder also cannot `redeem` (burn
  needs a thawed account), which is the expected behavior for a sanctions freeze.
- **Chinampa follows the same gate.** A Chinampa vault is a PDA, so it can hold series shares only
  if the gate admits that PDA. In Allow mode nothing is admitted until listed, PDAs included, so
  the issuer decides explicitly whether any secondary venue can hold a series.
- **Token-2022 group and member extensions** make a mint a group ("ELOTE") and each series share
  mint a member, with metadata (name, symbol, URI, extra key-value fields) stored on the mint.
  The shelf can be the group itself, with no registry program. Fields: `asset_class`, `status`,
  `spv_hash`, `docs_uri`. https://solana.com/docs/tokens/extensions/group-member
- **Peers (from the comments):** Maple segregated pools are the model for ring-fencing; Centrifuge
  tranches stay inside one credit book. Nothing here asks the credit logic itself to change.

### Colosseum precedents (Colosseum Copilot data, read 2026-10-24)

The closest winners have the same shape as ours, and none of them shipped live contracts for it.
That is both an opening and a warning that the hard parts are untested on Solana.

| Project | Award | What it is | What it tells us |
|---|---|---|---|
| [BORE.FI](https://colosseum.com/projects/explore/bore.fi-1) | Cypherpunk, 2nd place RWAs, $20k | Tokenized SPVs of cash-flowing SMEs, investment pools per industry | Same "shelf of SPV pools" shape as ELOTE-CR/EQ. Its submission was a frontend with mock data, with no contracts. |
| [Housd](https://colosseum.com/projects/explore/housd) | Frontier winner, accelerator C5 | Vault for residential real-estate credit with senior and junior tranches; Anchor and Squads in its stack | Tranches *inside one credit book* (Centrifuge shape), not across asset classes. A later option for CR, not for the shelf. |
| [Kormos](https://colosseum.com/projects/explore/kormos) | Cypherpunk, 2nd place DeFi, $20k, accelerator C4 | Liquid and locked depositors; a FIFO redemption queue when liquidity is short | A concrete model for the redemption queue we deferred. It is needed before Chinampa, because lent-out capital is exactly a liquidity shortfall. |
| [ODL](https://colosseum.com/projects/explore/odl-(on-demand-liquidity)) | Frontier winner, accelerator C5 | Liquidator marketplace for tokenized private credit and private equity | Colosseum funded "RWA private credit has no secondary liquidity" as a problem in its own right. Chinampa targets the same gap, and the problem is open, not solved. |
| [Crafts](https://colosseum.com/projects/explore/crafts-1) | Frontier winner | Equity-linked tokens through an SPV, sealed-bid price discovery | A shape for ELOTE-EQ: an SPV token with its own issuance, not credit lines. Supports keeping EQ out of `elote_pool`. |

Limits: Copilot's semantic search returned few relevant matches for multi-series RWA shelves;
the table comes from category-filtered winner lists plus full project records. No Mexico SME
credit project turned up. Crypto World's Fair winners are announced by Dec 5 and are not in the
data yet.

## Plan

### Phase 0: no code (do first)

1. **ADR 0001** `docs/adr/0001-series-shelf.md`. Series = separate admin plus pool; shelf =
   Token-2022 group outside `elote_pool`; shares permissioned by Token ACL for private-placement
   series; why Chinampa is opt-in and still not a mega-NAV; EQ/IN/GR described only.
2. **Ops convention.** Mainnet admin of every series = Squads vault index *n*. Document the index
   to series map. Applies from the first mainnet pool, because admin cannot change later.
3. **Client.** `listSeries()` in `@softlaw/elote-pool-client`: read the group's members, then each
   member's pool (the share mint's mint authority is the pool PDA). Works for v1 pools as well,
   provided their share mints are listed off-chain until v2 exists.

### Phase 1: hardening batch already approved in the register (unchanged)

R-11, R-21/R-27, R-24, R-25, plus a decimals-derived `MIN_FIRST_DEPOSIT` and rejecting
borrower == admin. Small, no layout change. Lands before Phase 2 so the diff stays readable.

### Phase 2: pool v2, Token-2022 (one branch, one review)

| # | Change | Size | Notes |
|---|---|---|---|
| 2a | `TokenInterface` + `transfer_checked` everywhere; `InterfaceAccount<Mint/TokenAccount>` | M | Classic and Token-2022 asset mints both work. Needed anyway for MEX or CETES. |
| 2b | **Asset-mint policy at `init_pool`** (P0-5): reject transfer fee, permanent delegate, transfer hook, non-transferable, confidential-only, and mint close authority; freeze authority per the owner's decision (reject, or accept and record) | S | Covers EF-1 and EF-3 from the Etherfuse research. Interest-bearing is allowed only for display, but CETES as a pool asset stays out (unit mismatch). |
| 2c | **Share mint supplied by the admin and validated** instead of created by the program: Token-2022, `mint_authority == pool PDA`, supply 0, decimals == asset decimals, no transfer fee, no transfer hook, no close authority, no permanent delegate (unless the owner decides one is needed for court-ordered transfers) | M | Lets the admin pre-configure metadata, group member and DefaultAccountState=Frozen plus Token ACL with standard tooling, without the program reimplementing each extension. Replaces the `["shares", pool]` PDA for v2 pools. Front-running is impossible because the mint authority must already be the pool PDA. |
| 2d | `Pool.version = 2`, flag `permissioned` in `reserved[4]`, set when the share mint has DefaultAccountState=Frozen; plus `attestor: Pubkey` and `tenedor: Pubkey` appended (64 bytes) with `set_attestor` / `set_tenedor` admin instructions, per the expediente token design | S | Read-only fact for clients and Chinampa; the two keys are the trusted roles of `docs/plans/2026-10-31-expediente-token-design.md`. |
| 2e | Dead-shares account must be thawed at init when shares are permissioned | S | Otherwise the first deposit fails. Either the gate admits the pool PDA, or `init_pool` takes a pre-thawed account. Test both paths. |
| 2f | `NavUpdated { pool, total_assets, share_supply }` event after every deposit, redeem, repay and write-off | S | For indexers, the shelf page and later Chinampa pricing. No state change. |

v1 code paths stay for v1 pools (the devnet demo). A v2 pool cannot share a (admin, mint) pair with
a v1 pool, which the ops convention already avoids.

Also in this batch: **line v3, tranched lines with proportional interest**
(`docs/plans/2026-10-31-multi-draw-plan.md`), because it grows the `CreditLine` account and should
break the layout only once.

**Tests (LiteSVM):** a Token-2022 asset; a classic asset through the interface; every rejected
extension; frozen-by-default shares (deposit fails before thaw, succeeds after); first deposit with
permissioned dead shares; two series under two Squads vault PDAs with isolated NAVs (the issue's
acceptance test); a compute check against the 100k CU ceiling (`transfer_checked` costs more).

**Then:** Codama regen, `schedule.ts` unchanged, ARCHITECTURE.md §1, §2, §12 and SECURITY.md (new
trust: Token ACL gate, metadata update authority, asset issuer freeze).

### Not in this plan (by design)

- **Chinampa program.** Wait for counsel, Phase 2 and a pricing design that marks arrears.
  Pre-requisite already captured: the pool exposes NAV, and `arrears` is computable on chain.
- **Redemption queue** (ERC-7540 style, FIFO like Kormos) for when the vault is lent out. Needed
  before any secondary venue, but not for multi-series. It is the next contract phase after
  Phase 2 and the prerequisite for Chinampa.
- **Senior/junior tranches inside a CR series** (Housd, Centrifuge). A later product option that
  stays inside one credit book.
- **EQ / IN / GR programs.** No underlying and no NAV trust model yet.
- **Seed migration and `set_admin`.** Not needed under the Squads convention.

## Decisions needed from the owner

All five were decided in the owner review of 2026-10-31
(`docs/decisions/2026-10-31-owner-review.md`):

1. CR series shares are permissioned (Token ACL) from the first mainnet pool. **Decided: yes.**
2. Token ACL with the ABL gate. **Decided: ABL in Allow mode**, audit and mainnet address verified
   2026-10-30; the SAS-reading gate is a post-audit upgrade.
3. Permanent delegate on share mints. **Decided: no**, unless counsel requires it in writing.
4. Freeze policy for the asset mint. **Decided: accept an issuer freeze authority and record it**
   (MEXe has one; CETES, read live 2026-10-31, has none). `init_pool` rejects the other
   extensions listed in 2b and records `asset_freezable` as a read-only flag.
5. Squads topology. **Decided: one multisig, one vault per series**, until counsel asks for
   separate multisigs per SPV.

## Estimate

Phase 0: 1 day. Phase 1: 1 to 2 days. Phase 2: 4 to 6 days including tests and docs, then an
external review before mainnet.
