# The pagaré token: the título itself, issued and locked on chain

Date: 2026-10-31. Status: decided (owner), not started. Source: Mario's framing of 2026-10-31,
and his position stated the same day: **the token is the pagaré**. That is Elote's legal stance
and its innovation, not a representation of a paper held elsewhere. Inputs: the two surveys in
`docs/research/2026-10-31-document-tokens-evm.md` (Centrifuge document NFTs and Tinlake, Huma v2,
Untangled, Goldfinch, Maple, Provenance) and `docs/research/2026-10-31-document-tokens-solana.md`
(Metaplex Core, Token-2022, Huma on Solana, Credix, Light), plus the loans-gov-mexico pagaré
research (`brief-tok/M-pagares-recovery.md`, `brief-tok/G-tokenization.md` Findings 10 and 11),
whose textual reading was more conservative than this position and is cited where it marks a
risk. Every code claim cites the report that cites the file.

## The position

The 2024 reform of the LGTOC (DOF 26-03-2024) says títulos de crédito exist whether issued in
writing or electronically, may be issued "a través de un sistema de información" under Código de
Comercio art. 89, and that presentment, endoso, delivery, transfer, verification of the last
holder and aval all happen through that system (arts. 5o, 5o Bis, 17, 26, 27, 29, 39, 40;
`G-tokenization.md` Finding 11). Elote's position:

1. **The sistema de información is Solana plus the Elote program plus the Softlaw agreements
   engine, together**, designated as such in the text of every pagaré and in the credit
   agreement. The chain holds the título's registration, integrity commitment and circulation;
   the engine holds the signed data message (the pagaré text, with the names the law requires)
   and serves it to whoever the system entitles.
2. **The token is the título.** One Metaplex Core asset per disposición is the electronic pagaré:
   its creation is the issuance, its on-chain holder is the tenedor, its transfer through the
   program's `endorse` instruction is the endoso, and its burn against full payment is the
   cancellation (art. 129). Circulation is traceable by construction, which is what art. 5o Bis
   asks for integrity, and the título "can be consulted" in the system through a public verifier.
3. **Holders are personas.** The pagaré is never al portador: every wallet that may hold a pagaré
   token is bound to an identified persona through the KYC record and the SAS attestation
   (identity evaluation), and the token can only move to such a wallet. While the loan is alive
   the holder is the series vehicle (the SOFOM, fideicomiso or SPV behind each series, whose
   administrator is the Squads vault) and the pool PDA is that vehicle's custody account.
4. **Privacy holds because the chain carries the commitment, not the text.** The pagaré's text
   (debtor, beneficiary, amount in words, place, date, signatures) is a data message in the
   engine, encrypted at rest; the asset carries its hash, the amount, the dates and the holder's
   key. Nothing personal is in instruction data, attributes or URIs.

### What counsel must defend, stated plainly

The loans-gov-mexico research reads the same texts more narrowly and names the attack surface
(`M-pagares-recovery.md` §1.6, §1.7, §2.2): tesis 2031391 demands a signature from an accredited
PSC rather than a bare wallet key; art. 5o Bis grants the writing requirement "cuando así
expresamente lo permita la Ley" and no pagaré article was amended; arts. 17 and 27 want a system
"determinado conforme a las disposiciones legales aplicables" while no CNBV rules exist; a PDA is
not a persona. The design below answers each one with a mechanism, and counsel item 16 asks for
the opinion that closes them. Until that opinion, the mechanisms are built so that the same
token also works as the custody record of a paper or PSC-signed pagaré, which costs nothing
extra.

## What the surveys settled

| Question | Answer | Where |
|---|---|---|
| Has a document token been locked in a pool before? | Centrifuge's document NFTs (2019-2020) anchored a salted merkle root, minted only with proofs of chosen fields and a registered identity's signature, bound token and document both ways, gave the holder a read role; Tinlake locked the NFT in the pool and gated borrow and repay on custody. Nobody claimed the token was the título. | evm report §1, patterns 1-5, 7 |
| What went wrong elsewhere? | Huma: creator can rewrite the URI and self-declare payments, the pledged token is never released, a plaintext reference travels in calldata. Tinlake returns the NFT to the borrower after a 100% write-off. Credix publishes unsalted hashes of tax IDs. | evm patterns 12-14; solana report §4 |
| Which Solana primitive? | Metaplex Core: one account per asset, pool PDA as owner and update authority, `PermanentFreezeDelegate` and `PermanentTransferDelegate` set at creation (they cannot be added later), `ImmutableMetadata`, attributes locked with `Authority::None`, `AppData` with the pool as data authority for the endoso log. Token-2022 needs two accounts, locks metadata all at once and its `NonTransferable` setting would block the endoso; Light adds an indexer dependency and no privacy. Huma on Solana uses Core plus a program sidecar. | solana report §1, §6, §7 |

## Design

### Issuance: `draw` mints the pagaré

- `draw(amount, pagare_hash)` requires three signatures: the borrower (the suscriptor), the
  attestor (the agreements engine's key, stating the data message with `pagare_hash` was signed
  by the borrower with the method the template fixes, PSC or e.firma where counsel requires it,
  and conserved with a NOM-151 constancia), and the fee payer (the app's relayer, since SME
  wallets rarely hold SOL). No money moves without the attestor.
- The program creates, by CPI, one Core asset in the series' collection: owner = pool PDA (the
  vehicle's custody), update authority = pool PDA, `PermanentFreezeDelegate` and
  `PermanentTransferDelegate` with the pool PDA as authority, `ImmutableMetadata`, an Attributes
  plugin `{pagare_hash, amount, currency, issued_at, due_at, line, draw_index}` with authority
  `None`, and an `AppData` section (data authority = pool PDA) that will hold the endoso log. The
  asset's name is the series symbol plus the draw index; its URI points to public, non-personal
  JSON or is empty. Cost about 0.0056 SOL per pagaré, 0.0032 recoverable at burn (solana report
  §1(d)).
- A `Pagare` sidecar PDA `["pagare", line, draw_index]` stores `{asset, pagare_hash, amount,
  issued_at, due_at, draw_index}` for the program's own logic; it holds no status and no folio
  (the engine maps index to folio off chain). `expediente_root: [u8; 32]`, the salted merkle root
  of the credit file, is stored on `CreditLine` at `open_line` next to `agreement_hash`.
- Emits `PagareIssued { pool, line_id, draw_index, asset, pagare_hash, amount, due_at, holder }`.

### Custody: the pool holds the título

- While the line is alive the asset sits in the pool PDA. That is both the collateral lock the
  owner asked for and the vehicle's possession of the título. `repay`, `mark_in_collection` and
  `write_off` take no pagaré accounts and loop over nothing: a pagaré's state is derived by
  clients from the line's status and the coverage rule (pagarés are covered oldest first by
  `draw_index`; pagaré `k` is paid once cumulative principal and interest repaid cover every
  pagaré up to `k`).
- Frozen by the permanent freeze delegate from creation, so nothing but the program's own
  instructions can move it, and the permanent transfer delegate lets the program move it while
  frozen (solana report §1(b)).

### Cancellation: `cancel_pagare(k)` burns against payment

- Anyone may call it once the coverage rule shows pagaré `k` paid; the program burns the asset,
  closes the sidecar with rent to the relayer, and emits `PagareCancelled`. This is the art. 129
  delivery of the paid título, executed in the system. It fails while the line is `InCollection`
  or `WrittenOff`: a defaulted pagaré stays in the holder's possession for cobranza, never goes
  back to the debtor (Tinlake's unlock-to-borrower is the pattern to avoid).

### Endoso: `endorse_pagare(k, kind, endoso_hash, to)`

- Signers: the current holder's key (the vehicle's Squads vault while the asset is in the pool;
  the endorsee's own key once it has left) and the attestor (the engine recorded the endoso with
  endorsee name, type, place and date, art. 29, and holds the signed record whose hash is
  `endoso_hash`).
- `kind` is `Propiedad`, `Procuración` or `Garantía` (art. 33). `Procuración` transfers custody to
  a despacho's wallet for collection without transferring ownership, and the program marks the
  asset so it can only come back or be returned; `Propiedad` transfers ownership.
- `to` must hold a valid SAS attestation under Elote's credential (an identified persona), or the
  instruction fails: no pagaré ever becomes al portador.
- After maturity the endoso has the effects of a cesión (art. 37); the program records `kind` and
  the time, and the debtor notice and RUG registration happen in the engine and off chain
  (`M-pagares-recovery.md` §2.2). Investor-to-investor trades of series shares never touch a
  pagaré.
- The program appends `{kind, endoso_hash, from, to, at}` to the asset's `AppData` and transfers
  it with `TransferV1` through the permanent delegate; emits `PagareEndorsed`.

### Consultation: the verifier judges can use

A public, read-only page that takes a pagaré asset address and shows: the issuance event, the
hash, the amount and dates, the full endoso chain and the current holder, and lets a party who is
entitled (holder, debtor, court) fetch the data message from the engine with its NOM-151
constancia. Arts. 47 and 54 assume the judge can consult the system; the page plus a fe de hechos
by a corredor or notario is how (`M-pagares-recovery.md` §1.6).

### Amendments

A convenio modificatorio is a new data message: `amend_line(new_terms_hash)` proposed by the admin
and accepted by the borrower's signature before a deadline (Maple's two-party hash commitment,
evm pattern 10). Out of scope for the first batch; the layout leaves room.

### Trusted roles, new

Written into `SECURITY.md`'s trust table and `ARCHITECTURE.md` §8: the **attestor** can block every
disbursement and every endoso by withholding its signature and cannot move funds or títulos;
`set_attestor` by the admin is the recovery. The **series vehicle's key** (the Squads vault) signs
endosos out of the pool, which makes it a gate on every sale of paper (counsel item 10's list of
admin powers). Metaplex Core's upgrade authority becomes a dependency of issuance and endoso.

### What does not change

The pool's NAV counts outstanding principal, never a document valuation; there is no oracle.
Investor gating (Token ACL) and the SAS attestations are untouched, and the attestation is what
makes a wallet an eligible endorsee. `repay` stays permissionless. Series shares never reference a
pagaré.

## Changes to the tranched plan

- D9 becomes the issuance above: `draw` mints the pagaré asset and its sidecar; the hash is in the
  asset, not only in an event.
- New D14: attestor co-signature on `draw` and on `endorse_pagare`; `attestor: Pubkey` on the
  Pool with `set_attestor`; `collection: Pubkey` on the Pool for the series' Core collection. The
  Pool grows by 64 bytes in the pool v2 batch (the series-shelf facts table and item 2d say so).
- New D15: `expediente_root` on `CreditLine` (32 bytes more; the v3 growth becomes 51 bytes).
- New instructions: `cancel_pagare`, `endorse_pagare`, `set_attestor`. New errors:
  `AttestorRequired`, `PagareNotCovered`, `NotHolder`, `EndorseeNotAttested`.
- Tests: a draw without the attestor fails; one asset and one sidecar per draw, frozen, owned by
  the pool; coverage pays oldest first and `cancel_pagare` of a later pagaré fails while an earlier
  one is uncovered; a written-off or in-collection line keeps its pagarés and `cancel_pagare`
  fails; an endoso to a wallet without attestation fails; `Procuración` cannot be re-endorsed `en
  propiedad` by the despacho; the endoso log in `AppData` matches the events; `repay`,
  `mark_in_collection` and `write_off` take no pagaré accounts; compute stays under the ceiling
  with the Core CPI on `draw`.
- Estimate: 2.5 days more on the program and tests (Core CPI, endoso, cancel), half a day on the
  agreements engine (salted root, attestor signature, endoso records), one day for the verifier
  page. Core's CPI lands in `draw`, so it ships with the tranched batch, not after the review.

## Decisions taken

| # | Decision |
|---|---|
| 1 | The token is the pagaré: one Core asset per disposición, issued by the pool at `draw`, burned at payment, endorsed through the program. |
| 2 | The sistema de información is Solana plus the program plus the agreements engine, designated as such in the pagaré text. |
| 3 | Holders are identified personas; while the loan is alive the holder is the series vehicle and the pool PDA is its custody account; endorsees must carry Elote's SAS attestation. |
| 4 | A draw and an endoso need the attestor's signature; no título is issued or moved against an unattested hash. |
| 5 | Defaulted pagarés stay with the holder; paid ones are burned against payment. |
| 6 | Only hashes, salted commitments, amounts, dates and keys go on chain; the text lives in the engine and is served by entitlement. |
| 7 | Token-2022, Light and a plain record without a token are rejected for the título; the sidecar record stays for the program's logic. |

## For counsel (consolidated list, item 16, reframed)

Elote's position is that the Core asset issued by the program is the electronic pagaré under
LGTOC arts. 5o and 5o Bis, with the chain, the program and the agreements engine as the sistema
de información. Counsel is asked to confirm or correct: (a) the designation of that system in the
pagaré text as meeting arts. 17 and 27 while no CNBV rules exist; (b) the signature method the
template must fix so that the borrower's signature is attributable under CCom art. 97 and tesis
2031391, and whether the attestor plus the NOM-151 constancia suffice when a wallet key signs;
(c) that the series vehicle, holding through the pool PDA, is the tenedor, and how a judge
verifies the last holder (art. 39) from the endoso log; (d) that burn against full payment is the
art. 129 cancellation; (e) the effects of a post-maturity endoso and the notice it needs;
(f) the art. 5o Bis gate in a juicio ejecutivo mercantil. These extend items 6 and 15.
