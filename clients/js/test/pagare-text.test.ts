// The borrower's wallet key signs the pagaré text itself, with no domain separation, and the
// signature is published on chain. The guard refuses anything that is not plain text, so the key
// never signs bytes that could be a Solana transaction message.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  AccountRole,
  type Address,
  type Blockhash,
  appendTransactionMessageInstruction,
  compileTransactionMessage,
  createTransactionMessage,
  generateKeyPairSigner,
  getCompiledTransactionMessageEncoder,
  pipe,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  signBytes,
} from "@solana/kit";
import * as pagare from "../src/pagare";
import * as client from "../src/index";

const PAGARE_TEXT = `PAGARÉ No. 7-0

Debo y pagaré incondicionalmente a la orden de ELOTE, en la Ciudad de México, la cantidad de
$12,500.00 (doce mil quinientos pesos 00/100 M.N.), el día 11 de diciembre de 2026.
Valor recibido a mi entera satisfacción. Interés ordinario: 3.5% mensual sobre saldos insolutos.
Suscriptor: línea 7, disposición 0. Firmado el 11 de noviembre de 2026. Nonce: 9f2c…
`;

const enc = (s: string) => new TextEncoder().encode(s);
const SYSTEM = "11111111111111111111111111111111" as Address;

async function transactionMessageBytes(version: 0 | "legacy"): Promise<Uint8Array> {
  const payer = await generateKeyPairSigner();
  const to = await generateKeyPairSigner();
  const transfer = {
    programAddress: SYSTEM,
    accounts: [
      { address: payer.address, role: AccountRole.WRITABLE_SIGNER },
      { address: to.address, role: AccountRole.WRITABLE },
    ],
    data: Uint8Array.of(2, 0, 0, 0, 0, 0xe4, 0x0b, 0x54, 2, 0, 0, 0), // SystemInstruction::Transfer 10 SOL
  };
  const message = pipe(
    createTransactionMessage({ version }),
    (m) => setTransactionMessageFeePayer(payer.address, m),
    (m) =>
      setTransactionMessageLifetimeUsingBlockhash(
        { blockhash: "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG" as Blockhash, lastValidBlockHeight: 0n },
        m
      ),
    (m) => appendTransactionMessageInstruction(transfer, m)
  );
  return new Uint8Array(getCompiledTransactionMessageEncoder().encode(compileTransactionMessage(message)));
}

test("assertPagareMessageText is exported from the client's index", () => {
  assert.equal(typeof client.assertPagareMessageText, "function");
});

test("a Spanish pagaré text with accents and newlines passes, and so does the demo placeholder", () => {
  pagare.assertPagareMessageText(enc(PAGARE_TEXT));
  pagare.assertPagareMessageText(enc("DEMO pagaré message, devnet, line 7 draw 0 20261111. Debo y pagaré incondicionalmente…"));
  pagare.assertPagareMessageText(
    enc("DEMO pagaré message, not a pagaré text: localnet, line 3 (4vzxVAuc1nczBTEPMBpGXiB7W3PyuRikEMbdd4m7zUcq) draw 0 20261112")
  );
});

test("a serialized transaction message, legacy or v0, is refused", async () => {
  for (const version of ["legacy", 0] as const) {
    const bytes = await transactionMessageBytes(version);
    assert.throws(() => pagare.assertPagareMessageText(bytes), /not plain text/, `version ${version}`);
  }
});

test("empty bytes, invalid UTF-8 and control or invisible characters are refused", () => {
  const refused: [string, Uint8Array][] = [
    ["empty", new Uint8Array(0)],
    ["NUL", enc("Debo y pagaré\u0000")],
    ["CR", enc("Debo y pagaré\r\nincondicionalmente")],
    ["tab", enc("Debo\ty pagaré")],
    ["DEL", enc("Debo y pagaré\u007f")],
    ["C1", enc("Debo y pagaré\u0085")],
    ["zero-width space", enc("Debo y​ pagaré")],
    ["bidi override", enc("Debo y ‮pagaré")],
    ["word joiner", enc("Debo y⁠ pagaré")],
    ["leading BOM", enc("﻿Debo y pagaré")],
    ["invalid UTF-8", Uint8Array.of(0x44, 0x65, 0x62, 0x6f, 0xc3, 0x28)],
    ["lone continuation byte", Uint8Array.of(0x80, 0x41)],
  ];
  for (const [name, bytes] of refused) {
    assert.throws(() => pagare.assertPagareMessageText(bytes), /not plain text/, name);
  }
});

test("verifyPagareSignature is false for a good signature over bytes that are not plain text", async () => {
  const borrower = await generateKeyPairSigner();
  for (const bytes of [await transactionMessageBytes(0), enc("Debo y pagaré\u0000"), enc("Debo y​ pagaré")]) {
    const signature = await signBytes(borrower.keyPair.privateKey, bytes);
    assert.equal(await pagare.verifyPagareSignature({ message: bytes, signature, borrower: borrower.address }), false);
  }
  // A string carrying a control character is refused the same way.
  const text = "Debo y pagaré\u0000";
  const signature = await signBytes(borrower.keyPair.privateKey, enc(text));
  assert.equal(await pagare.verifyPagareSignature({ message: text, signature, borrower: borrower.address }), false);
});
