// The salted hashes, the FIX helpers and the procurador offset the web's lookup relies on.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { address, getAddressEncoder } from "@solana/kit";
import { certCommitment, saltedHash } from "../src/hashes";
import { mexicoCityDate, parseFix } from "../src/fx";
import { PAGARE_PROCURADOR_OFFSET } from "../src/pagare";
import { getPagareEncoder } from "../src/generated";

test("salted hashes are sha256(salt || bytes)", async () => {
  const salt = new Uint8Array(32).fill(0x5a);
  const fingerprint = new Uint8Array(32).fill(0xcf);
  const expected = createHash("sha256").update(salt).update(fingerprint).digest();
  assert.deepEqual(Buffer.from(await certCommitment(salt, fingerprint)), expected);
  assert.deepEqual(Buffer.from(await saltedHash(salt, fingerprint)), expected);
  assert.notDeepEqual(Buffer.from(await certCommitment(salt, fingerprint)), createHash("sha256").update(fingerprint).digest());
});

test("the FIX parses at four decimals and dates follow Mexico City", () => {
  assert.equal(parseFix("18.3740"), 183_740n);
  assert.equal(parseFix("18.374"), 183_740n);
  assert.equal(parseFix("20"), 200_000n);
  assert.throws(() => parseFix("18.37401"));
  assert.throws(() => parseFix("0"));
  assert.equal(mexicoCityDate(1_793_707_200n), 20261103, "2026-11-03T12:00Z");
  assert.equal(mexicoCityDate(1_793_707_200n + 15n * 3_600n), 20261103, "03:00Z on the 4th is still the 3rd in Mexico City");
  assert.equal(mexicoCityDate(1_793_707_200n + 18n * 3_600n), 20261104);
});

test("procurador sits at the offset the lookup filters on", () => {
  const procurador = address("8hoyNNMkn7XJDyJ7EuNJtvobmbrNbHEaULHUcVX1Qku9");
  const zero = address("11111111111111111111111111111111");
  const bytes = getPagareEncoder().encode({
    line: zero, drawIndex: 0, asset: zero, pagareHash: new Uint8Array(32), amountMxn: 0n, issuedAt: 0n, dueAt: 0n,
    drawnThrough: 0n, certFingerprint: new Uint8Array(32), signatureMethod: 0, status: 0, paidAt: 0n,
    receiptHash: new Uint8Array(32), cancelReceiptHash: new Uint8Array(32), procurador, flags: 0,
    logRoot: new Uint8Array(32), entryCount: 0, lastKind: 0, bump: 0, fixRate: 0n, fixDate: 0, usdcDelivered: 0n,
    deliveryHash: new Uint8Array(32), cancelGraceSeconds: 0, reserved: new Uint8Array(28),
  });
  const key = Buffer.from(getAddressEncoder().encode(procurador));
  assert.equal(Buffer.from(bytes).indexOf(key), PAGARE_PROCURADOR_OFFSET);
});
