# Security policy

`elote_pool` holds depositor funds. If you find a vulnerability, please report it privately:

- GitHub: https://github.com/soft-law/elote-contracts/security/advisories/new
- Do not open a public issue for a security problem.

We acknowledge reports within 72 hours and keep you informed while we fix and deploy. The program embeds this policy on chain through `security.txt` (`solana-security-txt`).

## Scope

- `programs/elote_pool` (the on-chain program, program id `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`)
- `clients/js` (`@softlaw/elote-pool-client`)
- `scripts/` (the council and operator tooling)

## Deployments

See "Deployments and upgrade authority" in the README for the deployed clusters, the upgrade authority and how to verify the on-chain build against this source. The program is deployed on devnet only and holds test tokens. A deployed build is verified byte for byte against a build of this source with its cluster's feature (no feature for mainnet, `devnet` for devnet); a mainnet build that forgets its feature still gets the strictest asset list.

The v1 program was audited on 2026-11-01 and re-audited on 2026-11-02 (`tests/audit.rs`, `tests/reaudit.rs` hold the regression tests). The council redesign (PR 4) has not been audited yet; see "Not final" at the end of this file.

## Testing assets

Never test with an asset that has real value, not even on a "test mainnet-beta". The devnet allow-list holds only sandbox and faucet mints (Etherfuse sandbox MEXe `MEXeDZ8U…` and USDC `4D4r1KuS…`, Circle's devnet USDC `4zMMC9sr…`); the `localnet` feature (tests and the local e2e) accepts any mint and is never deployed.

## Roles and permissions

| Role | How it is identified | What it can do |
|---|---|---|
| Council | `Pool.council`: vault 0 of the Squads v4 multisig `Pool.multisig`. Every council instruction re-checks the multisig account (owner = Squads, vault 0 at the stored canonical bump, no config authority, at most ten members, threshold greater than half the voting members) and refuses a decision past its `valid_until` or dated more than 15 days ahead | Approve or reject line requests; suspend and resume draws; cancel an undrawn line; propose, withdraw and fire an acceleration; write off; endorse a pagaré en procuración, revoke it, annotate court acts and receipts, and cancel a paid pagaré as its holder; publish the FIX (any move, the first one, same-day corrections); name the FIX feeder and the guardian; set the line cap, the outstanding cap, the acceleration notice, the collection period and the borrower's cancel grace; resume the pool; propose a new council |
| Council members | The members of `Pool.multisig` | Each one alone can `pause` the pool (voter or not); together, through Squads votes, they act as the council |
| Guardian | `Pool.guardian`, set by the council (the zero key for none) | `pause` only |
| FIX feeder | `Pool.fx_feeder`, set by the council (the zero key for none) | `post_fix` for a later date within ±5% of the FIX in force; nothing else |
| Creator | `Pool.creator`, the payer of `init_pool` and a seed of the pool's address | Nothing after `init_pool` |
| Borrower | `CreditLine.borrower`, the signer of `request_line` | Request a line, withdraw its request, draw tranches alone at the FIX it signs while the window is open, and cancel a pagaré carrying the PAGADO mark alone once the `cancel_grace_seconds` in force at the payment (30 to 365 days, fixed on the pagaré) have passed since it |
| Sponsor | `CreditLine.sponsor`, an optional co-signer of `request_line` | Nothing after the request; it is a recorded party |
| Line payer | `CreditLine.payer`, the funder of the request's rent (Elote's backend in the product) | Receives the rent back when a request is rejected or withdrawn |
| Procurador | `Pagare.procurador`, written by `endorse_pagare`, cleared by `revoke_procuracion` and the extinction | `cancel_pagare` on that pagaré alone, while the program signs for the burn delegate `["procurador", pagare, procurador]`. It can never be the pool, the council, the guardian, the pool's creator, the FIX feeder, the line's borrower, rent payer or sponsor, or a multisig member |
| Depositor | Any signer holding the pool asset | `deposit` and `redeem` at the NAV, subject to the pause and the vault's liquidity |
| Anyone | No check | `repay` any drawn line (as payer), `mark_in_collection` once a scheduled line has missed `max_missed` installments plus the grace |
| Upgrade authority | The BPF loader's upgrade authority for the program id | Replace the program |

## Money flows

The program moves tokens in exactly four places, and nowhere else:

| Instruction | From | To |
|---|---|---|
| `deposit` | the depositor's account | the pool's vault (and it mints shares) |
| `repay` | the payer's account | the pool's vault (`address = pool.vault`) |
| `draw` | the vault | the borrower's own token account (owner = `CreditLine.borrower`) |
| `redeem` | the vault | the holder's account (and it burns shares) |

Every other transfer is rent (SOL): a rejected or withdrawn request returns its rent to the line's payer; `draw` sweeps stray lamports from the pagaré addresses to the rent payer; `cancel_pagare` returns the burnt asset's rent to its payer. A payment therefore always lands in the pool's vault. There is no instruction through which Elote's backend, or anyone, moves the borrower's USDC: the backend can only be a `repay` payer with its own funds (a SPEI payment converted to USDC) or the rent payer of a request.

## Personal data on chain

Nothing the program or the scripts write on chain names a person. Accounts and events hold keys (pseudonyms), amounts, dates and hashes; no event has a free-text field. The Core asset is named `PAGARE <line_id>-<draw_index>`, its URI is empty, and it carries eight attributes (pagaré hash, amount in MXN, currency, issue and due dates, line, draw index, signature method). The certificate fingerprint is stored only in the pagaré record, as a salted commitment.

What keeps a defaulted borrower from being identified is operational, and these rules apply in any case:

- One borrower key per line: the app derives a separate wallet per line, so a borrower's lines are not linkable on chain through a shared key. The app never publishes a wallet.
- A repayment that arrives by SPEI is paid on chain by the backend's payer key, never by the borrower's wallet from personal funds.
- The sponsor's key is not reused across lines and is never published next to the sponsor's name.
- One procurador key per case (per pagaré or line), never reused: a shared key would let anyone enumerate every line in collection from it. `endorse-pagare.ts` makes a fresh key per case and warns when an explicit key already holds a pagaré under procuración.
- Salted hashes: `receipt_hash`, `agreement_hash` and the certificate commitment are `sha256(salt || document)`, with the salt in the expediente, so a low-entropy document (a SPEI CEP, a public e.firma certificate) cannot be confirmed by guessing.
- The production collection is created with a neutral name.
- Closing terminal accounts reduces what `getProgramAccounts` lists, but the transaction history stays on the ledger. Which accounts can be closed is still to be decided; pagaré records stay (the PAGADO mark protects the debtor).

The on-chain default data model is not final and may change before the first production credit.

## Trust assumptions

- **The council is trusted with the vault, within caps.** Each line is capped at `max_line_bps` of the NAV at its approval (converted to MXN at the FIX), the pool at `max_outstanding_principal` of USDC cost at every draw, a curator cannot be a borrower (`BorrowerIsCurator`), and every decision carries `valid_until`. A council majority can still approve lines to wallets it controls up to those caps.
- **Squads v4 is a dependency.** The council's votes, proposals and execution live in Squads (program `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`, immutable on mainnet). The program reads the Multisig account at fixed offsets; `tests/squads.rs` runs the real mainnet binary (sha256 pinned) so a layout change fails the suite. Squads proposals never expire, hence `valid_until`.
- **The FIX feeder is trusted within the band.** The feeder publishes only Banxico's FIX (source fixed: the SIE; every `FixPosted` carries the date and the slot and can be checked against the DOF). A compromised feeder can move the FIX at most ±5% per publication and only forward in date; the council publishes the first FIX, larger moves and same-day corrections and can rotate the feeder. The borrower signs the FIX at every draw (`FixMismatch` if it changed), but sees it in the same app.
- **The investors carry the USD/MXN risk.** Lines are in MXN and the vault in USDC; the NAV is at historical cost, so a weaker peso is realised as a loss when the borrower pays (`FxRealized`), not before. A USDC depeg is absorbed by the pool. Nothing in the program suspends payment in USDC, and `repay` charges no conversion fee.
- **The upgrade authority is trusted with the program.** Until it is a multisig and builds are verified, one key can replace the program.
- **The pool asset is trusted, within a policy.** `init_pool` accepts only the cluster's allow-list and refuses dangerous Token-2022 extensions and an asset mint equal to the share mint. A freeze authority and `Pausable` are accepted and recorded in `Pool.asset_controls`.
- **Metaplex Core is a dependency.** The pagaré assets live in a bare Core collection whose update authority is the pool PDA; `tests/core_capabilities.rs` pins what the pinned Core release lets each party do.

## Pause

Pause blocks `draw`, `deposit`, `request_line`, `approve_line` and `redeem`. `repay` is never paused, so a borrower can always pay; the pagaré instructions, collection, write-off and FIX publication are not paused either. Any member of the council multisig (voter or not), the guardian, or the council can pause alone; only the council resumes (`resume`, with `valid_until`).

## Errors from two programs

Squads also numbers its errors from 6000 (its `NotAMember` is 6005, the same number as `CouncilInvalid`). Inside a Squads execution an error code alone is ambiguous: the council tooling and the web decode an error by the program that raised it, read from the invoke stack in the logs (`scripts/lib/cli.ts`).

## Centralization risks

| Risk | Effect | Mitigation in place / planned |
|---|---|---|
| Council majority compromised | Lines to controlled wallets up to the caps; a pause or a write-off | Majority threshold checked on chain, at most ten members, caps, `valid_until`; KYC of curators off chain |
| Lost keys in a 2-of-2 council | The council cannot act until a member is replaced, which itself needs both | Add a third curator (the first council is 3 members) |
| Pause with no exit | While paused, depositors cannot redeem and borrowers cannot draw; repayments still arrive | Any member can pause, only the council resumes; written procedure in the runbook |
| Single upgrade key | The program can be replaced by one key | Squads multisig as upgrade authority; verified builds |

## Known risks by design

Documented limits, not open vulnerabilities; most are pinned by `programs/elote_pool/tests/known_limits.rs`.

- **Step-function NAV at historical cost.** Interest counts when collected, a loss when written off, an FX gain or loss when principal is repaid; a redemption between a missed installment and the write-off exits before the loss. A provision at the mark is reserved in the layout (`impaired_principal`, `provision_bps`, zero) and not decided.
- **Collection can be unreachable** on a short schedule (`covered + max_missed > installments`); such a line is written off after the term.
- **Late interest is computed at the second of the repayment**, so clients pad a payment meant to settle it.
- **Exact-second gates on a leader-set clock** can move by a few seconds.
- **A concurrent payment makes an in-flight repayment retry** (`PagareAccountsMismatch`); the client re-reads and retries.
- **`init_pool` and `draw` compute vary with PDA bump searches** (1,500 CU per failed try); `init_pool` is pinned under 70,000.

## What the program does enforce

- Every account is typed and owner-checked; the pool, the vault, the share mint, the dead-share account, every line and every pagaré record are PDAs validated by seeds and stored bumps; token accounts are checked against the pool's mints and their owners; the CPI targets are the token programs (`transfer_checked`), the system program and Metaplex Core. The program never invokes Squads or a caller-chosen program.
- Every money instruction ends with `Pool::check()`: the NAV fits a `u64` and the share supply is zero or at least the dead shares (`InvariantBroken`).
- All arithmetic is checked; products go through `u128`; share accounting rounds in the pool's favour; FX conversions round down, and a repayment takes USDC rounded up so the pool never credits more MXN than it received.
- The first deposit locks dead shares and must meet a minimum.
- A repayment takes at most three pagaré records (`MAX_SIDECARS_PER_REPAY`, `TooManySidecars`), which keeps its heaviest shape under 100,000 CU; the client splits a larger payment.
- Events are emitted through `emit_cpi!`, with `nav_after` on every movement of the NAV and `NavUpdated` last.

## Not final

This version has not been audited. The on-chain default data model and the pagaré commitment signed at each draw may change before that audit.
