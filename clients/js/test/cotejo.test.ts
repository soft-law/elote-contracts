// Decision 56: a pool may name up to three cotejo keys, and then one of them co-signs every draw.
// Decision 57: the draw carries the sealed copy's hash as its last argument. The offsets are the
// ones tests/reference states for the Rust side (`reference::layout`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { AccountRole, address, generateKeyPairSigner, getAddressEncoder } from "@solana/kit";
import {
  ELOTE_POOL_PROGRAM_ADDRESS,
  NO_SEALED_HASH,
  getDrawInstruction,
  getPagareEncoder,
  getPagareSize,
  getPoolEncoder,
  getPoolSize,
  poolCotejoKeys,
} from "../src/index";

const zero = address("11111111111111111111111111111111");
const k1 = address("8hoyNNMkn7XJDyJ7EuNJtvobmbrNbHEaULHUcVX1Qku9");
const k2 = address("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
const POOL_COTEJO_KEYS = 447; // reference::layout::POOL_COTEJO_KEYS
const PAGARE_COTEJO = 435; // reference::layout::PAGARE_COTEJO
const PAGARE_SEALED_HASH = 467; // reference::layout::PAGARE_SEALED_HASH

test("poolCotejoKeys drops the empty slots", () => {
  assert.deepEqual(poolCotejoKeys({ cotejoKeys: [zero, k1, zero] }), [k1]);
  assert.deepEqual(poolCotejoKeys({ cotejoKeys: [k1, zero, k2] }), [k1, k2]);
  assert.deepEqual(poolCotejoKeys({ cotejoKeys: [zero, zero, zero] }), []);
});

test("the pool and the pagaré carry the new fields at the reference offsets", () => {
  assert.equal(getPoolSize(), 591);
  assert.equal(getPagareSize(), 527);
  const b32 = (v: number) => new Uint8Array(32).fill(v);
  const pool = getPoolEncoder().encode({
    council: zero, assetMint: zero, shareMint: zero, vault: zero, deadShares: zero, collection: zero,
    outstandingPrincipal: 0n, interestCollected: 0n, writtenOff: 0n, lineCount: 0n, collectionPeriodSeconds: 0,
    maxLineBps: 0, assetControls: 0, permissioned: false, paused: false, bump: 0, creator: zero, councilKind: 0,
    multisig: zero, councilVaultBump: 0, pendingCouncil: zero, pendingCouncilKind: 0, maxOutstandingPrincipal: 0n,
    accelerationNoticeSeconds: 0, impairedPrincipal: 0n, provisionBps: 0, fxFeeder: zero, fixRate: 0n, fixDate: 0,
    fixPostedAt: 0n, fxRealized: 0n, minFixRate: 0n, maxFixRate: 0n, recoverable: 0n,
    cotejoKeys: [zero, k1, k2], reserved: new Uint8Array(48),
  });
  const enc = getAddressEncoder();
  assert.deepEqual(Buffer.from(pool.slice(POOL_COTEJO_KEYS + 32, POOL_COTEJO_KEYS + 64)), Buffer.from(enc.encode(k1)));
  assert.deepEqual(Buffer.from(pool.slice(POOL_COTEJO_KEYS + 64, POOL_COTEJO_KEYS + 96)), Buffer.from(enc.encode(k2)));
  const pagare = getPagareEncoder().encode({
    line: zero, drawIndex: 0, asset: zero, pagareHash: b32(0), amountMxn: 0n, issuedAt: 0n, dueAt: 0n,
    drawnThrough: 0n, certFingerprint: b32(0), signatureMethod: 0, status: 0, paidAt: 0n, receiptHash: b32(0),
    procurador: zero, flags: 0, logRoot: b32(0), entryCount: 0, lastKind: 0, bump: 0, fixRate: 0n, fixDate: 0,
    usdcDelivered: 0n, deliveryHash: b32(0), signedPagareHash: b32(0), pagareSignature: new Uint8Array(64), signedOn: 0,
    cotejo: k1, sealedHash: b32(0x57), reserved: new Uint8Array(28),
  });
  assert.deepEqual(Buffer.from(pagare.slice(PAGARE_COTEJO, PAGARE_COTEJO + 32)), Buffer.from(enc.encode(k1)));
  assert.deepEqual(Buffer.from(pagare.slice(PAGARE_SEALED_HASH, PAGARE_SEALED_HASH + 32)), Buffer.from(b32(0x57)));
});

async function drawWith(cotejo: Parameters<typeof getDrawInstruction>[0]["cotejo"], sealedHash: Uint8Array) {
  const borrower = await generateKeyPairSigner();
  return getDrawInstruction({
    borrower, cotejo, pool: k1, line: k1, assetMint: k1, vault: k1, borrowerAssetAccount: k1, payer: borrower,
    asset: k2, pagare: k2, collection: k2, eventAuthority: k2,
    amountMxn: 1n, fixRate: 1n, fixDate: 20261111, assetBump: 255, pagareBump: 255, certFingerprint: new Uint8Array(32),
    signatureMethod: 0, signedPagareHash: new Uint8Array(32).fill(1), pagareSignature: new Uint8Array(64).fill(2), signedOn: 20261111,
    sealedHash,
  });
}

test("draw: a cotejo signer co-signs read only; without one the slot holds the program id", async () => {
  const cotejo = await generateKeyPairSigner();
  const signed = await drawWith(cotejo, new Uint8Array(NO_SEALED_HASH));
  assert.equal(signed.accounts[1].address, cotejo.address);
  assert.equal(signed.accounts[1].role, AccountRole.READONLY_SIGNER);
  const none = await drawWith(undefined, new Uint8Array(NO_SEALED_HASH));
  assert.equal(none.accounts[1].address, ELOTE_POOL_PROGRAM_ADDRESS);
  assert.equal(none.accounts[1].role, AccountRole.READONLY);
});

test("draw: the sealed hash is the last 32 bytes of the data, as given", async () => {
  const sealed = new Uint8Array(32).map((_, i) => i);
  const ix = await drawWith(undefined, sealed);
  assert.deepEqual(Buffer.from(ix.data.slice(-32)), Buffer.from(sealed));
});
