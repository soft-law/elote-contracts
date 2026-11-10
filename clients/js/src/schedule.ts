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

/** The fields of a `CreditLine` its ordinary interest is read from (decision 48). */
export type AccrualLine = {
  drawn: bigint;
  principalRepaid: bigint;
  interestRepaid: bigint;
  rateBps: number;
  principalSeconds: bigint;
  accruedAt: bigint;
  acceleratedAt: bigint;
  termEnd: bigint;
  firstDueAt: bigint;
  intervalSeconds: bigint;
  installments: number;
};

/** `CreditLine::last_due`: the last installment's due date, or `termEnd` on a bullet line. */
export function lineLastDue(line: Pick<AccrualLine, "termEnd" | "firstDueAt" | "intervalSeconds" | "installments">): bigint {
  if (line.installments === 0) return line.termEnd;
  return line.firstDueAt + BigInt(line.installments - 1) * line.intervalSeconds;
}

/**
 * The ordinary interest a line has accrued by `now`, repaid or not (`CreditLine::interest_accrued`,
 * decision 48): `rateBps * (principalSeconds + (drawn - principalRepaid) * (min(now, end) -
 * accruedAt)) / (10_000 * SECONDS_PER_YEAR)`, rounded down once, where `end` is the last due date,
 * or the acceleration when it came first.
 */
export function interestAccrued(line: AccrualLine, now: bigint): bigint {
  const last = lineLastDue(line);
  const end = line.acceleratedAt > 0n ? min(last, line.acceleratedAt) : last;
  const t = min(now, end);
  const elapsed = t > line.accruedAt ? t - line.accruedAt : 0n;
  const seconds = line.principalSeconds + (line.drawn - line.principalRepaid) * elapsed;
  return (seconds * BigInt(line.rateBps)) / (BPS * SECONDS_PER_YEAR);
}

/**
 * The ordinary interest a `repay` at `now` takes after the late interest and before principal: the
 * interest accrued and not yet settled (`CreditLine::interest_unpaid`). The program reads its own
 * clock: quote `now` a little ahead, since what the quote exceeds the accrual by falls on
 * principal.
 */
export function interestUnpaid(line: AccrualLine, now: bigint): bigint {
  return interestAccrued(line, now) - line.interestRepaid;
}

/** What the line owes at `now`, late interest aside: principal and interest unpaid (`CreditLine::owed`). */
export function lineOwed(line: AccrualLine, now: bigint): bigint {
  return line.drawn - line.principalRepaid + interestUnpaid(line, now);
}

/** The schedule of a line, from its fields; `null` for a bullet line (`installments` is 0). */
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
    principalRepaid: bigint;
    firstDueAt: bigint;
    acceleratedAt: bigint;
    termEnd: bigint;
  },
  now: bigint
): bigint {
  const s = lineSchedule(line);
  if (s) return lineArrears(line, s, now).lateDue;
  // A bullet line is one installment due at the term: late interest from `termEnd`, or from the
  // mark if it was accelerated before (`line_arrears` in the program).
  const bullet: Schedule = { ...line, intervalSeconds: 1n, installments: 1, maxMissed: 0 };
  return lineArrears({ ...line, firstDueAt: line.termEnd }, bullet, now).lateDue;
}

export type Arrears = {
  due: number;
  covered: number;
  missed: number;
  /** Principal fallen due and not repaid: what late interest accrues on (decision 48). */
  overdue: bigint;
  lateDue: bigint;
  firstMissedDue: bigint | null;
};

const min = (a: bigint, b: bigint) => (a < b ? a : b);
const max = (a: bigint, b: bigint) => (a > b ? a : b);

/**
 * Arrears of a drawn scheduled line at `now` (unix seconds), as `math::arrears` computes them.
 * There is no grace period: late interest runs from the first second past a due date.
 * The installments repay the principal drawn on the contract's calendar (decision 48): installment
 * `k` is due at `firstDueAt + (k - 1) * interval` and met once `principalRepaid` reaches its
 * cumulative share; late interest accrues on the principal overdue. Once `acceleratedAt` is set
 * every installment is due and late interest runs on all remaining principal from the mark.
 */
export function lineArrears(
  line: {
    drawn: bigint;
    principalRepaid: bigint;
    firstDueAt: bigint;
    /** `CreditLine.acceleratedAt`: zero until `mark_in_collection` accelerates the balance. */
    acceleratedAt: bigint;
  },
  s: Schedule,
  now: bigint
): Arrears {
  const principal = line.drawn;
  const paid = line.principalRepaid;
  const n = BigInt(s.installments);
  const base = principal / n;
  const cumulative = (k: bigint) => (k >= n ? principal : base * k);
  // One interval before the first due date: the anchor `math::arrears` counts from.
  const anchor = line.firstDueAt - s.intervalSeconds;
  const accelerated = line.acceleratedAt > 0n;
  const due = accelerated
    ? n
    : s.intervalSeconds <= 0n || now <= anchor
      ? 0n
      : min(n, (now - anchor) / s.intervalSeconds);
  const covered = paid >= principal ? n : min(n - 1n, paid / base);
  const lateCap =
    s.lateBps === 0
      ? 0n
      : (BPS * SECONDS_PER_YEAR + BigInt(s.lateBps) - 1n) / BigInt(s.lateBps);
  let overdue = 0n;
  let weighted = 0n;
  let firstMissedDue: bigint | null = null;
  for (let k = covered + 1n; k <= due; k++) {
    const part = cumulative(k) - max(paid, cumulative(k - 1n));
    const dueAt = anchor + k * s.intervalSeconds;
    // Once accelerated, an installment not yet due is exigible at the mark and accrues from it.
    const exigibleAt = accelerated ? min(dueAt, line.acceleratedAt) : dueAt;
    firstMissedDue ??= exigibleAt;
    const start = max(exigibleAt, s.latePaidUntil);
    if (now > start) weighted += part * min(now - start, lateCap);
    overdue += part;
  }
  // Capped at the overdue principal for this settlement and at the principal over the line's
  // life. The program refuses `lateCollected > principal` with Overflow; the mirror clamps the
  // lifetime room at zero rather than reporting a negative figure.
  const lifetimeLeft = max(0n, principal - s.lateCollected);
  const lateDue = min(min(overdue, (weighted * BigInt(s.lateBps)) / (BPS * SECONDS_PER_YEAR)), lifetimeLeft);
  return {
    due: Number(due),
    covered: Number(covered),
    missed: Number(due > covered ? due - covered : 0n),
    overdue,
    lateDue,
    firstMissedDue,
  };
}
