# @softlaw/elote-pool-client

TypeScript client for the `elote_pool` Solana program, generated with [Codama](https://github.com/codama-idl/codama) on `@solana/kit`. Program id `BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM` (devnet).

```sh
npm install @softlaw/elote-pool-client @solana/kit
```

```ts
import { ELOTE_POOL_PROGRAM_ADDRESS, fetchPool, findVaultPda } from "@softlaw/elote-pool-client";
```

ESM only. `@solana/kit` is a peer dependency so the app and the client share one copy. `schedule.ts` mirrors the program's installment and arrears math for off-chain previews.

Source, program and docs: https://github.com/soft-law/elote-contracts. Every version is published from that repository's `release-client.yml` with npm provenance.
