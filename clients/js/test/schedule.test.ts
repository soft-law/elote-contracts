// Replays `arrears-vectors.json`, produced by `programs/elote_pool/tests/vectors.rs` from the
// program's own arithmetic, against the TypeScript mirror. Run with `npm run test:client`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { interestAccrued, interestUnpaid, lineArrears, lineLateDue, lineOwed, lineSchedule, maxLinePrincipal } from "../src/schedule";

type Vector = {
  principal: string;
  anchor: string;
  interval: string;
  installments: number;
  late_bps: number;
  late_paid_until: string;
  late_collected: string;
  accelerated_at: string;
  paid: string;
  now: string;
  due: number;
  covered: number;
  missed: number;
  overdue: string;
  late_due: string;
  first_missed_due: string | null;
};

const vectors: Vector[] = JSON.parse(
  readFileSync(new URL("./arrears-vectors.json", import.meta.url), "utf8")
);

test("the vector file is not empty", () => {
  assert.ok(vectors.length > 100);
});

test("lineArrears agrees with math::arrears on every vector", () => {
  vectors.forEach((v, i) => {
    const a = lineArrears(
      {
        drawn: BigInt(v.principal),
        principalRepaid: BigInt(v.paid),
        // The vectors anchor the calendar one interval before the first due date.
        firstDueAt: BigInt(v.anchor) + BigInt(v.interval),
        acceleratedAt: BigInt(v.accelerated_at),
      },
      {
        intervalSeconds: BigInt(v.interval),
        installments: v.installments,
        lateBps: v.late_bps,
        maxMissed: 3,
        lateCollected: BigInt(v.late_collected),
        latePaidUntil: BigInt(v.late_paid_until),
      },
      BigInt(v.now)
    );
    assert.deepEqual(
      {
        due: a.due,
        covered: a.covered,
        missed: a.missed,
        overdue: a.overdue.toString(),
        lateDue: a.lateDue.toString(),
        firstMissedDue:
          a.firstMissedDue === null ? null : a.firstMissedDue.toString(),
      },
      {
        due: v.due,
        covered: v.covered,
        missed: v.missed,
        overdue: v.overdue,
        lateDue: v.late_due,
        firstMissedDue: v.first_missed_due,
      },
      `vector ${i}: ${JSON.stringify(v)}`
    );
  });
});

test("lineSchedule reads the schedule fields and returns null for a bullet line", () => {
  const line = {
    intervalSeconds: 2_592_000n,
    installments: 6,
    lateBps: 5_000,
    maxMissed: 3,
    lateCollected: 1_234n,
    latePaidUntil: 1_700_000_000n,
  };
  assert.deepEqual(lineSchedule(line), line);
  assert.equal(lineSchedule({ ...line, installments: 0 }), null);
});

test("maxLinePrincipal applies the cap to the NAV, rounded down", () => {
  assert.equal(maxLinePrincipal(10_000_000n, 1_000), 1_000_000n);
  assert.equal(
    maxLinePrincipal(10_000_001n, 1_000),
    1_000_000n,
    "rounded down like the program"
  );
  assert.equal(maxLinePrincipal(0n, 10_000), 0n);
});

test("lateDue is never negative when lateCollected exceeds the principal (R-24)", () => {
  // The program refuses this state with Overflow; the mirror must report 0, not a negative figure.
  const a = lineArrears(
    {
      drawn: 600n,
      principalRepaid: 0n,
      firstDueAt: 1_100n,
      acceleratedAt: 0n,
    },
    {
      intervalSeconds: 100n,
      installments: 6,
      lateBps: 10_000,
      maxMissed: 3,
      lateCollected: 601n,
      latePaidUntil: 0n,
    },
    1_000n + 600n + 50n * 31_536_000n
  );
  assert.equal(a.overdue, 600n);
  assert.equal(a.lateDue, 0n);
});

// The cases of `state.rs`'s unit tests (decision 48), on the line as the program stores it.
const MONTH = 31_536_000n / 12n;
const bullet = (principal: bigint) => ({
  drawn: 0n,
  principalRepaid: 0n,
  interestRepaid: 0n,
  rateBps: 1_200,
  principalSeconds: 0n,
  accruedAt: 0n,
  acceleratedAt: 0n,
  termEnd: 1_000_000n + 12n * MONTH,
  firstDueAt: 0n,
  intervalSeconds: 0n,
  installments: 0,
  principal,
});
/** `CreditLine::accrue` followed by a change of the outstanding principal. */
function accrueThen(l: ReturnType<typeof bullet>, now: bigint, change: Partial<ReturnType<typeof bullet>>) {
  const t = now < l.termEnd ? now : l.termEnd;
  const elapsed = t > l.accruedAt ? t - l.accruedAt : 0n;
  return {
    ...l,
    principalSeconds: l.principalSeconds + (l.drawn - l.principalRepaid) * elapsed,
    accruedAt: t > l.accruedAt ? t : l.accruedAt,
    ...change,
  };
}

test("interestAccrued: 600.00 a month on 60,000.00 at 12%, lower once principal is repaid, stopped at the end", () => {
  const t0 = 1_000_000n;
  let l = bullet(6_000_000n);
  assert.equal(interestAccrued(l, t0 + MONTH), 0n, "nothing drawn");
  l = accrueThen(l, t0, { drawn: 6_000_000n });
  assert.equal(interestAccrued(l, t0 + MONTH), 60_000n);
  l = accrueThen(l, t0 + MONTH, { principalRepaid: 1_000_000n });
  assert.equal(interestAccrued(l, t0 + MONTH), 60_000n, "the accrual is unchanged by accrue");
  assert.equal(interestAccrued(l, t0 + 2n * MONTH), 110_000n);
  l = { ...l, interestRepaid: 60_000n };
  assert.equal(interestUnpaid(l, t0 + 2n * MONTH), 50_000n);
  assert.equal(lineOwed(l, t0 + 2n * MONTH), 5_050_000n);
  assert.equal(interestAccrued(l, t0 + 12n * MONTH), 610_000n);
  assert.equal(interestAccrued(l, t0 + 40n * MONTH), 610_000n, "nothing after the term");
  // Accelerated after two months: nothing accrues after the mark.
  assert.equal(interestAccrued({ ...l, acceleratedAt: t0 + 2n * MONTH }, t0 + 9n * MONTH), 110_000n);
});

test("interestAccrued rounds once: 1.5 centavos a second is 5,400 in an hour however often it accrues", () => {
  const t0 = 1_000_000n;
  let l = accrueThen(bullet(394_200_000n), t0, { drawn: 394_200_000n });
  assert.equal(interestAccrued(l, t0 + 1n), 1n);
  const single = interestAccrued(l, t0 + 3_600n);
  for (let s = 1n; s <= 3_600n; s++) l = accrueThen(l, t0 + s, {});
  assert.deepEqual([single, interestAccrued(l, t0 + 3_600n)], [5_400n, 5_400n]);
});

test("interestAccrued on a scheduled line stops at the last due date", () => {
  const t0 = 1_000_000n;
  const line = {
    ...bullet(6_000_000n),
    drawn: 6_000_000n,
    accruedAt: t0,
    termEnd: t0 + 24n * MONTH,
    firstDueAt: t0 + MONTH,
    intervalSeconds: MONTH,
    installments: 6,
  };
  assert.equal(interestAccrued(line, t0 + 6n * MONTH), 360_000n);
  assert.equal(interestAccrued(line, t0 + 24n * MONTH), 360_000n, "not the term");
});

test("lineLateDue: an accelerated bullet line owes late interest from the mark", () => {
  const day = 86_400n;
  const line = {
    intervalSeconds: 0n, installments: 0, lateBps: 3_650, maxMissed: 0, lateCollected: 0n, latePaidUntil: 0n,
    drawn: 4_000_000n, principalRepaid: 0n, firstDueAt: 0n,
    acceleratedAt: 1_000n, termEnd: 1_000_000_000n,
  };
  // The program's integration case: 4.0 overdue for 10 days at 36.5% a year.
  assert.equal(lineLateDue(line, 1_000n + 10n * day), 40_000n);
  assert.equal(lineLateDue({ ...line, acceleratedAt: 0n }, 1_000n + 10n * day), 0n, "not accelerated, before the term");
});

test("lineLateDue: a bullet line past its term owes late interest from term_end, accelerated or not", () => {
  const day = 86_400n;
  const year = 365n * day;
  const termEnd = 1_000_000_000n;
  const line = {
    intervalSeconds: 0n, installments: 0, lateBps: 3_650, maxMissed: 0, lateCollected: 0n, latePaidUntil: 0n,
    drawn: 4_000_000n, principalRepaid: 0n, firstDueAt: 0n,
    acceleratedAt: 0n, termEnd,
  };
  for (const now of [termEnd - 1n, termEnd]) assert.equal(lineLateDue(line, now), 0n, `at ${now}`);
  // No grace period: a day late is a day of 36.5% on the 4.0 of principal (the program's case).
  assert.equal(lineLateDue(line, termEnd + day), 4_000n);
  assert.equal(lineLateDue(line, termEnd + year), 1_460_000n);
  assert.equal(lineLateDue(line, termEnd + 10n * day), 40_000n);
});
