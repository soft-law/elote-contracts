// Salted hashes for what goes on chain about a person's documents: the agreement and receipt
// hashes, and the commitment to a signing certificate. A plain hash of a document can be matched
// by anyone who holds or can rebuild the document; a salt kept off chain (in the expediente) stops
// that, and the SAT publishes e.firma certificates by RFC, so a bare fingerprint would name the
// borrower.

async function sha256(parts: Uint8Array[]): Promise<Uint8Array> {
  const total = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    total.set(p, at);
    at += p.length;
  }
  return new Uint8Array(await crypto.subtle.digest("SHA-256", total));
}

/** `sha256(salt || bytes)`: the convention for `agreement_hash` and `receipt_hash`. */
export const saltedHash = (salt: Uint8Array, bytes: Uint8Array): Promise<Uint8Array> => sha256([salt, bytes]);

/**
 * The `cert_fingerprint` a draw records for a PSC or e.firma signature: `sha256(salt ||
 * fingerprint)`, never the fingerprint itself. The salt stays in the expediente.
 */
export const certCommitment = (salt: Uint8Array, fingerprint: Uint8Array): Promise<Uint8Array> => sha256([salt, fingerprint]);
