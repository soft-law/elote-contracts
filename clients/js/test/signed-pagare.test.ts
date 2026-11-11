// Decision 55: the borrower subscribes the pagaré by signing, off chain, a message with its full
// text; the record keeps the message's SHA-256 and the signature, and the issuance entry binds the
// hash. The program cannot see the text, so it verifies nothing: these helpers let anyone who holds
// the text check a record against the borrower's address.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { address, generateKeyPairSigner, getAddressEncoder, signBytes } from "@solana/kit";
import { issuanceDocumentHash, registryEntryHash, signedPagareHash, verifyPagareSignature } from "../src/pagare";

const MESSAGE = "DEMO pagaré message, devnet, line 7 draw 0 20261111. Debo y pagaré incondicionalmente…";
const KIND_EMISION = 0; // state::KIND_EMISION

const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");

test("signedPagareHash is SHA-256 over the message's UTF-8 bytes, text or bytes", async () => {
  const expected = createHash("sha256").update(Buffer.from(MESSAGE, "utf8")).digest("hex");
  assert.equal(hex(await signedPagareHash(MESSAGE)), expected);
  assert.equal(hex(await signedPagareHash(new TextEncoder().encode(MESSAGE))), expected);
  assert.equal(hex(await signedPagareHash("DEMO pagaré")), "65a2dfe64c8cfaa5711fea0c7d0ba9fa187fec9eb14320ef04b1248c8feb17af");
});

test("verifyPagareSignature accepts the borrower's signature over the text and its hash", async () => {
  const borrower = await generateKeyPairSigner();
  const bytes = new TextEncoder().encode(MESSAGE);
  const signature = await signBytes(borrower.keyPair.privateKey, bytes);
  const hash = await signedPagareHash(MESSAGE);
  assert.equal(await verifyPagareSignature({ message: MESSAGE, signature, borrower: borrower.address }), true);
  assert.equal(await verifyPagareSignature({ message: bytes, signature, borrower: borrower.address, signedPagareHash: hash }), true);
});

test("verifyPagareSignature refuses another key", async () => {
  const borrower = await generateKeyPairSigner();
  const other = await generateKeyPairSigner();
  const signature = await signBytes(other.keyPair.privateKey, new TextEncoder().encode(MESSAGE));
  assert.equal(await verifyPagareSignature({ message: MESSAGE, signature, borrower: borrower.address }), false);
});

test("verifyPagareSignature refuses an altered text, and a text that does not match the record's hash", async () => {
  const borrower = await generateKeyPairSigner();
  const signature = await signBytes(borrower.keyPair.privateKey, new TextEncoder().encode(MESSAGE));
  const altered = MESSAGE.replace("line 7", "line 8");
  assert.equal(await verifyPagareSignature({ message: altered, signature, borrower: borrower.address }), false);
  // The signature is good but the record committed to another text.
  const otherHash = await signedPagareHash(altered);
  assert.equal(
    await verifyPagareSignature({ message: MESSAGE, signature, borrower: borrower.address, signedPagareHash: otherHash }),
    false
  );
  // A malformed signature is a refusal, not an exception.
  assert.equal(await verifyPagareSignature({ message: MESSAGE, signature: new Uint8Array(63), borrower: borrower.address }), false);
});

test("issuanceDocumentHash is sha256(pagare_hash || signed_pagare_hash), the issuance entry's hash", async () => {
  const pagareHash = new Uint8Array(32).fill(0x11);
  const signed = new Uint8Array(32).fill(0x22);
  // Pinned with the program's `registry::issuance_hash` (tests/pagare.rs).
  assert.equal(hex(await issuanceDocumentHash(pagareHash, signed)), "5189c77d29fe5d546a045ec46986852785fea5c13ac7da9c115ff5fb6edf817c");
});

test("the registry's first root depends on the signed pagaré hash", async () => {
  const asset = address("4vzxVAuc1nczBTEPMBpGXiB7W3PyuRikEMbdd4m7zUcq");
  const pool = address("11111111111111111111111111111113");
  const pagareHash = new Uint8Array(32).fill(0x11);
  const genesis = async (signed: Uint8Array) => {
    const entry = await registryEntryHash({
      asset, kind: KIND_EMISION, from: pool, to: pool, hash: await issuanceDocumentHash(pagareHash, signed), statedAt: 1_793_736_000n, entryCount: 1,
    });
    return createHash("sha256").update(new Uint8Array(32)).update(entry).digest("hex");
  };
  // registryEntryHash against an independent derivation.
  const enc = getAddressEncoder();
  const doc = await issuanceDocumentHash(pagareHash, new Uint8Array(32).fill(0x22));
  const stated = Buffer.alloc(8);
  stated.writeBigInt64LE(1_793_736_000n);
  const count = Buffer.alloc(4);
  count.writeUInt32LE(1);
  const manual = createHash("sha256")
    .update(Uint8Array.from(enc.encode(asset))).update(Uint8Array.of(KIND_EMISION))
    .update(Uint8Array.from(enc.encode(pool))).update(Uint8Array.from(enc.encode(pool)))
    .update(doc).update(stated).update(count).digest("hex");
  assert.equal(
    hex(await registryEntryHash({ asset, kind: KIND_EMISION, from: pool, to: pool, hash: doc, statedAt: 1_793_736_000n, entryCount: 1 })),
    manual
  );
  assert.notEqual(await genesis(new Uint8Array(32).fill(0x22)), await genesis(new Uint8Array(32).fill(0x23)));
});
