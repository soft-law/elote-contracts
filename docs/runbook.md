# Operations runbook: `elote_pool`

Scope: the on-chain program `elote_pool` as deployed on devnet today (`main` @ `d8d94a9`), and the
keys, scripts and people that operate it. The code in `programs/elote_pool/src` wins over this file;
README.md, SECURITY.md, docs/interface.md and docs/decisions/ give the reasons. Names and contact
details in angle brackets are placeholders to fill before the first mainnet pool.

Every command below runs from the repository root. Scripts take `--cluster local|devnet`, print the
cluster and, unless the RPC is this machine or `--yes` is given, ask before sending. `ELOTE_RPC`
overrides the RPC URL. Keys are read from files and never printed.

## 1. Roles and keys

| Role | Key | What it does | What it never does |
|---|---|---|---|
| Council | Vault 0 of the pool's Squads v4 multisig (`Pool.council`, `Pool.multisig`); 2 of 3 at the pilot, at most ten members, a majority threshold, no config authority, no time lock | Every administrative decision: approve or reject lines, suspend and resume draws, cancel an undrawn line, propose, withdraw and fire an acceleration, write off, endorse and revoke a procuración, court annotations, receipt annotations, annul a título issued in error, publish or correct the FIX, name the feeder, set the FIX bounds, the line cap, the outstanding cap, the provision, the collection period and the acceleration notice, pause and resume, hand over to a new council | Act without a vote. No member acts alone, the pause included (decision 39) |
| FIX feeder | `Pool.fx_feeder`, one key, set and rotated by the council | `post_fix` once per Mexico City day, within the band (section 2) | Anything else; it cannot be a procurador |
| Borrower | `CreditLine.borrower`, the signer of `request_line` | Requests a line, withdraws a request, draws tranches alone at the FIX it signs, receives its paid pagarés, may burn a delivered título | Approve its own line; a council member cannot be a borrower (`BorrowerIsCurator`) |
| Procurador | `Pagare.procurador`, named by `endorse_pagare` | Collects through `repay` like any payer and litigates with the registry entry | Hold any power over the asset (decision 38) |
| Elote's backend | A payer key with its own funds | Pays `repay` on a borrower's behalf (a SPEI payment converted to USDC with the backend's own funds); pays the rent of `request_line` and of each `draw`'s pagaré accounts | Move a borrower's USDC: there is no instruction for it |
| Sponsor | `CreditLine.sponsor`, optional co-signer of `request_line` | Recorded party (promoter or guarantor); may pay on the borrower's behalf | Anything after the request |
| Upgrade authority | The BPF loader's authority over the program id | Replaces the program (section 6) | Anything inside a pool |
| Anyone | No check | `repay` any drawn line, `mark_in_collection` an eligible line, `deliver_pagare` a paid pagaré | |

**Council decisions.** Every council instruction takes `valid_until` as its last argument; the program
refuses it past that second (`ProposalExpired`) or more than 15 days ahead (`ProposalTooFar`),
because Squads proposals never expire. The scripts set `valid_until` one day ahead. A decision is a
Squads vault transaction: created, proposed, approved by the threshold, executed with a 100,000 CU
limit (`scripts/lib/council.ts`). The scripts load the threshold of member keys from
`--council-keypairs <a.json,b.json>`, `ELOTE_COUNCIL_KEYPAIRS` or `.demo-keys/curator-<n>.json` and
run every step from one machine: a devnet practice. On mainnet each member approves from their own
device in the Squads app. Squads and `elote_pool` both number errors from 6000; the scripts name the
program that raised an error from the logs.

**Money flows.** The program moves tokens in four places only:

| Instruction | From | To |
|---|---|---|
| `deposit` | the depositor's account | the pool's vault (shares minted) |
| `repay` | the payer's account | the pool's vault |
| `draw` | the vault | the borrower's own token account |
| `redeem` | the vault | the holder's account (shares burned) |

Every other transfer is rent (SOL). A payment always lands in the vault, and the backend can only
be a `repay` payer with its own funds or a rent payer.

**Key hygiene.** These rules keep a defaulted borrower from being identified on chain:

- One dedicated wallet per borrower, never shared with anything else; SECURITY.md goes further and
  derives one wallet per line, so a borrower's lines are not linkable through a shared key. The app
  never publishes a wallet address.
- A repayment that arrives by SPEI is paid on chain by the backend's payer key, never from the
  borrower's wallet.
- The sponsor's key is used for that role only: never reused across roles or lines, never published
  next to the sponsor's name.
- One procurador key per case, never reused. `scripts/endorse-pagare.ts` generates it for the case
  into `.demo-keys/procuradores/<pool>-<line>-<draw>.json`; hand that file to the despacho. A reused
  key lets anyone list every line in collection from it. With `--procurador <address>` the script
  warns when the key already holds a pagaré under procuración.
- Agreement, receipt and certificate hashes are salted, `sha256(salt || document)`, with the salt in
  the expediente.
- The production Core collection of a series gets a neutral name: nothing linking it to lending,
  no series label, never "programa", "serie", "emisión" or "inversión". The pagaré assets are named
  `PAGARE <line_id>-<draw_index>` with an empty URI.
- The FIX feeder, the backend's payer, the upgrade authority and each council member are distinct
  keys. No key holds two roles.

**Creating a series.** The creator makes the share mint and a bare Metaplex Core collection (neutral
name, the pool PDA `["pool", creator, asset_mint]` as update authority), then calls `init_pool`
naming the multisig. A mainnet series uses a Token-2022 share mint with `DefaultAccountState =
Frozen` under Token ACL, so `init_pool` records `permissioned = true`; the dead-shares account
`["dead", pool]` is thawed before the first deposit. The creator has no power after `init_pool`.
`init_pool` sets the provision at 100%, the collection period at no less than 30 days and the
acceleration notice at no less than 7 days. The council then publishes the first FIX and names the
feeder (section 2).

## 2. The FIX

The FIX is Banxico's MXN/USD rate. Lines and pagarés are in MXN; the vault holds USDC. A draw delivers
USDC at the FIX in force, which the borrower signs; a repayment is credited in MXN at it.

**Source.** Banxico's SIE, exclusively. Every `FixPosted` event carries the rate, the date Banxico
determined it, the poster, the slot and the RedStone reference, and can be audited against the DOF.

**What the program accepts.**

| Rule | Feeder | Council |
|---|---|---|
| Date | Later than the FIX in force, never in the future | Not earlier than the FIX in force (a same-day correction is allowed), never in the future, at most 5 days back |
| How often | Once per Mexico City day, and not on a day the council already published (`FixAlreadyPostedToday`) | Any time |
| Move | Within 5% a day from the FIX in force, counting one to three days (15% at most); never the first FIX (`FixJump`) | Any |
| Reference | Within 2% of RedStone's MXNe price, at most 26 hours old, when the account exists; mandatory on a mainnet build (`FixOffReference`, `FxReferenceStale`, `FxReferenceInvalid`) | Not checked; recorded when readable |
| Bounds | Within `Pool.min_fix_rate ..= Pool.max_fix_rate` when set (`FixOutOfBounds`) | Same |

The RedStone account (`8QuwResPgdQdja4gdTbvNxeUznc58XX8baUM3cCwfVW2`) exists on mainnet only; the
devnet and localnet builds skip the check when it is missing. The reference never sets the FIX.

A draw and an approval need a FIX at most 5 days old (`FixStale`). A repayment takes the FIX in force
whatever its age, and the payer signs it (`FixMismatch` if it changed in between).

**Feeder SLA.** The feeder publishes within minutes of Banxico's determination of the day's FIX.
A missed day stops nothing at once; a FIX older than 5 days stops draws and approvals.

**Procedure: daily publication (feeder).**

1. Read the day's FIX from Banxico's SIE and check its date.
2. Publish:
   ```sh
   npx tsx scripts/post-fix.ts --cluster devnet --pool <pool> --fix 18.3740 --feeder-keypair <feeder.json>
   ```
   `--fix-date <yyyymmdd>` defaults to today in Mexico City by the chain clock. After a gap of
   several days the feeder publishes today's date alone. The script prints the RedStone reference and
   warns before sending when the reference is stale or more than 2% away.
3. Check the `FixPosted` event: rate, date, `by` = the feeder.

**Procedure: council publication** (the first FIX, a move beyond the band, a same-day correction, or
the feeder down).

1. A member proposes the FIX with its SIE source; the threshold approves.
2. Publish without `--feeder-keypair`:
   ```sh
   npx tsx scripts/post-fix.ts --cluster devnet --pool <pool> --fix 18.3750 [--fix-date <yyyymmdd>] --council-keypairs <a.json,b.json>
   ```
3. When the feeder is down, the council publishes within 5 days of the last FIX, so draws and
   approvals never go stale. A same-day correction by the council blocks the feeder until the next
   Mexico City day.

**When the feeder is refused.**

| Error | Meaning | Action |
|---|---|---|
| `FixJump` | The move exceeds the band, or no FIX was ever published | Check the SIE value. If correct, the council publishes it |
| `FixOffReference` | More than 2% from RedStone's MXNe price | Check the SIE value and the reference. If the FIX is right, the council publishes it and records why the reference diverged |
| `FxReferenceStale` / `FxReferenceInvalid` | The reference is older than 26 h, or not RedStone's account | The council publishes; report the reference outage |
| `FixAlreadyPostedToday` | A FIX was already published today (by the feeder or the council) | Nothing; the feeder publishes tomorrow. A wrong value today is corrected by the council |
| `FixDateInvalid` | The date is in the future, not later than the FIX in force, or not a date | Fix the date |
| `FixOutOfBounds` | Outside the council's bounds | Check the value; if Banxico's FIX really left the bounds, the council moves them and publishes |
| `Unauthorized` | The key is not `Pool.fx_feeder` | Use the feeder key, or rotate it |

**Rotating the feeder** (lost or suspect key):

```sh
npx tsx scripts/council-propose.ts --cluster devnet --pool <pool> --op set-fx-feeder --address <new feeder>
npx tsx scripts/council-propose.ts --cluster devnet --pool <pool> --op set-fx-feeder --address 11111111111111111111111111111111   # no feeder: only the council publishes
```

**FIX bounds:** `npx tsx scripts/council-propose.ts --cluster devnet --pool <pool> --op set-fix-bounds --min 15.0000 --max 25.0000`
(`--min 0 --max 0` for none).

**A pinned FIX (peso series at parity).** Bounds with `min == max` (e.g. `--min 1.0000 --max
1.0000` on a MEXe pool) pin the FIX: `post_fix` accepts exactly that rate, and approvals and draws
skip the 5-day staleness check (decision 42) — no re-post cadence, no feeder, no reference. Post
parity once at creation and leave it. What to watch instead of a price (which is 1.0 by
construction): that MEXe **redemption works** (a periodic small sandbox/mainnet redemption or the
issuer's attestations) and the mint's authority/freeze events. If redemption gates or the peg is
in doubt, run the depeg response below within 24 hours.

**Depeg response on a pinned pool** (tested:
`unpinning_restores_staleness_and_the_council_re_prices_under_pause`):

1. `pause` by council vote — deposits, redemptions, requests, approvals and draws stop; repay
   never stops.
2. `set_fix_bounds` to a working range. Unpinned, the old FIX is stale again at once, so
   approvals and draws stay blocked even after resume until step 3.
3. The council publishes the re-priced FIX (within the new bounds; the reference is not enforced
   on the council path).
4. `resume`. Repayments made in the interim were credited at the FIX in force at each payment.

## 3. Line lifecycle

| Status | Reached by | Leaves by |
|---|---|---|
| `Requested` | `request_line` | `approve_line`, `reject_line`, `withdraw_request` |
| `Approved` | `approve_line` | the first `draw` (`Active`), `cancel_line` |
| `Active` | the first `draw` | `repay` in full (`Repaid`), `mark_in_collection` or `accelerate_line` (`InCollection`), `write_off` after the term |
| `InCollection` | the mark or the acceleration | `repay` in full (`Repaid`), `write_off` |

**3.1 Request.** The borrower signs; the backend pays the account's rent; a named sponsor co-signs.
Refused while paused. Moves no money.

```sh
npx tsx scripts/request-line.ts --cluster devnet --pool <pool> --borrower-keypair <borrower.json> --principal 50000 \
  [--rate-bps 1200] [--term-days 90] [--late-rate-bps 10000] \
  [--installments 6 --interval-days 30 [--max-missed 3]] \
  [--sponsor-keypair <sponsor.json> --sponsor-role promoter|guarantor] \
  --agreement-file <agreement.pdf> --agreement-salt <64 hex> --expediente-root <64 hex> --pagare-template-hash <64 hex>
```

The script prints the line id and the terms hash the council approves. The request expires 15 days
after it was made; the borrower may withdraw it at any time, and the rent returns to the payer.

**3.2 Approval through Squads.**

1. The council members review the expediente and compute the terms hash from the documents.
2. Check the FIX in force is at most 5 days old; publish one first if not (section 2).
3. Approve:
   ```sh
   npx tsx scripts/approve-line.ts --cluster devnet --pool <pool> --line-id <n> --council-keypairs <a.json,b.json>
   ```
   The script recomputes the terms hash from the stored terms and refuses on a mismatch. The program
   refuses a line above `max_line_bps` of the NAV at the FIX in force, a borrower who is a council
   member, and an approval while paused.
4. To reject instead: `npx tsx scripts/council-propose.ts --cluster devnet --pool <pool> --op reject-line --line-id <n>`.

The approval opens the draw window: until the first due date on a scheduled line, until `term_end` on
a bullet line.

**3.3 Draws.** The borrower signs alone; a rent payer funds the pagaré asset and its record.

```sh
npx tsx scripts/draw.ts --cluster devnet --pool <pool> --line-id <n> --borrower-keypair <borrower.json> --amount 10000 \
  [--signature-method wallet|psc|efirma --cert-fingerprint <64 hex> --cert-salt <64 hex>]
```

The script prints the FIX and the USDC it delivers before asking. A draw is refused while paused,
once the window has closed (strictly before `availability_end`), on a FIX changed since the borrower
read it (`FixMismatch`) or older than 5 days (`FixStale`), beyond 64 tranches, or above what the
line, the line cap at the current NAV, the vault or the outstanding cap allow. Each draw mints one
frozen pagaré asset owned by the pool and its `Pagare` record.

The council closes a window early with `suspend_draws` (a KYC failure, a default event; the undrawn
remainder is forfeited) and reopens it with `resume_draws`. It clears an approved line nobody drew
with `cancel_line`. No script covers these three; see 3.6.

**3.4 Repayments.** Anyone may pay; never paused.

```sh
npx tsx scripts/repay.ts --cluster devnet --pool <pool> --line-id <n> (--amount <USDC> | --all) \
  [--payer-keypair <backend.json>] [--receipt-file <cep.pdf> --receipt-salt <64 hex>]
```

- The payment goes to late interest, then interest, then principal, never more than is due.
- One `repay` takes at most three pagaré records (`MAX_SIDECARS_PER_REPAY`); the script splits a
  larger payment into parts and retries a part once on `PagareAccountsMismatch` (another payment
  landed first).
- Each part carries one `deliver_pagare` per pagaré it fully covers, in the same transaction: the
  título goes to the borrower, or to the guarantor who settled it.
- A missing `--receipt-*` records a zero hash ("to follow"); the council attaches the real receipt
  later with `annotate_payment`.
- The backend pays with `--payer-keypair` and its own USDC; screen the `payer` of every `LineRepaid`.

**3.5 Pagaré delivery and burn.** A delivery missing from a repayment's transaction is re-sent by
anyone:

```sh
npx tsx scripts/deliver-pagare.ts --cluster devnet --pool <pool> --line-id <n> --draw-index <n>
```

It is refused before the pagaré is covered (`PagareNotCovered`) and twice (`PagareAlreadyDelivered`).
The owner of a delivered título may burn it and take the rent back; the record always stays:

```sh
npx tsx scripts/deliver-pagare.ts --cluster devnet --pool <pool> --line-id <n> --draw-index <n> --burn --owner-keypair <owner.json>
```

The council annuls a título issued in error with `annul_pagare` (an entry; nothing burns).

**3.6 Council decisions without a script.** `scripts/council-propose.ts` covers `set-fx-feeder`,
`set-max-line-bps`, `set-outstanding-cap`, `set-fix-bounds`, `reject-line` and `resume`. Every other
council instruction — `pause`, `suspend_draws`, `resume_draws`, `cancel_line`,
`propose_acceleration`, `withdraw_acceleration`, `accelerate_line`, `write_off`, `court_annotation`,
`annotate_payment`, `annul_pagare`, `set_provision_bps`, `set_collection_period`,
`set_acceleration_notice`, `propose_council` — and the permissionless `mark_in_collection` have no
script yet. Run them the way `council-propose.ts` does: the generated builder from
`clients/js/src/generated/instructions/<name>.ts`, with `council: council.signer`,
`multisig: pool.multisig` and `validUntil`, passed to `councilExecute` from `scripts/lib/council.ts`.
For example, as a throwaway file in `scripts/`:

```ts
import { address } from "@solana/kit";
import { getWriteOffInstructionAsync } from "../clients/js/src/index.ts";
import { chainNow, cluster, confirmCluster } from "./lib/cli.ts";
import { councilExecute, loadCouncil } from "./lib/council.ts";
import { loadLine, loadPool } from "./lib/pool.ts";

const c = cluster();
const poolAddress = address("<pool>");
await confirmCluster(c, "write_off");
const pool = await loadPool(c, poolAddress);
const { line } = await loadLine(c, poolAddress, <line id>n);
const council = await loadCouncil(c, pool.multisig);
const ix = await getWriteOffInstructionAsync({
  council: council.signer, multisig: pool.multisig, pool: poolAddress, line,
  shareMint: pool.shareMint, vault: pool.vault, validUntil: (await chainNow(c)) + 86_400n,
});
await councilExecute(c, council, "write_off", [ix]);
```

On mainnet the same instruction is proposed in the Squads app from the IDL, and each member approves
from their own device.

## 4. Collection and the NAV

**4.1 The objective mark.** `mark_in_collection` is permissionless and never paused. A scheduled
`Active` line is eligible once `max_missed` installments are missed — or every remaining installment,
when fewer remain (decision 41) — and the due date of the last of them has passed. There is no grace
period. The mark records `accelerated_at`: from then every installment is due, late interest runs on
all remaining principal, and the ordinary interest of the installments not yet due is forgiven.

1. Every day, read every `Active` scheduled line and compute its arrears with `lineArrears` from
   `@softlaw/elote-pool-client` (it mirrors the program).
2. Send `mark_in_collection` for each eligible line (3.6 pattern, no council needed).
3. `NotEnoughMissed`: a repayment landed in between; try again on the next run. `LineStatus`:
   someone else marked it first; nothing to do.
4. Check the `LineInCollection` event (missed, remaining, interest_forgiven, impaired_principal).

A bullet line has no installments and is never marked. Past its `term_end` it stays `Active`,
unprovisioned, until the council accelerates it or writes it off.

**4.2 The council's acceleration.** For a default event on a drawn `Active` line (bullet or
scheduled, before or after the term):

1. `propose_acceleration`: draws are suspended at once; `AccelerationProposed.due_at` is the end of
   the pool's notice (at least 7 days). Notify the borrower of the acceleration with that date.
2. `withdraw_acceleration` if the borrower cures; draws stay suspended until `resume_draws`.
3. After `due_at`, `accelerate_line`: the line moves to `InCollection` exactly as with the mark,
   without missed installments (`LineAccelerated`).

A mark during the notice supersedes the proposal.

**4.3 What the provision does to the NAV.** At the mark or the acceleration the line's outstanding
USDC cost is added to `Pool.impaired_principal`, and the redemption NAV drops at once by
`impaired_principal × provision_bps / 10,000` (100% unless the council changed it with
`set_provision_bps`). No money moves. Every later recovery (a `repay` on the line in collection)
subtracts what it recovered from the impairment and the NAV rises back; `write_off` removes the rest
as the loss.

**4.4 The dual price** (decisions 40 and 40b).

| Price | Formula | Used by |
|---|---|---|
| Entry NAV | `vault + outstanding_principal` (the impaired book at par) | `deposit` |
| Provisioned NAV | `vault + outstanding_principal − impaired_principal × provision_bps / 10,000` | `redeem`, the line caps, every `nav_after` and `NavUpdated.total_assets` |

An entrant buys any impaired book at par, so a recovery hands the entrant nothing; deposits are never
gated. The two prices differ whenever a line is in collection. The front end shows both: the price a
deposit buys at and the price a redemption pays. The entry NAV appears in no event; compute it from
the vault balance and `Pool.outstanding_principal`.

**4.5 Collection, the procuración and the court.**

1. Endorse the pagarés of the line en procuración to the despacho, one fresh key per case:
   ```sh
   npx tsx scripts/endorse-pagare.ts --cluster devnet --pool <pool> --line-id <n> --draw-index <n> --endoso-hash <64 hex>
   ```
   Hand `.demo-keys/procuradores/<pool>-<line>-<draw>.json` to the despacho. The procurador collects
   through `repay`; the asset stays with the pool.
2. Revoke when the mandate ends:
   ```sh
   npx tsx scripts/endorse-pagare.ts --cluster devnet --pool <pool> --line-id <n> --draw-index <n> --revoke --revocation-hash <64 hex>
   ```
3. The judicial stage runs off chain (decision 38-g). The pool records at most a court's act as a
   `court_annotation` entry over the document's hash, and that annotation gates nothing.

**4.6 The exits.** A line in collection leaves by full repayment, by sale of the portfolio, or by
write-off, at 3 to 12 months of arrears:

- **Sale of the portfolio** is the endoso en propiedad (registry kind 1), reserved for a future
  feature: no instruction makes it today.
- **Write-off** (council): a line in collection once the pool's collection period (at least 30 days,
  `set_collection_period`) has run from the mark and its oldest missed due date or its term has
  passed; an `Active` line after its term. Refused earlier with `WriteOffTooEarly` or
  `TermNotEnded`. Never paused. `LineWrittenOff` carries the loss in MXN and USDC and the NAV after.

## 5. Pause and resume

**What a pause blocks:** `draw`, `deposit`, `redeem`, `request_line`, `approve_line` (`Paused`).

**What never stops:** `repay`, `post_fix`, every pagaré instruction (`deliver_pagare`,
`burn_pagare`, `endorse_pagare`, `revoke_procuracion`, `court_annotation`, `annotate_payment`,
`annul_pagare`), `mark_in_collection`, the acceleration, `write_off`, and every council setting. A
borrower can always pay.

**Who:** the council, by vote, both ways (decision 39). No member and no other key pauses alone.
Both carry `valid_until`.

**When to pause:**

- a council member's key, or a majority, is compromised or suspected;
- a poisoned FIX is in force (a wrong rate published by a compromised feeder or by error) and a
  correction cannot land at once;
- a program bug that moves money wrongly, or `InvariantBroken` from any instruction;
- an asset event: the issuer freezes the vault or pauses the mint, or USDC depegs;
- a legal or regulatory order.

Do not pause for one borrower's default (that is collection) or a KYC failure (that is
`suspend_draws` on the borrower's lines).

**Procedure.**

1. A member opens the Squads proposal naming the reason and the expected duration.
2. The threshold approves; execute `pause` (3.6 pattern: `getPauseInstructionAsync` with `council`,
   `multisig`, `pool`, `validUntil`). `council-propose.ts` has no pause op yet.
3. Check `PoolPaused`; publish it on the status page `<URL>`.
4. Fix the cause: rotate the feeder and re-publish the FIX (section 2), rotate members inside Squads
   or hand over with `propose_council` / `accept_council`, upgrade the program (section 6).
5. Resume after the incident review:
   ```sh
   npx tsx scripts/council-propose.ts --cluster devnet --pool <pool> --op resume
   ```
6. Check `PoolResumed`. A pause longer than `<7 days>` is reported to every holder by `<channel>`.

Approved lines keep their window while paused: a long pause can let a draw window lapse.

## 6. Deploy and upgrade

**Two key traps.**

- The Solana CLI's default keypair is not the upgrade authority. On the deploying machine
  `solana config get` names `~/.config/solana/softlaw-devnet.json`; the authority
  `GU7CfoGZYsfkx23aJoqFjfwyAmLCExqBBoBFrn2EvZKS` is `~/.config/solana/id.json`. Always pass both
  `--upgrade-authority` and `-k` explicitly.
- `target/deploy/elote_pool-keypair.json` is not the program id: `anchor build` writes a fresh random
  one. Never run `anchor deploy` or `anchor keys sync`; they would deploy to, or rewrite
  `declare_id!` with, the wrong address.

**Gates before any deploy.** All on the commit to deploy, clean tree:

1. Build for the tests: `NO_DNA=1 anchor build --ignore-keys --arch v0 --tools-version v1.54 -- --features localnet`
2. Full suite: `cargo test -p elote_pool`
3. Devnet-feature unit tests: `cargo test -p elote_pool --lib --features devnet`
4. Client: `npm run check:client && npm run typecheck && npm run test:client`
5. Local end to end: `scripts/local-e2e.sh` (its own validator, Core and Squads binaries checked by
   sha256, a 2-of-3 multisig, everything in a temporary directory).

The localnet `.so` from step 1 is the e2e's and is never deployed.

**Procedure (devnet, as executed today).**

1. Build with the cluster's feature, which overwrites the localnet `.so`:
   ```sh
   NO_DNA=1 anchor build --ignore-keys --arch v0 --tools-version v1.54 -- --features devnet
   ```
   Mainnet builds with no feature.
2. Check the authority key: `solana-keygen pubkey ~/.config/solana/id.json` prints
   `GU7CfoGZYsfkx23aJoqFjfwyAmLCExqBBoBFrn2EvZKS`.
3. Compare sizes: `ls -l target/deploy/elote_pool.so` against the data length of
   `solana program show BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM -u devnet`. When the binary
   grew past it, extend first:
   ```sh
   solana program extend BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM <additional bytes> -u devnet -k ~/.config/solana/id.json
   ```
4. Deploy:
   ```sh
   solana program deploy target/deploy/elote_pool.so --program-id BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM \
     --upgrade-authority ~/.config/solana/id.json -k ~/.config/solana/id.json -u devnet
   ```
5. Verify byte for byte; the two hashes must be equal:
   ```sh
   solana-verify get-executable-hash target/deploy/elote_pool.so
   solana-verify get-program-hash -u devnet BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM
   ```
6. Record the commit, date and hash in the README's deployments table and in section 7.
7. Smoke test on a pool: a FIX, a request, an approval through Squads, a draw, a repayment that
   delivers its pagaré.

**Before mainnet.** The upgrade authority moves to a Squads multisig; the IDL is published on chain
(`anchor idl init`); the build has no cluster feature and is verified the same way.

## 7. Devnet directory

| Item | Address | Notes |
|---|---|---|
| Program | `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM` | `main` @ `ce0b1b2`, 2026-11-08; verified hash `d2b24a8e51648300d747fba7c9308713dd2fe5768bc6a4a8ca1c679bc37ab61a` against the `--features devnet --arch v0 --tools-version v1.54` build |
| Upgrade authority | `GU7CfoGZYsfkx23aJoqFjfwyAmLCExqBBoBFrn2EvZKS` | `~/.config/solana/id.json`; a single hot wallet, test only. Squads and `anchor idl init` before mainnet |
| Council multisig | `C3sa3p6WRkEULZbVmYGDB3i6YaoDAWJXM9euUPJg8Vap` | Squads v4, 2 of 3, created with `scripts/devnet-multisig.ts` |
| Council (vault 0) | `6s6aBtkG6Wm9N6Gx1igKnc3Q3fcJt2r1WtyyLSQrnS6x` | |
| Smoke pool | `7ezjgeqcSkdXE5pt2gGBczbwEMvH9C8m1nPhBXt7kCKo` | Created with `scripts/demo-pool.ts --cluster devnet --multisig C3sa3p6WRkEULZbVmYGDB3i6YaoDAWJXM9euUPJg8Vap --asset-mint 4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU --scale 1000` |
| Pool asset | `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU` | Circle's devnet USDC, from faucet.circle.com (10 per hour) |
| Squads v4 | `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf` | |
| Metaplex Core | `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d` | |

A new devnet multisig: `npx tsx scripts/devnet-multisig.ts --cluster devnet --members <a.json,b.json,c.json> --threshold 2`.
The pool address is `["pool", creator, asset_mint]`, so one creator key makes one pool per asset mint.

## 8. Compute and monitoring

**Compute.** Measured by `programs/elote_pool/tests/compute.rs` (the program alone) and
`tests/squads.rs` (the whole Squads execution). The rule is 100,000 CU per transaction, Squads
included; `draw` is pinned at or below 90,000 and `init_pool` under 70,000.

| Case | CU |
|---|---|
| accelerate_line at 120 installments, all due | 49255 |
| annotate_payment at 120 installments | 20368 |
| approve_line (with the multisig check) | 15271 |
| approve_line through Squads | 41411 |
| deliver_pagare at 120 installments | 67653 |
| deliver_pagare after a procuración | 67559 |
| burn_pagare | 30568 |
| court_annotation at 120 installments | 17690 |
| draw, first tranche at 120 installments | 82336 |
| draw, second tranche at 120 installments | 82510 |
| draw, tranche 10 at 120 installments | 82523 |
| endorse_pagare (procuracion, plugin added) at 120 installments | 38407 |
| endorse_pagare (procuracion, plugin approved) at 120 installments | 44992 |
| endorse_pagare through Squads | 70140 |
| init_pool, classic asset and permissioned shares | 38173 |
| init_pool, classic asset and plain shares | 38639 |
| mark_in_collection at 120 installments | 36729 |
| mark_in_collection at 120 installments, 3 missed | 13712 |
| mark_in_collection at 18 installments | 16719 |
| pause by the council | 9492 |
| post_fix, by the council | 12966 |
| post_fix, by the feeder | 11794 |
| post_fix, by the feeder against the reference | 12081 |
| propose_acceleration | 14587 |
| propose_council through Squads | 34972 |
| repay after the mark at 120 installments | 58047 |
| repay at 120 installments | 68421 |
| repay at 18 installments | 39537 |
| repay covering three sidecars at 120 installments, all missed | 90003 |
| request_line at 120 installments | 22390 |
| resume_draws | 13892 |
| revoke_procuracion at 120 installments | 40092 |
| suspend_draws | 14577 |
| write_off after the mark at 120 installments | 55350 |
| write_off at 120 installments | 55348 |
| write_off at 18 installments | 26584 |

**Events an indexer follows.** Every instruction emits through `emit_cpi!` (`events.rs`):

| Area | Events |
|---|---|
| Pool and NAV | `PoolInitialized`, `Deposited`, `Redeemed`, `NavUpdated`, `PoolPaused`, `PoolResumed` |
| Lines | `LineRequested`, `LineApproved`, `LineRejected`, `RequestWithdrawn`, `LineCancelled`, `LineDrawn`, `LineDrawsSuspended`, `LineDrawsResumed`, `LineRepaid`, `LateInterestCollected`, `FxRealized` |
| Collection | `LineInCollection`, `AccelerationProposed`, `AccelerationWithdrawn`, `LineAccelerated`, `LineWrittenOff` |
| Pagarés | `PagareIssued`, `PagareEntry`, `PagarePayment`, `PagareReceiptAnnotated`, `PagareDelivered`, `PagareBurned`, `PagareAnnulled` |
| FIX | `FixPosted`, `FxFeederSet`, `FixBoundsSet` |
| Council and settings | `CouncilProposed`, `CouncilAccepted`, `MaxLineBpsSet`, `ProvisionSet`, `OutstandingCapSet`, `CollectionPeriodSet`, `AccelerationNoticeSet` |

**Alerts.**

| Condition | Why | Action |
|---|---|---|
| `impaired_principal > outstanding_principal` on the `Pool` account, or any `InvariantBroken` | `Pool::check` holds it impossible | Pause (section 5), page `<on-call>` |
| Share supply nonzero and below 1,000 | The dead shares never move | Pause, page |
| `NavUpdated` discontinuity: `total_assets` differs from the previous one by more than the flows in between explain | A movement outside the four money flows | Investigate; pause if money moved wrongly |
| `FixPosted` missing for a Mexico City business day, or the FIX older than 4 days | Draws and approvals stop at 5 days | Feeder restart, else the council publishes |
| `FixPosted.fix_rate` different from the SIE value for `fix_date` | Poisoned FIX | Council correction the same day; pause if it cannot land |
| `FixPosted.by` neither the feeder nor the council, or a `FxFeederSet` / `CouncilProposed` / settings event without a matching approved proposal | Key compromise | Pause, rotate |
| `PoolPaused` | | Status page |
| A line eligible for `mark_in_collection` for more than 24 h | The collection period counts from the mark | Send the mark |
| A covered pagaré without `PagareDelivered` for more than 1 h | The delivery was left out of the repayment | `deliver-pagare.ts` |

**NavUpdated continuity.** `NavUpdated` is emitted last by `deposit`, `redeem`, `repay` and
`write_off`, and its `total_assets` is the provisioned NAV. Between two `NavUpdated` events the NAV
also moves without one: `LineInCollection` and `LineAccelerated` (their `impaired_principal` times
`provision_bps` comes off), `ProvisionSet` (the provision is re-priced), and a token donated straight
to the vault. `draw` moves USDC from the vault to outstanding principal and leaves the NAV unchanged.
An indexer replays these to explain every step; a step it cannot explain is the alert.

## Incident contacts

| Role | Name | Reach |
|---|---|---|
| On-call engineer | `<name>` | `<phone / pager>` |
| Council members (2 of 3) | `<names>` | `<channel>` |
| FIX feeder operator | `<name>` | `<channel>` |
| Compliance officer | `<name>` | `<email>` |
| Legal counsel / despacho | `<firm>` | `<email>` |
| Etherfuse (asset issuer and rails) | `<contact>` | `<email>` |
| Security disclosures | GitHub security advisory (SECURITY.md) | https://github.com/soft-law/elote-contracts/security/advisories/new |
