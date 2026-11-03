// The Metaplex Core collection a pool's pagarés are minted into. The expected bytes are the
// instruction the mpl-core crate 0.12.1 builds (`CreateCollectionV2Builder`, the builder
// `programs/elote_pool/tests/common/mod.rs::create_collection` uses), printed from Rust with the
// same arguments. The Rust suite runs that builder against the pinned Core binary 0.15.3
// (`tests/fixtures/mpl_core_program-0.15.3.so`), which validates it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { AccountRole, type Address, createNoopSigner, getAddressDecoder } from "@solana/kit";
import { MPL_CORE_PROGRAM_ADDRESS, getCreateCollectionV2Instruction } from "../src/core";

const key = (byte: number) => getAddressDecoder().decode(new Uint8Array(32).fill(byte)) as Address;

test("getCreateCollectionV2Instruction encodes what the mpl-core crate builds", () => {
  const ix = getCreateCollectionV2Instruction({
    collection: createNoopSigner(key(1)),
    updateAuthority: key(2),
    payer: createNoopSigner(key(3)),
    name: "ELOTE-CR-TEST",
    uri: "",
  });
  assert.equal(ix.programAddress, MPL_CORE_PROGRAM_ADDRESS);
  assert.deepEqual(
    ix.accounts!.map((m) => [m.address, m.role]),
    [
      ["4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi", AccountRole.WRITABLE_SIGNER],
      ["8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR", AccountRole.READONLY],
      ["CktRuQ2mttgRGkXJtyksdKHjUdc2C4TgDzyB98oEzy8", AccountRole.WRITABLE_SIGNER],
      ["11111111111111111111111111111111", AccountRole.READONLY],
    ]
  );
  // discriminator 21 | name | uri | plugins: Some([]) | external plugin adapters: None
  assert.equal(
    Buffer.from(ix.data as Uint8Array).toString("hex"),
    "150d000000454c4f54452d43522d5445535400000000010000000000"
  );
});
