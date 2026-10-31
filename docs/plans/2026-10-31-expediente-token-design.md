# The expediente as the asset the pool locks: design

Date: 2026-10-31. Status: decided (owner review), not started. Source: Mario's framing of
2026-10-31: other lending protocols lock a collateral token in the pool; in Elote that token
represents the signed legal documents (credit agreement, one pagaré per draw, the credit file or
expediente), and privacy must hold. Inputs: the two surveys in
`docs/research/2026-10-31-document-tokens-evm.md` (Centrifuge document NFTs and Tinlake, Huma v2
receivables, Untangled, Goldfinch, Maple, Provenance) and
`docs/research/2026-10-31-document-tokens-solana.md` (Metaplex Core, Token-2022, Huma on Solana,
Credix, Light), plus the loans-gov-mexico pagaré research (`brief-tok/M-pagares-recovery.md`,
`brief-tok/G-tokenization.md` Findings 10 and 11). Every code claim below cites the report that
cites the file.

## What the token is, and is not

- **It is** the on-chain record of the loan's documents and of their custody: a salted
  commitment to the expediente, one commitment per signed pagaré, folio, amounts, dates, status,
  and who holds the paper. The pool owns it while the line is alive, so a draw is only possible
  against a locked document, and the pool keeps it when the line is written off so cobranza has
  it.
- **It is not** the pagaré. Under the 2024 LGTOC reform a pagaré may be electronic through a
  "sistema de información", but the tenedor must be a named persona, each endoso is a signed
  record naming the endorsee, and a bare token transfer has none of those elements
  (`M-pagares-recovery.md` §1.4, §1.7, §2.2). The token tracks the economic interest and the
  location of the título; a SOFOM, fiduciario or SPV is the legal holder. Huma says this about its
  own receivable NFT in code (`document-tokens-evm.md`, pattern 15).
- **It is not a valor.** One token per line, bilateral, never offered in series to the public
  (`G-tokenization.md` Finding 11, "Trap").

## What the surveys settled

| Question | Answer | Where |
|---|---|---|
| Has this been built? | Yes: Centrifuge's document NFTs (2019-2020) anchored a salted merkle root of the document, minted only with proofs of chosen fields and a signature from a registered identity, bound the token to the document both ways, and gave the token holder a read role; Tinlake locked that NFT in the pool and gated borrow and repay on custody. | evm report §1, patterns 1-5, 7 |
| What went wrong elsewhere? | Huma's receivable NFT: creator can rewrite the URI and self-declare payments, the pledged token is never released, and a plaintext reference travels in calldata. Tinlake hands the NFT back to the borrower after a 100% write-off, which gives away the paper cobranza needs. Credix publishes unsalted hashes of tax IDs, which enumeration reverses. | evm patterns 12-14; solana report §4 |
| Which Solana primitive? | The pool's own program account is the source of truth: about 0.002 SOL per record, all refundable, no third-party upgrade authority, no CPI on draw or repay. A Metaplex Core asset per line is the visible wrapper when one is wanted (owner and update authority = pool PDA, `PermanentFreezeDelegate` and `PermanentTransferDelegate` set at creation, `ImmutableMetadata`, attributes locked with `Authority::None`, `AppData` for per-draw hashes). Token-2022 is worse on every row (two accounts, all-or-nothing metadata lock, `NonTransferable` blocks a sale). Light compression adds an indexer dependency and no privacy. | solana report §6, §7 |
| Who else does it that way? | Huma on Solana: a Core asset plus a program account per receivable. Credix: program account only. | solana report §3, §4 |

## Design

Two layers, in this order.

### Layer 1, in the line v3 batch: document records inside elote_pool

1. **Expediente commitment at opening.** `open_line` gains `expediente_root: [u8; 32]`, the
   salted merkle root of the credit file (one leaf per document or field,
   `hash(field_id ‖ value ‖ salt)`, salts kept off chain by the parties), stored next to
   `agreement_hash`, which stays the keccak of the signed master agreement. Zero is rejected as
   today (`lines.rs:78`). Nothing else about the person goes on chain.
2. **One `Pagare` record per draw.** `draw(amount, document_hash)` creates a PDA
   `["pagare", line, draw_index]` holding only immutable facts plus custody:
   `{document_hash, amount, issued_at, due_at, draw_index, tenedor, endoso_hash}`.
   `document_hash` is the hash of the signed pagaré bytes (plus the schedule annex when
   separate). There is **no folio on chain**: `draw_index` identifies the pagaré and the
   agreements engine maps index to folio off chain, so no issuing-system number ever appears in
   instruction data or in an event (evm pattern 13). `tenedor` is copied from the pool's
   `tenedor` field at creation (item 3). The record is owned by the program and sits under the
   line: that is the lock. Rent about 0.002 SOL per record, refunded on close; the app's relayer
   pays it, because SME wallets rarely hold SOL.
3. **Two new pool fields and the attestor co-signature.** Pool v2 gains `attestor: Pubkey` (the
   agreements engine's signing key) and `tenedor: Pubkey` (the key of the one legal holder per
   series: a SOFOM, fiduciario or SPV). `Pool.reserved` has 28 free bytes, so the Pool layout
   grows by 64 bytes in the same breaking batch as the rest of pool v2, with admin instructions
   `set_attestor` and `set_tenedor`. A draw requires the attestor's signature over the
   transaction, which states that the pagaré with `document_hash` was signed by the borrower and
   conserved with a NOM-151 constancia before any money moved. This is Untangled's validator
   signature and Centrifuge's identity signature at mint (evm patterns 8 and 2). Without it the
   borrower could draw against a hash of nothing. **New trusted roles, to be written into
   `SECURITY.md`'s trust table:** the attestor can block every disbursement by withholding its
   signature and cannot move funds, with `set_attestor` as the only recovery; the tenedor key can
   sign endosos and cannot touch the pool. `ARCHITECTURE.md` §8's "no operator role" sentence
   changes accordingly.
4. **Lifecycle, with derived status and no account fan-out.** The record never stores a status.
   `Paid`, `InCollection` and `WrittenOff` are computed by clients from the line's status and the
   coverage rule: pagarés are covered oldest first by `draw_index`, and pagaré `k` is paid once the
   line's cumulative principal and interest repaid cover every pagaré up to `k`. So `repay`,
   `mark_in_collection` and `write_off` take no pagaré accounts and do no loops.
   - `close_pagare(k)` verifies that coverage at close time, closes the record with rent to the
     relayer and emits `PagareClosed` (LGTOC art. 129: the debtor is entitled to the cancelled
     título against payment; the cancellation happens in the issuing system off chain, the close
     only says so). It fails while the line is `InCollection` or `WrittenOff`, so written-off
     records are never released: cobranza needs them, and Tinlake's return-to-borrower is the
     pattern to avoid.
   - `endorse_pagare(k, endoso_hash, new_tenedor)` is signed by the current tenedor's key and by
     the admin; it records the hash of the signed endoso (endorsee name, type, place and date live
     in that document, not on chain) and changes `tenedor`. The admin co-signature makes the
     Squads vault a gate on every sale of paper, which goes on counsel item 10's list of admin
     powers. Before maturity this is an endoso en
     propiedad; after it, a cesión, and the debtor notice and RUG registration happen off chain
     (`M-pagares-recovery.md` §2.2). Investor-to-investor trades of series shares never touch a
     pagaré record.
   - Amendments: a `convenio modificatorio` is a new version, `amend_line(new_terms_hash)`
     proposed by the admin and accepted by the borrower's signature before a deadline (Maple's
     two-party hash commitment, evm pattern 10). Out of scope for the first batch; listed so the
     layout leaves room.
5. **Privacy by construction.** On chain: salted roots, document hashes, folios, amounts, dates,
   status, public keys. Off chain, in the agreements engine: the signed documents, the salts, the
   wallet-to-person map, the read-access list, and the NOM-151 constancias. Read access follows
   custody: the engine serves a document only to the current tenedor's key, the borrower, the
   admin, or a judge-facing verifier with a fe de hechos (evm pattern 5; `M-pagares-recovery.md`
   §1.6). No folio, RFC, CURP or document name is ever passed in instruction data or placed in a
   URI, and no identifier is hashed without a salt (evm pattern 13; Credix's mistake).
6. **Events.** `PagareIssued { pool, line_id, draw_index, document_hash, amount, due_at,
   tenedor }`, `PagareClosed { pool, line_id, draw_index }`, `PagareEndorsed { pool, line_id,
   draw_index, endoso_hash, from, to }`, and `LineDrawState` as in the tranched plan. Append-only
   as always, and no folio in any of them.

### Layer 2, after the external review: a visible asset per line

When investors or a buyer need to see the position in a wallet or an indexer, or when the first
sale of a line happens, the pool mints one Metaplex Core asset per line at `open_line`:
owner and update authority = pool PDA; `PermanentFreezeDelegate` and `PermanentTransferDelegate`
with the pool PDA as authority, set at creation because they cannot be added later;
`ImmutableMetadata`; an Attributes plugin (agreement hash, expediente root, folio prefix, principal,
term) with authority `None`; an `AppData` section with the pool PDA as data authority, appended
with each pagaré hash at `draw`. Burn on `Repaid`; keep frozen on `WrittenOff`; `TransferV1` by
the permanent delegate on a sale, in the same transaction as `endorse_pagare`. One asset per line,
not per pagaré, to keep a sale atomic and avoid Core's non-refundable fee on every draw. The
`CreditLine` and the `Pagare` records stay the source of truth; the asset is the wrapper, as in
Huma on Solana. Cost about 0.0056 SOL per line, 0.0032 recoverable. Risks accepted then: Core's
upgrade authority and one extra CPI on `open_line`, `draw` and close.

### What does not change

The pool's NAV still counts outstanding principal, never a document valuation; there is no
oracle. Investor gating (Token ACL) and the SAS attestations are untouched. `repay` stays
permissionless. Series shares never reference a pagaré.

## Changes to the tranched plan

- D9 becomes Layer 1 item 2: the per-draw hash is stored in a `Pagare` record, not only emitted.
- New D14: attestor co-signature on `draw`; `attestor` and `tenedor` on the Pool (64 bytes, pool
  v2 batch; the series-shelf facts table and item 2d say so); `set_attestor`, `set_tenedor`.
- New D15: `expediente_root` on `CreditLine` (32 bytes more; the v3 growth becomes 51 bytes).
- New instructions: `endorse_pagare`, `close_pagare`, `set_attestor`, `set_tenedor`. New errors:
  `AttestorRequired`, `PagareNotCovered`, `NotTenedor`.
- Tests: a draw without the attestor fails; records are created per draw with no folio; the
  coverage rule pays oldest first and `close_pagare` of a later pagaré fails while an earlier one
  is uncovered; a written-off or in-collection line keeps its records and `close_pagare` fails; an
  endoso changes the tenedor and only the tenedor's key plus the admin may sign it; no instruction
  accepts a hash of zero; `repay`, `mark_in_collection` and `write_off` take no pagaré accounts.
- `SECURITY.md` trust table and `ARCHITECTURE.md` §8 gain the attestor and tenedor roles.
- Estimate: 1.5 days more on the program and tests, half a day on the agreements engine side to
  produce the salted root and the attestor signature.

## Decisions taken

| # | Decision |
|---|---|
| 1 | The lockable asset is a program record first, a Core asset second. Token-2022 and Light are out. |
| 2 | The pool, not the borrower, creates the record, and only the pool changes its status. |
| 3 | A draw needs the attestor's signature; no money moves against an unattested hash. |
| 4 | Written-off records are never released to the borrower; paid ones are closable with rent back to the borrower. |
| 5 | The legal holder is a persona whose key is `tenedor`; token movements never stand in for an endoso. |
| 6 | Only salted commitments, hashes, folios, amounts, dates and keys go on chain; read access follows custody off chain. |

## For counsel (joins the consolidated list as item 16)

Whether the `Pagare` record plus the agreements engine, named in the contract as the "sistema de
información", satisfies LGTOC arts. 5o, 5o Bis, 17, 26, 27 and 29 for issuance, delivery and
endoso; how the cancellation of a paid electronic pagaré is evidenced (art. 129); and whether the
attestor's signature and the NOM-151 constancia together meet the evidentiary standard of tesis
2031391 without SAT e.firma. These extend items 6 and 15.
