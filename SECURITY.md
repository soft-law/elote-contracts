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
| Pool admin | `Pool.admin`, a signer fixed at `init_pool` and part of the pool's address; it cannot be changed | `open_line` (approve a credit line of up to `max_line_bps` of the NAV to any borrower other than itself, any number of times), `write_off` (recognise a loss once the line is overdue past the grace), `set_paused`, `set_attestor` (replace the attestor) |
| Attestor | `Pool.attestor`, named at `init_pool`, rotated by the admin with `set_attestor`; the agreements engine's key | Co-signs `open_line` (attesting the signed agreement matches the arguments) and `draw` (attesting the pagaré behind the hash was signed and is conserved); in step 6 also `endorse_pagare`. It can block every opening, draw and endoso by withholding its signature and cannot move funds or títulos |
| Borrower | `CreditLine.borrower`, a co-signer of `open_line` | `draw` tranches of its approved line, with the attestor, while the draw window is open (until the first due date, or the term on a bullet line) |
| Sponsor | `CreditLine.sponsor`, an optional co-signer of `open_line` | Nothing after the opening; it is a recorded party, not an authority |
| Depositor | Any signer holding the pool asset | `deposit` and `redeem` at the current net asset value, subject to the pause and to the vault's liquidity, and, on a permissioned series, to holding a thawed share account |
| Share-mint freeze authority | The freeze authority of the admin-supplied share mint (Token ACL's `MintConfig` in production, driven by the allow list the backend feeds; the program does not check who it is) | Thaw a holder's share account (admit an investor) or freeze it (bar deposits and redemptions); thaw the dead-shares account before the first deposit. It cannot move funds or shares |
| Anyone | No check | `repay` any drawn line, `mark_in_collection` once a scheduled line has missed `max_missed` installments plus the grace |
| Upgrade authority | The BPF loader's upgrade authority for the program id | Replace the program. Today a single key; see the README |

No instruction closes a `Pool` or a `CreditLine` account, and no instruction moves tokens out of the vault other than `draw` (to the borrower named on an admin-approved line) and `redeem` (to the holder burning shares).

## Trust assumptions

- **The pool admin is trusted with the whole vault.** Depositors lend to whoever the admin approves. Each line is capped at `max_line_bps` of the NAV at its opening (`LineTooLarge`; 1,000 bps, a tenth, in production pools), and `open_line` refuses the admin key as the borrower (`BorrowerIsAdmin`), so the one-key path "open one line to self, draw the vault" is closed. There is still no exposure limit per borrower across lines and no allowlist of borrowers: a compromised or dishonest admin can open several lines to wallets it controls and drain the vault a tenth at a time. The cap is measured against the NAV at the moment of `open_line`, which a deposit in the same transaction, redeemed right after, inflates; so the cap bounds honest mistakes and the size of any single line the app proposes, and is not a defence against the admin.
- **The upgrade authority is trusted with the program.** Until the authority is a multisig and builds are verified, the deployed program can be replaced by one key.
- **The pool asset is trusted, within a policy.** `init_pool` accepts a classic SPL Token or a Token-2022 mint, under the token program the caller names, and refuses a mint whose extensions take or hide value or would freeze the vault at creation: transfer fee, permanent delegate, transfer hook, non-transferable, any confidential extension, mint close authority, and a frozen default account state (`AssetMintNotAllowed`). Two issuer controls that can stop the vault without touching the program are accepted and recorded in `Pool.asset_controls` (decision 15): a freeze authority (MEXe has one) and the `Pausable` extension. The asset's mint authority can still inflate the supply; nothing on chain checks the issuer.
- **The share mint is the admin's, within a policy.** `init_pool` validates the mint the admin supplies (pool PDA as mint authority, empty supply, the asset's decimals, no fee, hook, close authority, permanent delegate, interest-bearing rate or confidential extension: `ShareMintNotAllowed`). With `DefaultAccountState = Frozen` the shares are permissioned (`PoolFlags.permissioned`, the fact anyone can check): every share account is born frozen and the mint's freeze authority decides who is thawed. The program records nothing about that authority; the first mainnet pool must be created with a Token-2022 frozen-by-default mint whose freeze authority is Token ACL's `MintConfig` (decision 15, see `docs/runbook.md`).
- **Depositors accept first-come-first-served liquidity.** Redemptions are paid from the vault balance only; principal lent out is part of the NAV but is not liquid until it is repaid.
- **The agreement hash is an opaque commitment.** The program does not verify what the hash stands for; the agreements service that produced it does.

## Centralization risks

| Risk | Effect | Planned mitigation |
|---|---|---|
| Single admin key | Loss or compromise of the key: the vault can be drained (open a line to a second wallet, draw), or the pool can be paused forever | Multisig as admin; operator and pauser roles with caps; two-step origination so the borrower signs separately |
| Admin is immutable | The admin is part of the pool PDA seeds, so it cannot be rotated; a compromised pool must be abandoned and a new one created | Roles account with two-step handover |
| Pause with no exit | While paused, depositors cannot redeem and borrowers cannot draw an approved line while its term keeps running, so a long pause can let an approved line lapse at `term_end`; repayments still arrive. A lost admin key while paused locks every depositor | Pauser role that cannot unpause, multisig unpause, a written procedure |
| Single upgrade key | The program can be replaced by one key | Squads multisig as upgrade and IDL authority; verified builds |

## Known risks by design

These are documented limits of the current program, not open vulnerabilities. All but the missing cancel instruction are pinned by a test in `programs/elote_pool/tests/known_limits.rs`; the planned changes are on the docs site under "Roadmap and limits".

- **Step-function NAV.** Interest counts when it is collected and a loss when it is written off, so the share price moves in steps. A deposit placed just before a known repayment captures part of that interest; a redemption between `mark_in_collection` and `write_off` exits at the pre-loss value and leaves the loss to the remaining holders.
- **No cancel.** There is no instruction to revoke an approved line: it is an option for the borrower to draw until the term ends.
- **No recovery after write-off.** A written-off line takes no repayment. If every line is written off and the NAV reaches zero while shares exist, `deposit` fails until value is transferred into the vault directly.
- **Collection can be unreachable.** `mark_in_collection` requires `covered + max_missed <= installments`; on a short schedule a borrower who paid early installments and then stopped can only be written off after the term.
- **First-deposit floor on low-decimal assets.** The first-deposit minimum is 1.0 of the asset, but never fewer base units than 1,000 times the dead shares (1,000,000), so on an asset with fewer than six decimals the minimum is more than one whole unit (10,000.00 on a two-decimal asset). Pinned by `first_deposit_minimum_is_one_unit_of_the_asset_with_a_dead_shares_floor`.
- **Late interest timing.** Late interest is computed at the second of the repayment, so a client must pad a repayment that is meant to settle it, or the program rejects it with `LateInterestUnpaid`.

## What the program does enforce

- Every account is typed and owner-checked by Anchor; the pool, the vault, the share mint, the dead-share account and every line are PDAs validated by seeds and stored bumps; token accounts are checked against the pool's mints and the signer; the only CPI targets are the token programs (classic SPL Token or Token-2022, through Anchor's `TokenInterface`, with `transfer_checked` carrying the mint's decimals); after each CPI the balances read for events are reloaded.
- Every money instruction (`deposit`, `redeem`, `draw`, `repay`, `write_off`) ends with `Pool::check()`: the NAV (`vault + outstanding_principal`) fits a `u64`, and where the share mint is an account (`deposit`, `redeem`, `repay`, `write_off`) its supply is zero or at least the dead shares. A failure is `InvariantBroken` and the transaction is refused. This is defence in depth: no instruction today can break either invariant, and the check is there so that a future change which does is refused on chain rather than found in an audit.
- All arithmetic is checked; products go through `u128`; both share-accounting legs round down in the pool's favour; late interest is capped at the overdue amount per settlement and at the principal over the line's life.
- The first deposit locks dead shares and must meet a minimum, so a dust deposit cannot set the opening share price and a donation cannot round later deposits to zero.
- Events are emitted through `emit_cpi!` (self-CPI), not logs, with `nav_after` on every money event.
