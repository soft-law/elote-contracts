// Reads the packed `reserved` bytes of `Pool` and `CreditLine` and computes the arrears of a
// scheduled line exactly as the program does (`programs/elote_pool/src/math.rs`). Hand-written:
// not produced by Codama.

const BPS = 10_000n;
/** The year that annualizes `late_bps`: 365 days. */
export const SECONDS_PER_YEAR = 31_536_000n;
/** `CreditLine.version` of a line with an installment schedule. */
export const LINE_VERSION_SCHEDULED = 2;

type Bytes = ArrayLike<number>;

const view = (b: Bytes) =>
  new DataView(Uint8Array.from(b as ArrayLike<number>).buffer);

/** `Pool.reserved[0..4]`: the pool's grace period in seconds (u32 LE). */
export function poolGracePeriodSeconds(reserved: Bytes): bigint {
  return BigInt(view(reserved).getUint32(0, true));
}

export type Schedule = {
  intervalSeconds: bigint;
  installments: number;
  lateBps: number;
  maxMissed: number;
  flags: number;
  lateCollected: bigint;
  latePaidUntil: bigint;
};

/** `CreditLine.reserved` of a version-2 line; `null` for a single-maturity line. */
export function decodeSchedule(line: {
  version: number;
  reserved: Bytes;
}): Schedule | null {
  if (line.version !== LINE_VERSION_SCHEDULED) return null;
  const v = view(line.reserved);
  return {
    intervalSeconds: v.getBigInt64(0, true),
    installments: v.getUint16(8, true),
    lateBps: v.getUint16(10, true),
    maxMissed: v.getUint8(12),
    flags: v.getUint8(13),
    lateCollected: v.getBigUint64(14, true),
    latePaidUntil: v.getBigInt64(22, true),
  };
}

export type Arrears = {
  due: number;
  covered: number;
  missed: number;
  overdue: bigint;
  lateDue: bigint;
  firstMissedDue: bigint | null;
};

const min = (a: bigint, b: bigint) => (a < b ? a : b);
const max = (a: bigint, b: bigint) => (a > b ? a : b);

/** Arrears of a drawn scheduled line at `now` (unix seconds), as `math::arrears` computes them. */
export function lineArrears(
  line: {
    principal: bigint;
    interestDue: bigint;
    principalRepaid: bigint;
    interestRepaid: bigint;
    drawnAt: bigint;
  },
  s: Schedule,
  graceSeconds: bigint,
  now: bigint
): Arrears {
  const total = line.principal + line.interestDue;
  const paid = line.principalRepaid + line.interestRepaid;
  const n = BigInt(s.installments);
  const base = total / n;
  const cumulative = (k: bigint) => (k >= n ? total : base * k);
  const due =
    s.intervalSeconds <= 0n || now <= line.drawnAt
      ? 0n
      : min(n, (now - line.drawnAt) / s.intervalSeconds);
  const covered = paid >= total ? n : min(n - 1n, paid / base);
  const lateCap =
    s.lateBps === 0
      ? 0n
      : (BPS * SECONDS_PER_YEAR + BigInt(s.lateBps) - 1n) / BigInt(s.lateBps);
  let overdue = 0n;
  let weighted = 0n;
  let firstMissedDue: bigint | null = null;
  for (let k = covered + 1n; k <= due; k++) {
    const part = cumulative(k) - max(paid, cumulative(k - 1n));
    const dueAt = line.drawnAt + k * s.intervalSeconds;
    firstMissedDue ??= dueAt;
    const start = max(dueAt + graceSeconds, s.latePaidUntil);
    if (now > start) weighted += part * min(now - start, lateCap);
    overdue += part;
  }
  // Capped at the overdue amount for this settlement and at the principal over the line's life.
  const lateDue = min(
    min(overdue, (weighted * BigInt(s.lateBps)) / (BPS * SECONDS_PER_YEAR)),
    line.principal - s.lateCollected
  );
  return {
    due: Number(due),
    covered: Number(covered),
    missed: Number(due > covered ? due - covered : 0n),
    overdue,
    lateDue,
    firstMissedDue,
  };
}
