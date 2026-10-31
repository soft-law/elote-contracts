# KYC / identity proofs of concept

Throwaway code for the 2026-10-30 evaluation of KYC/KYB providers and on-chain identity for Elote.
Sandbox and devnet only, with no real personal data. Nothing here is imported by the program or the
client, and it has its own `package.json` so it does not touch the root `node_modules`.

```bash
cd poc
npm install --ignore-scripts
cp .env.example .env        # .env is ignored by the root .gitignore; never commit it
npm test                    # offline: Sumsub + Didit signing and webhook verification, synthetic secrets
npm run sas                 # devnet: credential, schema, issue, verify, expiry, revoke
npm run sns                 # mainnet read-only: name resolution and primary domain
npm run sumsub              # sandbox: applicant + WebSDK token (needs SUMSUB_* sandbox keys)
npm run didit               # sandbox: hosted verification session (needs DIDIT_* sandbox keys)
npm run etherfuse           # sandbox: 100 MXN -> CETES onramp to a devnet wallet (needs ETHERFUSE_* sandbox key)
```

Node 24 runs the `.ts` files directly (type stripping). No build step.

## What each PoC shows

| PoC | Files | Code lines | Result |
|---|---|---|---|
| Sumsub | `sumsub/sumsub.ts`, `live.ts`, `sumsub.test.ts` | 69 + 19 (+ 39 test) | Request signing (HMAC-SHA256 over ts + METHOD + path + body) and `X-Payload-Digest` webhook verification for SHA1/256/512 pass offline. Live applicant + WebSDK token: **pending sandbox keys**. |
| Didit | `didit/didit.ts`, `live.ts`, `didit.test.ts` | 41 + 11 (+ 35 test) | Session creation and webhook verification (`X-Signature-V2` over Python-canonical JSON, `X-Signature` over raw bytes, 300 s window) pass offline; the canonical form was checked against Python's `json.dumps`. Live session: **pending sandbox keys**. |
| SAS | `sas/flow.ts` | 156 | Ran on devnet 2026-10-30 in 82 s, 5 transactions. See below. |
| Etherfuse CETES | `etherfuse/cetes-onramp.ts` | 60 | Sandbox onramp of 100 MXN into CETES on a devnet wallet, after listing the Solana assets a quote may use. **Not run yet: needs a sandbox API key** (`ETHERFUSE_*` in `.env`). Review: `docs/plans/2026-10-31-cetes-investment-product.md`. |
| SNS | `sns/resolve.ts` | 30 | SDK v4 (`@bonfida/spl-name-service` 4.0.1) refuses `.sol` on mainnet since slot 452,825,395 (`UnsupportedTldError`); `bonfida.sns` resolves to `Fw1ETanDZafof7xEULsnq9UY6o71Tpds89tNwPkWLb1v` in ~300 ms. `getPrimaryDomain`: **pending a private RPC**, the public endpoint answers 429 for that call. The SDK pulls 11 npm advisories (4 high, via web3.js 1.x, `bigint-buffer`, `@solana/spl-token`). |

## SAS devnet run (2026-10-30)

Schema `ELOTE-INVESTOR-KYC` v1: `jurisdiction` (String, ISO country) and `verified_at` (I64). No
name, document number or provider id goes on chain. It is the same claim layout that Moonsong's
`token-acl-sas-gate` reads, so these attestations can later drive the Token ACL thaw of a series'
share mint (see `docs/plans/2026-10-24-series-shelf-contract-plan.md`).

- credential `F4nr2HpuMg6ucFCe4jwZxirfL8J7safG9svqydAA1wzi`, schema `CLgXkpeGwVqFMJtG4AH1L1XSEsx8C6Lgm9MWgiRTWYWg`
- investor attestation verified `ok`; a random wallet is refused; a 1-second max-age policy refuses it
- an attestation with a 20 s expiry is refused after 30 s **while its account still exists on chain**:
  SAS checks `expiry` only at creation (`program/src/processor/create_attestation.rs:47`), so every
  verifier must compare it with the clock itself
- after `CloseAttestation` (revocation) the account is gone and verification fails

The `verify()` function in `sas/flow.ts` lists every check a verifier needs: program owner, schema
credential, paused flag, PDA derived from (credential, schema, wallet), credential/schema/nonce
match, expiry against the cluster clock, and claim age.

## Friction log

- **Sumsub**: signing is 10 lines with `node:crypto`. The webhook digest must be computed on the raw
  body bytes; a framework that parses JSON first breaks it. The live part needs a sandbox app token
  (`sbx:` prefix, enforced by `live.ts`) and a level created in the dashboard.
- **Didit**: the recommended `X-Signature-V2` signs Python's `json.dumps(sort_keys=True,
  separators=(",",":"), ensure_ascii=False)` after turning whole floats into ints. Node needs a
  re-implementation; known divergences (`1e-07` vs `1e-7`, non-BMP key order) are documented in
  `didit.ts`. The KYB registry callback is unsigned (Didit skill `SKILL.md:658`).
- **SAS**: the official example (`examples/typescript/attestation-flow-guides/src/kit/shared.ts`)
  stores name, age and country on chain; don't copy its schema. The current client package is
  `@solana/attestation` (formerly `sas-lib`). The `@solana/kit-plugin-*` packages the example
  uses were published the same day as this run, so the PoC uses plain `@solana/kit` 8.5 instead.
- **SNS**: `safeResolve` exists (`js/src/resolve/index.ts:88`), but on mainnet today it throws
  `UnsupportedTldError` for every `.sol` name, because the SDK pauses `.sol` after a fixed slot
  (`js/src/config.ts:18`, `js/src/utils/assertTldSupported.ts`). The v4 JS SDK is on
  `@solana/web3.js` 1.x; the kit variant (`@solana-name-service/sns-sdk-kit` 1.0.1) targets kit 6.
