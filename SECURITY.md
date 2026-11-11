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

Never test with an asset that has real value, not even on a "test mainnet-beta". The devnet allow-list holds only sandbox and faucet mints (Etherfuse sandbox MEXe `MEXeDZ8U…` and USDC `BXTou3…`, Circle's devnet USDC `4zMMC9sr…`; the sandbox identifiers come from Etherfuse's `GET /supported_assets` and can change — re-check before a redeploy); the `localnet` feature (tests and the local e2e) accepts any mint and is never deployed.

## Roles and permissions

| Role | How it is identified | What it can do |
|---|---|---|
| Council | `Pool.council`: vault 0 of the Squads v4 multisig `Pool.multisig`. Every council instruction re-checks the multisig account (owner = Squads, vault 0 at the stored canonical bump, no config authority, at most ten members, threshold greater than half the voting members) and refuses a decision past its `valid_until` or dated more than 15 days ahead | Approve or reject line requests; suspend and resume draws; cancel an undrawn line; propose, withdraw and fire an acceleration; write off; endorse a pagaré en procuración, revoke it, annotate court acts and receipts, and annul a título issued in error; publish the FIX (any move, the first one, same-day corrections, dated at most five days back); name the FIX feeder; name the cotejo keys; set the FIX bounds, the line cap, the outstanding cap, the acceleration notice, the collection period and the provision; pause and resume the pool; propose a new council |
| Council members | The members of `Pool.multisig` | Nothing alone (decision 39); together, through Squads votes, they act as the council |
| FIX feeder | `Pool.fx_feeder`, set by the council (the zero key for none) | `post_fix` for a later date within 5% a day of the FIX in force (one to three days, so at most 15%) and within the pool's FIX bounds, once per Mexico City day; nothing else |
| Cotejo | One of `Pool.cotejo_keys` (up to three; the zero key an empty slot), set by the council with `set_cotejo_keys` (decision 56) | Co-sign `draw` while the pool names any key: the lender's automatic service signs only after checking the pagaré text, its hash and the borrower's signature, which the program cannot see. Nothing else |
| Creator | `Pool.creator`, the payer of `init_pool` and a seed of the pool's address | Nothing after `init_pool` |
| Borrower | `CreditLine.borrower`, the signer of `request_line` | Request a line, withdraw its request, draw tranches at the FIX it signs while the window is open (with a cotejo key's co-signature when the pool names any); a paid pagaré is delivered to it once the line owes nothing (`deliver_pagare`, callable by anyone, decision 49), and as the delivered título's owner it may burn the asset |
| Sponsor | `CreditLine.sponsor`, an optional co-signer of `request_line` | Nothing after the request; it is a recorded party |
| Line payer | `CreditLine.payer`, the funder of the request's rent (Elote's backend in the product) | Receives the rent back when a request is rejected or withdrawn |
| Procurador | `Pagare.procurador`, written by `endorse_pagare`, cleared by `revoke_procuracion` and by the settling payment | Nothing over the asset: it collects through `repay`, like any payer, and litigates with the registry's entry (decision 38). It can never be the pool, the council, the pool's creator, the FIX feeder, the line's borrower, rent payer or sponsor, or a multisig member |
| Depositor | Any signer holding the pool asset | `deposit` and `redeem` at the NAV, subject to the pause and the vault's liquidity |
| Anyone | No check | `repay` any drawn line (as payer), `mark_in_collection` once a scheduled line has missed `max_missed` installments, past the due date of the last of them |
| Upgrade authority | The BPF loader's upgrade authority for the program id | Replace the program |

## Money flows

The program moves tokens in exactly four places, and nowhere else:

| Instruction | From | To |
|---|---|---|
| `deposit` | the depositor's account | the pool's vault (and it mints shares) |
| `repay` | the payer's account | the pool's vault (`address = pool.vault`) |
| `draw` | the vault | the borrower's own token account (owner = `CreditLine.borrower`) |
| `redeem` | the vault | the holder's account (and it burns shares) |

Every other transfer is rent (SOL): a rejected or withdrawn request returns its rent to the line's payer; `draw` sweeps stray lamports from the pagaré addresses to the rent payer; `burn_pagare` returns the burnt asset's rent to its owner. A payment therefore always lands in the pool's vault. There is no instruction through which Elote's backend, or anyone, moves the borrower's USDC: the backend can only be a `repay` payer with its own funds (a SPEI payment converted to USDC) or the rent payer of a request.

## Personal data on chain

Nothing the program or the scripts write on chain names a person. Accounts and events hold keys (pseudonyms), amounts, dates and hashes; no event has a free-text field. The Core asset is named `PAGARE <line_id>-<draw_index>`, its URI is empty, and it carries nine attributes (pagaré hash, the hash of the message the borrower signed, amount in MXN, currency, issue and due dates, line, draw index, signature method). The certificate fingerprint is stored only in the pagaré record, as a salted commitment. The record also holds the hash and the 64-byte signature the draw gives (decision 55), meant to be the plain SHA-256 of the pagaré message the borrower signed and the borrower's ed25519 signature over it; the program checks neither (see Trust assumptions). No text, but anyone holding the exact text can match the hash, so the app should put a random nonce inside the signed message. It also holds the key of the cotejo service that co-signed (decision 56; a service key, not a person's) and the sealed copy's hash (decision 57), a hash of encrypted content.

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

- **The council is trusted with the vault, within caps.** Each line is capped at `max_line_bps` of the NAV at its approval (converted to MXN at a FIX at most five days old) and again at every draw, where the line's USDC drawn, the tranche included, stays within `max_line_bps` of the NAV at that moment; the pool at `max_outstanding_principal` of USDC cost at every draw, a curator cannot be a borrower (`BorrowerIsCurator`), and every decision carries `valid_until`. A council majority can still approve lines to wallets it controls up to those caps.
- **Squads v4 is a dependency.** The council's votes, proposals and execution live in Squads (program `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`, immutable on mainnet). The program reads the Multisig account at fixed offsets; `tests/squads.rs` runs the real mainnet binary (sha256 pinned) so a layout change fails the suite. Squads proposals never expire, hence `valid_until`.
- **The FIX feeder is trusted within the band.** The feeder publishes only Banxico's FIX (source fixed: the SIE; every `FixPosted` carries the date and the slot and can be checked against the DOF). Each publication of the feeder moves the FIX at most 5% for each calendar day between its date and the date of the FIX in force, counting one to three days (15% at most), only forward in date, once per Mexico City day (not on a day the feeder or the council already published, `FixAlreadyPostedToday`: a feeder behind by several days publishes today's date alone), and never outside `Pool.min_fix_rate ..= Pool.max_fix_rate` when the council has set them; the council publishes the first FIX, larger moves and same-day corrections, never dated more than five days back, and can rotate the feeder. Each feeder publication must also sit within 2% of the market reference, RedStone's MXNe price on Solana (`fx::REFERENCE_ACCOUNT`, owned by RedStone's adapter `fx::REFERENCE_PROGRAM`), at most 26 hours old; the council publishes without that check, and every `FixPosted` records the reference. The borrower signs the FIX at every draw (`FixMismatch` if it changed), but sees it in the same app.
- **On mainnet the reference check is mandatory.** A build without a cluster feature refuses a feeder publication unless the reference account exists, is owned by RedStone's adapter and parses as its MXNe `PriceData`. RedStone's account exists on mainnet only, so `localnet` and `devnet` builds skip the check when the account is missing; only a mainnet build may hold real value.
- **RedStone is a dependency of the feeder.** The reference is the price of the MXNe token from a single source (Etherfuse's API, signed three of five by RedStone's primary signers); it never sets the FIX. If it stops or drifts, the feeder is refused (`FxReferenceStale`, `FixOffReference`) and the council publishes. The program reads the account at fixed offsets pinned against the adapter's `PriceData` layout; a layout change refuses the feeder (`FxReferenceInvalid`) rather than misreading it.
- **The investors carry the USD/MXN risk.** Lines are in MXN and the vault in USDC; the NAV is at historical cost, so a weaker peso is realised as a loss when the borrower pays (`FxRealized`), not before. A USDC depeg is absorbed by the pool. Nothing in the program suspends payment in USDC, and `repay` charges no conversion fee.
- **The upgrade authority is trusted with the program.** Until it is a multisig and builds are verified, one key can replace the program.
- **The pool asset is trusted, within a policy.** `init_pool` accepts only the cluster's allow-list and refuses dangerous Token-2022 extensions and an asset mint equal to the share mint. A freeze authority and `Pausable` are accepted and recorded in `Pool.asset_controls`.
- **A cotejo key is trusted to refuse, never to pay** (decision 56). While the pool names cotejo keys, a draw needs one of them co-signing (`CotejoRequired` otherwise, checked before anything is written), and the pagaré records which key it was. The key can only add or withhold that signature: the borrower still signs, the cotejo account is read only and pays nothing, and every amount, the FIX, the recipient (the borrower's own token account) and every cap are the program's checks as before. A cotejo key therefore cannot draw, move funds, choose an amount or a recipient, or change any record. What a compromised cotejo key can do: co-sign a draw the borrower signed, so the protection it adds (the off-chain check that the text, its hash and the signature agree) is lost until it is replaced; the draw itself is still the borrower's and within the caps. What a lost or failing key does: it blocks draws while it is the only one; up to three keys run side by side. The council rotates them as a set with `set_cotejo_keys`, also while the pool is paused, and all zero lifts the requirement. `set_cotejo_keys` refuses the pool, the council or the program as a key (`CotejoKeyInvalid`), so the council's own vault is named no cotejo (a later rotation of the council to a key already in the set is not re-checked: harmless, the council controls both); another vault of the council's multisig can be named and co-sign a draw through Squads as an incident fallback (ARCHITECTURE §4.2).
- **The program does not verify the signed pagaré** (decision 55). `draw` stores `signed_pagare_hash` and `pagare_signature` as given, refusing only an all-zero one (`PagareSignatureMissing`): it never checks the signature against the borrower's key, nor against the hash, and never sees the text. In a pool that names no cotejo key a borrower can therefore record any non-zero hash and any 64 bytes, which may match no text at all; the draw is still the borrower's own signed transaction, within the caps. Only a cotejo service (decision 56) that checked the text, its hash and the signature before co-signing, or anyone checking off chain with the client's `verifyPagareSignature` and the text, gives assurance that a record's signature is good.
- **The pagaré signature has no domain separation, by design.** The borrower's wallet key signs the pagaré text itself (the hash commits to the exact text, a legal decision), and the signature is published on chain; if the signed bytes were ever a serialized Solana transaction message, the signature would authorize that transaction. The client's `assertPagareMessageText` refuses anything that is not plain text (empty, invalid UTF-8, a control character other than `\n`, a C1 control, a zero-width or bidi mark, a bidi isolate, the line or paragraph separator, the BOM), which a transaction message, with its binary header and 32-byte blockhash, cannot pass; `signPagare` (and so `draw.ts --pagare-text`) runs it before signing, and `verifyPagareSignature` is false for such bytes. The guard is the client's own; nothing relies on how a wallet's `signMessage` treats such input. Freezing the signed format without a prefix is a decision to revisit consciously, not a default.
- **An approved council proposal outlives a rotation.** Squads proposals never expire, and the program checks only each decision's `valid_until` (at most 15 days ahead). A proposal approved but not executed, for any council setter, can still be executed inside that window and re-install the old value: an older `set_cotejo_keys` naming a key since compromised, an older FIX feeder, older caps. During an incident rotation the council cancels or rejects every pending proposal in Squads before, or together with, the rotation, and keeps `valid_until` short on cotejo-key proposals (the scripts set one day). The program does not refuse a borrower, a multisig member or the FIX feeder as a cotejo key (`set_cotejo_keys` refuses only the pool, the council and the program): the council must never name one, since a borrower named as cotejo co-signs its own draws.
- **The sealed copy's hash is the parties' statement** (decision 57): `Pagare.sealed_hash` is stored as the draw gives it, opaque and never rewritten; the program checks nothing about it, and zero means no sealed copy.
- **Metaplex Core is a dependency.** The pagaré assets live in a bare Core collection whose update authority is the pool PDA; `tests/core_capabilities.rs` pins what the pinned Core release lets each party do.

## Pause

Pause blocks `draw`, `deposit`, `request_line`, `approve_line` and `redeem`. `repay` is never paused, so a borrower can always pay; the pagaré instructions, collection, write-off, FIX publication and the council's setters (`set_cotejo_keys` included, so a key can be rotated in an incident) are not paused either. Only the council pauses, by a Squads vote like any other decision, and only the council resumes (decision 39; both with `valid_until`). No single key — member or otherwise — can pause alone.

## Errors from two programs

Squads also numbers its errors from 6000 (its `NotAMember` is 6005, the same number as `CouncilTooLarge`). Inside a Squads execution an error code alone is ambiguous: the council tooling and the web decode an error by the program that raised it, read from the invoke stack in the logs (`scripts/lib/cli.ts`).

## Centralization risks

| Risk | Effect | Mitigation in place / planned |
|---|---|---|
| Council majority compromised | Lines to controlled wallets up to the caps; a pause, a write-off, or a quita of principal (decision 51: a loss recognised at once, the títulos released to the borrower) | Majority threshold checked on chain, at most ten members, caps, `valid_until`; KYC of curators off chain |
| Lost keys in a 2-of-2 council | The council cannot act until a member is replaced, which itself needs both | Add a third curator (the first council is 3 members) |
| Pause with no exit | While paused, depositors cannot redeem and borrowers cannot draw; repayments still arrive | Both the pause and the resume take a council vote (decision 39); written procedure in the runbook |
| Single upgrade key | The program can be replaced by one key | Squads multisig as upgrade authority; verified builds |

## Known risks by design

Documented limits, not open vulnerabilities; most are pinned by `programs/elote_pool/tests/known_limits.rs`.

- **Step-function NAV at historical cost, dual-priced.** Interest counts when collected, an FX gain or loss when principal is repaid. A line entering collection provisions its whole outstanding cost in the redemption NAV at the mark (decision 40, `provision_bps` = 100% at `init_pool`, council-set), so a redemption between the mark and the write-off already prices the expected loss; a recovery reverses its share, and the write-off books what remains. Deposits price at the NAV with no provision (decision 40b, Maple's dual exchange rate): an entrant buys any impaired book at par, so depositing just before a recovery — flash-amplified or patient — nets zero or less. The entrant's cost is the impaired fraction at par, bounded by the sell-or-write-off policy (3–12 months); the residuals that stay are redeeming before a foreseeable mark (inherent to objective marking) and the pro-rata share of cash-basis interest, both documented here.
- **A written-off claim stays in the price deposits pay** (decision 51): `write_off` takes a line's cost out of the book but keeps it in `Pool.recoverable`, which `entry_nav` counts, until it is recovered or the council condones it. A deposit made before a recovery therefore bought the claim at par and takes none of the principal recovered (`tests/quita.rs::a_deposit_before_a_recovery_takes_none_of_it`): the debtor cannot deposit, repay and redeem to take its own payment back. The other side of it: while a pool carries a written-off claim, a new depositor pays for that claim and can redeem at once only its share of the rest (`limit_after_a_total_write_off_a_deposit_buys_the_claim_at_par`); the front end must show both prices. Holders who see a principal quita proposed on an active line can still redeem before it executes, as before a write-off: the council pauses first.
- **A written-off line keeps accruing late interest** (decision 51): the write-off is the pool's loss, not the debtor's release, so a recovery pays the late interest run since, within the lifetime cap of the principal, unless the council condones it in the settlement.
- **A quita is the council's alone and recognises its loss at once**: no on-chain check ties `document_hash` to a signed record or the amounts to a policy; the record's hash and the `LineCondoned` event are the audit trail.
- **Collection can be unreachable** on a short schedule (`covered + max_missed > installments`); such a line is written off after the term.
- **Late and ordinary interest are computed at the second of the repayment** (decision 48: ordinary interest accrues per second on the principal outstanding), so clients pad a payment meant to settle a line; the program takes only what is owed.
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
