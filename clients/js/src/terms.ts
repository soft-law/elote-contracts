// The terms hash the council approves (`approve_line` takes it and refuses a mismatch). The program
// computes it in `request_line` from the stored terms (programs/elote_pool/src/terms.rs); the
// curators' tooling computes it from the documents with this function. One shared vector pins both.
import { type Address, type ReadonlyUint8Array, getAddressEncoder } from "@solana/kit";
import type { ScheduleParamsArgs } from "./generated";

/** `CreditLine.currency`: Mexican pesos, ISO 4217 484. Every amount of a line is in centavos. */
export const CURRENCY_MXN = 484;

const DOMAIN = new TextEncoder().encode("elote_pool:terms:v1");
const ZERO_KEY = "11111111111111111111111111111111" as Address;

export type LineTermsInput = {
  /** Centavos of MXN. */
  principal: bigint;
  rateBps: number;
  /** Annual late interest, bullet or scheduled. */
  lateBps: number;
  termEnd: bigint;
  agreementHash: ReadonlyUint8Array;
  sponsor: Address | null;
  sponsorRole: number;
  schedule: ScheduleParamsArgs | null;
  expedienteRoot: ReadonlyUint8Array;
  pagareTemplateHash: ReadonlyUint8Array;
};

function le(value: bigint | number, bytes: number, signed = false): Uint8Array {
  const out = new Uint8Array(bytes);
  const view = new DataView(out.buffer);
  const v = BigInt(value);
  if (bytes === 1) view.setUint8(0, Number(v));
  else if (bytes === 2) view.setUint16(0, Number(v), true);
  else if (signed) view.setBigInt64(0, v, true);
  else view.setBigUint64(0, v, true);
  return out;
}

/**
 * `sha256("elote_pool:terms:v1" || pool || line_id u64 || borrower || currency u16 || principal u64 ||
 * rate_bps u16 || term_end i64 || first_due_at i64 || interval_seconds i64 || installments u16 ||
 * late_bps u16 || max_missed u8 || sponsor_present u8 || sponsor (zero key when none) || sponsor_role u8
 * || agreement_hash || pagare_template_hash || expediente_root)`, little endian, schedule fields zero
 * on a bullet line.
 */
export async function termsHash(
  pool: Address,
  lineId: bigint,
  borrower: Address,
  t: LineTermsInput,
  currency: number = CURRENCY_MXN
): Promise<Uint8Array> {
  const key = getAddressEncoder();
  const s = t.schedule ?? { firstDueAt: 0, intervalSeconds: 0, installments: 0, maxMissed: 0 };
  const parts: ReadonlyUint8Array[] = [
    DOMAIN,
    key.encode(pool),
    le(lineId, 8),
    key.encode(borrower),
    le(currency, 2),
    le(t.principal, 8),
    le(t.rateBps, 2),
    le(t.termEnd, 8, true),
    le(s.firstDueAt, 8, true),
    le(s.intervalSeconds, 8, true),
    le(s.installments, 2),
    le(t.lateBps, 2),
    le(s.maxMissed, 1),
    le(t.sponsor ? 1 : 0, 1),
    key.encode(t.sponsor ?? ZERO_KEY),
    le(t.sponsorRole, 1),
    t.agreementHash,
    t.pagareTemplateHash,
    t.expedienteRoot,
  ];
  const message = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    message.set(p, at);
    at += p.length;
  }
  return new Uint8Array(await crypto.subtle.digest("SHA-256", message));
}
