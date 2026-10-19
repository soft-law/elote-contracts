# Security policy

`elote_pool` holds depositor funds. If you find a vulnerability, please report it privately:

- GitHub: https://github.com/soft-law/elote-contracts/security/advisories/new
- Do not open a public issue for a security problem.

We acknowledge reports within 72 hours and keep you informed while we fix and deploy. The program embeds this policy on chain through `security.txt` (`solana-security-txt`).

## Scope

- `programs/elote_pool` (the on-chain program, program id `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM`)
- `clients/js` (`@softlaw/elote-pool-client`)

## Deployments

See "Deployments and upgrade authority" in the README for the deployed clusters, the upgrade authority and how to verify the on-chain build against this source. The program is deployed on devnet only and holds test tokens. It has not been audited.

## Roles and permissions

| Role | How it is identified | What it can do |
|---|---|---|
| Pool admin | `Pool.admin`, a signer fixed at `init_pool` and part of the pool's address; it cannot be changed | `open_line` (approve a credit line of any size to any borrower, itself included), `write_off` (recognise a loss once the line is overdue past the grace), `set_paused` |
| Borrower | `CreditLine.borrower`, a co-signer of `open_line` | `draw` the whole principal of its approved line before the term ends |
| Sponsor | `CreditLine.sponsor`, an optional co-signer of `open_line` | Nothing after the opening; it is a recorded party, not an authority |
| Depositor | Any signer holding the pool asset | `deposit` and `redeem` at the current net asset value, subject to the pause and to the vault's liquidity |
| Anyone | No check | `repay` any drawn line, `mark_in_collection` once a scheduled line has missed `max_missed` installments plus the grace |
| Upgrade authority | The BPF loader's upgrade authority for the program id | Replace the program. Today a single key; see the README |

No instruction closes a `Pool` or a `CreditLine` account, and no instruction moves tokens out of the vault other than `draw` (to the borrower named on an admin-approved line) and `redeem` (to the holder burning shares).

## Trust assumptions

- **The pool admin is trusted with the whole vault.** Depositors lend to whoever the admin approves. There is no cap on a line's principal, no exposure limit, no allowlist of borrowers and no separation between the admin and a borrower. A compromised or dishonest admin can open a line to its own wallet and draw every token in the vault.
- **The upgrade authority is trusted with the program.** Until the authority is a multisig and builds are verified, the deployed program can be replaced by one key.
- **The pool asset is trusted.** `init_pool` accepts any classic SPL mint. The asset's freeze authority, if any, can freeze the vault; the asset's mint authority can inflate the supply. Token-2022 mints are rejected by the token-program constraint.
- **Depositors accept first-come-first-served liquidity.** Redemptions are paid from the vault balance only; principal lent out is part of the NAV but is not liquid until it is repaid.
- **The agreement hash is an opaque commitment.** The program does not verify what the hash stands for; the agreements service that produced it does.

## Centralization risks

| Risk | Effect | Planned mitigation |
|---|---|---|
| Single admin key | Loss or compromise of the key: the vault can be drained (open a line to self, draw), or the pool can be paused forever | Multisig as admin; operator and pauser roles with caps; two-step origination so the borrower signs separately |
| Admin is immutable | The admin is part of the pool PDA seeds, so it cannot be rotated; a compromised pool must be abandoned and a new one created | Roles account with two-step handover |
| Pause with no exit | While paused, depositors cannot redeem; repayments still arrive. A lost admin key while paused locks every depositor | Pauser role that cannot unpause, multisig unpause, a written procedure |
| Single upgrade key | The program can be replaced by one key | Squads multisig as upgrade and IDL authority; verified builds |

## Known risks by design

These are documented limits of the current program, not open vulnerabilities. All but the missing cancel instruction are pinned by a test in `programs/elote_pool/tests/known_limits.rs`; the planned changes are on the docs site under "Roadmap and limits".

- **Step-function NAV.** Interest counts when it is collected and a loss when it is written off, so the share price moves in steps. A deposit placed just before a known repayment captures part of that interest; a redemption between `mark_in_collection` and `write_off` exits at the pre-loss value and leaves the loss to the remaining holders.
- **No cancel.** There is no instruction to revoke an approved line: it is an option for the borrower to draw until the term ends.
- **No recovery after write-off.** A written-off line takes no repayment. If every line is written off and the NAV reaches zero while shares exist, `deposit` fails until value is transferred into the vault directly.
- **Collection can be unreachable.** `mark_in_collection` requires `covered + max_missed <= installments`; on a short schedule a borrower who paid early installments and then stopped can only be written off after the term.
- **Decimals.** The first-deposit minimum is a fixed 1,000,000 base units, meant as 1.0 of a six-decimal asset.
- **Late interest timing.** Late interest is computed at the second of the repayment, so a client must pad a repayment that is meant to settle it, or the program rejects it with `LateInterestUnpaid`.

## What the program does enforce

- Every account is typed and owner-checked by Anchor; the pool, the vault, the share mint, the dead-share account and every line are PDAs validated by seeds and stored bumps; token accounts are checked against the pool's mints and the signer; CPI targets are the SPL Token program only; after each CPI the balances read for events are reloaded.
- All arithmetic is checked; products go through `u128`; both share-accounting legs round down in the pool's favour; late interest is capped at the overdue amount per settlement and at the principal over the line's life.
- The first deposit locks dead shares and must meet a minimum, so a dust deposit cannot set the opening share price and a donation cannot round later deposits to zero.
- Events are emitted through `emit_cpi!` (self-CPI), not logs, with `nav_after` on every money event.
