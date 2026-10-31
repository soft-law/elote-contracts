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

export type PoolFlags = {
  /** `reserved[6]`: shares are frozen by default (Token ACL); holders need a thaw. */
  permissioned: boolean;
  /** `reserved[7]` bit 0: the asset's issuer can freeze the vault. */
  assetFreezable: boolean;
  /** `reserved[7]` bit 1: the asset carries the Token-2022 `Pausable` extension. */
  assetPausable: boolean;
};

/** The read-only facts `init_pool` recorded about the two mints (`Pool.reserved[6..8]`). */
export function poolFlags(reserved: Bytes): PoolFlags {
  const v = view(reserved);
  const controls = v.getUint8(7);
  return {
    permissioned: v.getUint8(6) === 1,
    assetFreezable: (controls & 0b01) !== 0,
    assetPausable: (controls & 0b10) !== 0,
  };
}

/** `Pool.reserved[4..6]`: the largest line as a share of the NAV, in basis points (u16 LE). */
export function poolMaxLineBps(reserved: Bytes): number {
  return view(reserved).getUint16(4, true);
}

/**
 * The largest principal `open_line` accepts at `nav` under `maxLineBps`, rounded down as
 * `Pool::max_line_principal` does. The app checks this before signing; `LineTooLarge` is the
 * program's backstop.
 */
export function maxLinePrincipal(nav: bigint, maxLineBps: number): bigint {
  return (nav * BigInt(maxLineBps)) / BPS;
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
  /** Principal and interest fallen due and not paid. */
  overdue: bigint;
  /** The principal share of `overdue`: what late interest accrues on (decision 11). */
  overduePrincipal: bigint;
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
  let overduePrincipal = 0n;
  let weighted = 0n;
  let firstMissedDue: bigint | null = null;
  for (let k = covered + 1n; k <= due; k++) {
    const part = cumulative(k) - max(paid, cumulative(k - 1n));
    // Late interest accrues on the principal share of each part only (decision 11).
    const partPrincipal = (part * line.principal) / total;
    const dueAt = line.drawnAt + k * s.intervalSeconds;
    firstMissedDue ??= dueAt;
    const start = max(dueAt + graceSeconds, s.latePaidUntil);
    if (now > start) weighted += partPrincipal * min(now - start, lateCap);
    overdue += part;
    overduePrincipal += partPrincipal;
  }
  // Capped at the overdue principal for this settlement and at the principal over the line's
  // life. The program refuses `lateCollected > principal` with Overflow; the mirror clamps the
  // lifetime room at zero rather than reporting a negative figure.
  const lifetimeLeft = max(0n, line.principal - s.lateCollected);
  const lateDue = min(
    min(
      overduePrincipal,
      (weighted * BigInt(s.lateBps)) / (BPS * SECONDS_PER_YEAR)
    ),
    lifetimeLeft
  );
  return {
    due: Number(due),
    covered: Number(covered),
    missed: Number(due > covered ? due - covered : 0n),
    overdue,
    overduePrincipal,
    lateDue,
    firstMissedDue,
  };
}
