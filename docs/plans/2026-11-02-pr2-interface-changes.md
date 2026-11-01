# PR 2 (`v2/pool-and-lines`): interface changes for the backend and the app

Appended per commit, oldest first. Everything below is on the branch; nothing is deployed. Error
codes are `6000 + index` in `errors.rs`; accounts and events are in `idl/elote_pool.json` and the
Codama client (`clients/js/src/generated`). Sizes include the 8-byte discriminator.

## Through step 8 (`f366ff5`, 2026-11-02)

### Instructions

| Instruction | Arguments | Signers | Notes |
|---|---|---|---|
| `init_pool` | `grace_period_seconds: u32`, `max_line_bps: u16` (1..=10 000), `attestor: Pubkey`, `collection_period_seconds: u32` (>= 30 days) | admin | New accounts since v1: `share_mint` (admin-created, pool PDA as mint authority), `collection` (a bare Core collection with the pool PDA as update authority: no plugins, nothing minted), `share_token_program`. `attestor != admin`. Emits `PoolInitialized`, `PoolConfigured`, `PoolLineCapSet`, `PoolFlags`, `AttestorSet`, `PoolCollectionSet`, `CollectionPeriodSet` |
| `set_attestor` | none | admin, new attestor | The new key co-signs; never the admin. Emits `AttestorSet` |
| `set_collection_period` | `collection_period_seconds: u32` (>= 30 days) | admin | Emits `CollectionPeriodSet` |
| `open_line` | `line_id, principal, rate_bps, term_end, agreement_hash, sponsor: Option<Pubkey>, sponsor_role: u8, schedule: Option<ScheduleParams>, expediente_root: [u8; 32]` | admin, borrower, attestor, sponsor if named | `borrower != admin`, `borrower != attestor`. Emits `LineOpened`, `LineScheduled` (if scheduled), `LineTermsSet`, `LineSponsorRole` |
| `draw` | `amount, document_hash, asset_bump: u8, pagare_bump: u8, cert_fingerprint: [u8; 32], signature_method: u8` (0 wallet, 1 PSC + wallet, 2 e.firma + wallet) | borrower, attestor, payer (rent) | New accounts: `payer`, `asset` (PDA `["pagare_asset", line, draws u16 LE]`), `pagare` (PDA `["pagare", line, draws]`), `collection`, `mpl_core_program`, `system_program`; the client passes both bumps (`drawPagareAccounts` in `clients/js/src/pagare.ts`). Emits `LineDrawn`, `LineDrawState`, `PagareIssued` |
| `repay` | `amount: u64` | anyone | Refused on a zero debt while the line can still draw (`WrongAmount`). **Closing repayment:** once the window has closed, a payment on a zero debt closes the line (`Repaid`) and moves nothing: `LineRepaid` with zero parts and `remaining = 0`; not an error. Emits `LateInterestCollected` (if any), `LineRepaid`, `NavUpdated` |
| `mark_in_collection` | none | anyone | Accelerates the balance: sets `accelerated_at`, forgives the interest of installments not yet due. Emits `LineInCollection` (with `interest_forgiven`) |
| `write_off` | none | admin | On a line in collection also waits `collection_period_seconds` from `accelerated_at`. Emits `LineWrittenOff`, `NavUpdated` |
| `suspend_draws` | none | admin | Emits `LineDrawsSuspended` |
| `cancel_line` | none | admin | Emits `LineCancelled` |
| `cancel_pagare` | `draw_index: u16`, `receipt_hash: [u8; 32]` | holder (the admin while the asset is in the pool), attestor, payer (receives the asset's rent) | Accounts: `pool`, `line`, `pagare`, `asset` (= `Pagare.asset`), `collection`, `mpl_core_program`, `system_program`. Emits `PagareCancelled`, then burns the asset |

### Events (all `emit_cpi!`, read from inner instructions)

New since v1: `PoolLineCapSet { pool, max_line_bps }`, `PoolFlags { pool, asset_controls, permissioned }`,
`AttestorSet { pool, attestor }`, `PoolCollectionSet { pool, collection }`,
`CollectionPeriodSet { pool, collection_period_seconds }`, `LineTermsSet { pool, line_id,
first_due_at, availability_end, expediente_root }`, `LineSponsorRole { pool, line_id, sponsor, role }`,
`LineDrawState { pool, line_id, amount, drawn, available, interest_due, availability_end,
document_hash }`, `PagareIssued { pool, line_id, draw_index, asset, pagare_hash, amount, due_at,
holder }`, `PagareCancelled { pool, line_id, draw_index, asset, receipt_hash, holder }`,
`LineDrawsSuspended { pool, line_id, available_forfeited }`, `LineCancelled { pool, line_id }`,
`LateInterestCollected { pool, line_id, amount }`, `NavUpdated { pool, total_assets, share_supply }`.
Changed by appending: `LineInCollection` gains `interest_forgiven: u64`; `LineDrawn.amount` is the
tranche, emitted once per draw.

### Errors added (6019 onward)

6019 `BorrowerIsAdmin`, 6020 `LineTooLarge`, 6021 `InvariantBroken`, 6022 `AssetMintNotAllowed`,
6023 `ShareMintNotAllowed`, 6024 `AttestorRequired`, 6025 `AvailabilityEnded`, 6026 `TooManyDraws`,
6027 `CollectionNotAllowed`, 6028 `DocumentHashRequired`, 6029 `SignatureMethodInvalid`,
6030 `PagareNotCovered`, 6031 `PagareAlreadyPaid`, 6032 `NotHolder`, 6033 `AttestorIsAdmin`,
6034 `BorrowerIsAttestor`, 6035 `AgreementHashRequired`, 6036 `ExpedienteRootRequired`,
6037 `SponsorRoleInvalid`, 6038 `CollectionPeriodTooShort`. `TermEnded` (6012) now means a
`term_end` not after the opening. Anchor's `ConstraintSeeds` (2006) is what a wrong pagaré address
or bump returns on `draw`; `AccountNotInitialized` (3012) is what `cancel_pagare` returns on a
line that never drew.

### Account layouts

| Account | Size | Change |
|---|---|---|
| `Pool` | 299 bytes | `attestor` and `collection` appended after `bump`; `reserved`: 0..4 grace, 4..6 `max_line_bps`, 6 `permissioned`, 7 `asset_controls`, 12..16 `collection_period_seconds` (8..12 kept for `cancel_delay_seconds`) |
| `CreditLine` | 313 bytes (305 before step 7) | `drawn`, `draws`, `availability_end`, `first_due_at`, `sponsor_role`, `expediente_root` (line v3) and `accelerated_at` (step 7) appended after `bump`; the `Schedule` packed in `reserved` 0..30 |
| `Pagare` | 245 bytes | New: `line, draw_index, asset, pagare_hash, amount, issued_at, due_at, drawn_through, cert_fingerprint, signature_method, status, paid_at, receipt_hash, bump, reserved`. Grows at step 6d (see ARCHITECTURE §2.3) |

### Client helpers (`clients/js/src`)

`schedule.ts`: `poolGracePeriodSeconds`, `poolMaxLineBps`, `maxLinePrincipal`, `poolFlags`,
`decodeSchedule`, `lineArrears` (now takes `acceleratedAt`); the mirror of `math::arrears`, checked
against `test/arrears-vectors.json` (421 cases). `pagare.ts`: `MPL_CORE_PROGRAM_ADDRESS`,
`findPagareAssetPda`, `drawPagareAccounts` (asset, sidecar and both bumps for the next draw). The
generated client derives `findPagarePda` and the `cancelPagare` builder derives the sidecar itself.

## 6d part 1 (a): the registry and the final creation set

- **`Pagare` grows to 353 bytes** (from 245): `log_root: [u8; 32]`, `entry_count: u16`, `last_kind: u8`,
  `flags: u8`, `cancel_receipt_hash: [u8; 32]`, `requested_at: i64`, `requested_receipt_hash: [u8; 32]`
  appended after `reserved`. Sidecars created before this commit (devnet only) do not deserialize.
  `status` gains the value 2, `PAGARE_PAID_PENDING_CANCEL`.
- **The registry.** Every pagaré has an append-only hash chain in its sidecar: `log_root =
  sha256(log_root || entry_hash)` with `entry_hash = sha256(asset || kind || from || to || hash ||
  stated_at LE || entry_count LE)`. The entry's full content is the new event `PagareEntry { pool,
  line_id, draw_index, asset, kind, from, to, entry_hash, entry_count, recorded_at, stated_at }`;
  an indexer replays the events to rebuild and verify the chain. Kinds: 0 Emision, 1 Propiedad,
  2 Procuracion, 3 Revocacion, 4 Terminacion, 5 Abono, 6 AnotacionJudicial, 7 ConstanciaJudicial,
  8 EndosoTardio, 9 Pagado, 10 SolicitudCancelacion, 11 Recibo. `draw` writes entry 1 (Emision,
  from = to = the pool PDA, hash = the pagaré's hash) and emits `PagareEntry` after `PagareIssued`.
- **The asset's plugin set changed.** The pagaré asset now carries `PermanentFreezeDelegate`
  (authority `UpdateAuthority`), `ImmutableMetadata`, `Attributes` and `AddBlocker`; no
  `PermanentTransferDelegate`, no `PermanentBurnDelegate`, no `AppData` adapter. Anything that read
  the AppData adapter reads the sidecar's registry and the `PagareEntry` events instead.
- `cancel_pagare` is unchanged in its interface; it now thaws before burning.

## 6d part 1 (b): `endorse_pagare`, the endoso en procuración

- **New instruction** `endorse_pagare(draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32])`.
  Signers: holder (the admin while the asset is in the pool), attestor, payer. Accounts: `pool`,
  `line`, `pagare`, `asset` (= `Pagare.asset`), `collection`, `mpl_core_program`,
  `system_program`. In this version `kind` must be 2 (`Procuracion`); `to` is the procurador's key,
  never the zero key or the pool; `endoso_hash` is the signed endoso's hash, non-zero. Appends entry
  `Procuracion` (from = the pool, to = the procurador) and emits `PagareEntry`; the asset gains an
  owner-managed `BurnDelegate` whose authority is the PDA `["procurador", pagare, to]` (helper to
  come in `pagare.ts`); nothing else moves.
- **Sidecar `flags`:** bit 0 = the `BurnDelegate` plugin exists on the asset; bit 1 = a procuración
  is in force. Clients read bit 1 to show "en procuración" and `Pagare.procurador` (the former
  `reserved` bytes at offset 213, same size) for who may collect; the zero key when none.
- **Errors added:** 6039 `PagareAccountsMismatch` (reserved for the repay entries), 6040
  `EndosoKindInvalid`, 6041 `EndorseeInvalid`, 6042 `ProcuracionState`.
- Compute: about 34 600 CU at 120 installments.

## 6d part 1 (c): `revoke_procuracion`, and the procurador on the sidecar

- **New instruction** `revoke_procuracion(draw_index: u16, entry_hash: [u8; 32])`. Signers: holder (the
  admin while the asset is in the pool), payer. Accounts as `endorse_pagare` without the attestor.
  `entry_hash` is the signed revocation's hash. Appends entry `Revocacion` (from = the pool, to =
  the recorded procurador), emits `PagareEntry`, clears `flags` bit 1 and `Pagare.procurador`; the
  asset's `BurnDelegate` returns to authority `Owner` and stays. Errors: `DocumentHashRequired`,
  `ProcuracionState` (6042) when nothing is in force.
- **`Pagare.procurador: Pubkey`** at offset 213 (the former `reserved` bytes, same size): the
  procurador of the procuración in force, the zero key when none. Set by `endorse_pagare`, cleared
  here. Clients show it as who may collect today.
