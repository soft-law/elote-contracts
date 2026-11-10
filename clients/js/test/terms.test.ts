// The shared terms-hash vector: the same inputs and expected hash as
// `programs/elote_pool/src/terms.rs` (`the_terms_hash_matches_the_pinned_vector`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { type Address, getAddressDecoder } from "@solana/kit";
import { lineSponsor, termsHash } from "../src/terms";

const key = (byte: number) => getAddressDecoder().decode(new Uint8Array(32).fill(byte));
const fill = (byte: number) => new Uint8Array(32).fill(byte);

test("the terms hash matches the program's pinned vector", async () => {
  const h = await termsHash(key(0x11), 7n, key(0x22), {
    principal: 5_000_000n,
    rateBps: 2_400,
    lateBps: 5_000,
    termEnd: 1_800_000_000n,
    agreementHash: fill(0xa7),
    sponsor: key(0x55),
    sponsorRole: 1,
    schedule: { firstDueAt: 1_770_000_000n, intervalSeconds: 2_592_000n, installments: 12, maxMissed: 3 },
    expedienteRoot: fill(0xe7),
    pagareTemplateHash: fill(0x7e),
  });
  assert.equal(Buffer.from(h).toString("hex"), "aac738788a0f850ca79bf7c0513d07af3a13e2fb6bff4758273e82d46b98cbfc");
});

test("lineSponsor reads the zero key as no sponsor, as the terms hash does", () => {
  const someone = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf" as Address;
  assert.equal(lineSponsor({ sponsor: "11111111111111111111111111111111" as Address }), null);
  assert.equal(lineSponsor({ sponsor: someone }), someone);
});
