// Computes the arrears of a scheduled line exactly as the program does
// (`programs/elote_pool/src/math.rs`). Hand-written: not produced by Codama.

const BPS = 10_000n;
/** The year that annualizes `late_bps`: 365 days. */
export const SECONDS_PER_YEAR = 31_536_000n;

/**
 * The largest line `approve_line` accepts at `nav` under `maxLineBps`, in USDC, rounded down as
 * `Pool::max_line_principal` does; `approve_line` converts it to MXN at the FIX in force
 * (`mxnForUsdc`). The app checks this before signing; `LineTooLarge` is the program's guard.
 */
export function maxLinePrincipal(nav: bigint, maxLineBps: number): bigint {
  return (nav * BigInt(maxLineBps)) / BPS;
}

export type Schedule = {
  intervalSeconds: bigint;
  installments: number;
  lateBps: number;
  maxMissed: number;
  lateCollected: bigint;
  latePaidUntil: bigint;
};

/** The schedule of a line, from its fields; `null` for a bullet line (`installments` is 0). */
/**
 * The ordinary interest a bullet line has accrued by `mark` (programs/elote_pool/src/math.rs
 * `bullet_accrued_interest`): `rateBps * (drawn * mark - drawTimeWeight) / (10_000 * SECONDS_PER_YEAR)`,
 * rounded down once. What `accelerate_line` keeps on a bullet line (never more than `interestDue`,
 * never less than what was repaid).
 */
export function bulletAccruedInterest(drawn: bigint, drawTimeWeight: bigint, rateBps: number, mark: bigint): bigint {
  const span = drawn * mark - drawTimeWeight;
  if (span < 0n) throw new RangeError("the mark is before the draws");
  return (span * BigInt(rateBps)) / (BPS * SECONDS_PER_YEAR);
}

export function lineSchedule(line: Schedule): Schedule | null {
  if (line.installments === 0) return null;
  const { intervalSeconds, installments, lateBps, maxMissed, lateCollected, latePaidUntil } = line;
  return { intervalSeconds, installments, lateBps, maxMissed, lateCollected, latePaidUntil };
}

/**
 * The late interest due on `line` at `now`, as `repay` charges it: a scheduled line's
 * (`lineArrears`), or a bullet line's, which the program treats as one installment due at
 * `termEnd`: late interest runs from `termEnd`, or from the mark if the line was accelerated
 * before. There is no grace period: late interest runs from the first second past a due date.
 */
export function lineLateDue(
  line: Schedule & {
    drawn: bigint;
    interestDue: bigint;
    principalRepaid: bigint;
    interestRepaid: bigint;
    firstDueAt: bigint;
    acceleratedAt: bigint;
    termEnd: bigint;
  },
  now: bigint
): bigint {
  const s = lineSchedule(line);
  if (s) return lineArrears(line, s, 0n, now).lateDue;
  // A bullet line is one installment due at the term: late interest from `termEnd`, or from the
  // mark if it was accelerated before (`line_arrears` in the program).
  const bullet: Schedule = { ...line, intervalSeconds: 1n, installments: 1, maxMissed: 0 };
  return lineArrears({ ...line, firstDueAt: line.termEnd }, bullet, 0n, now).lateDue;
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

/**
 * Arrears of a drawn scheduled line at `now` (unix seconds), as `math::arrears` computes them.
 * `graceSeconds` is `math::arrears`'s grace term, kept for the pinned vectors: the program passes
 * zero, so pass `0n` to mirror a line on chain.
 * The installments repay `drawn + interestDue` on the contract's calendar: installment `k` is
 * due at `firstDueAt + (k - 1) * interval` (decision 25), and late interest accrues on the
 * principal share (`drawn`) of what is overdue. Once `acceleratedAt` is set every installment is
 * due and late interest runs on all remaining principal from the mark.
 */
export function lineArrears(
  line: {
    drawn: bigint;
    interestDue: bigint;
    principalRepaid: bigint;
    interestRepaid: bigint;
    firstDueAt: bigint;
    /** `CreditLine.acceleratedAt`: zero until `mark_in_collection` accelerates the balance. */
    acceleratedAt: bigint;
  },
  s: Schedule,
  graceSeconds: bigint,
  now: bigint
): Arrears {
  const total = line.drawn + line.interestDue;
  const paid = line.principalRepaid + line.interestRepaid;
  const n = BigInt(s.installments);
  const base = total / n;
  const cumulative = (k: bigint) => (k >= n ? total : base * k);
  // One interval before the first due date: the anchor `math::arrears` counts from.
  const anchor = line.firstDueAt - s.intervalSeconds;
  const accelerated = line.acceleratedAt > 0n;
  const due = accelerated
    ? n
    : s.intervalSeconds <= 0n || now <= anchor
      ? 0n
      : min(n, (now - anchor) / s.intervalSeconds);
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
    const partPrincipal = (part * line.drawn) / total;
    const dueAt = anchor + k * s.intervalSeconds;
    // Once accelerated, an installment not yet due is exigible at the mark and accrues from it.
    const exigibleAt = accelerated ? min(dueAt, line.acceleratedAt) : dueAt;
    const accrualFrom = accelerated ? min(dueAt + graceSeconds, line.acceleratedAt) : dueAt + graceSeconds;
    firstMissedDue ??= exigibleAt;
    const start = max(accrualFrom, s.latePaidUntil);
    if (now > start) weighted += partPrincipal * min(now - start, lateCap);
    overdue += part;
    overduePrincipal += partPrincipal;
  }
  // Capped at the overdue principal for this settlement and at the principal over the line's
  // life. The program refuses `lateCollected > principal` with Overflow; the mirror clamps the
  // lifetime room at zero rather than reporting a negative figure.
  const lifetimeLeft = max(0n, line.drawn - s.lateCollected);
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
