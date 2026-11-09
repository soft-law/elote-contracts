// Replays `arrears-vectors.json`, produced by `programs/elote_pool/tests/vectors.rs` from the
// program's own arithmetic, against the TypeScript mirror. Run with `npm run test:client`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { bulletAccruedInterest, lineArrears, lineLateDue, lineSchedule, maxLinePrincipal, repayInterestLeft } from "../src/schedule";

type Vector = {
  total: string;
  principal: string;
  principal_left: string;
  anchor: string;
  interval: string;
  installments: number;
  late_bps: number;
  grace: number;
  late_paid_until: string;
  late_collected: string;
  accelerated_at: string;
  paid: string;
  now: string;
  due: number;
  covered: number;
  missed: number;
  overdue: string;
  overdue_principal: string;
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
        interestDue: BigInt(v.total) - BigInt(v.principal),
        // Only the sum and the principal left unpaid matter to the arrears.
        principalRepaid: BigInt(v.principal) - BigInt(v.principal_left),
        interestRepaid: BigInt(v.paid) - (BigInt(v.principal) - BigInt(v.principal_left)),
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
      BigInt(v.grace),
      BigInt(v.now)
    );
    assert.deepEqual(
      {
        due: a.due,
        covered: a.covered,
        missed: a.missed,
        overdue: a.overdue.toString(),
        overduePrincipal: a.overduePrincipal.toString(),
        lateDue: a.lateDue.toString(),
        firstMissedDue:
          a.firstMissedDue === null ? null : a.firstMissedDue.toString(),
      },
      {
        due: v.due,
        covered: v.covered,
        missed: v.missed,
        overdue: v.overdue,
        overduePrincipal: v.overdue_principal,
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
      interestDue: 0n,
      principalRepaid: 0n,
      interestRepaid: 0n,
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
    0n,
    1_000n + 600n + 50n * 31_536_000n
  );
  assert.equal(a.overdue, 600n);
  assert.equal(a.lateDue, 0n);
});

test("repayInterestLeft is the accrued interest on every active line", () => {
  const year = 31_536_000n;
  const t0 = 1_800_000_000n;
  // 6.0 drawn in full at t0 at 12% over a year: 720_000 fixed, 60_000 accrued a month in
  // (programs/elote_pool/tests/schedule.rs::a_settlement_in_two_payments_pays_what_one_payment_pays).
  const line = {
    status: 2,
    principal: 6_000_000n,
    drawn: 6_000_000n,
    availabilityEnd: t0 + year,
    interestDue: 720_000n,
    interestRepaid: 0n,
    drawTimeWeight: 6_000_000n * t0,
    rateBps: 1_200,
  };
  const now = t0 + year / 12n;
  assert.equal(repayInterestLeft(line, now), 60_000n);
  assert.equal(repayInterestLeft({ ...line, interestRepaid: 60_000n }, now), 0n);
  assert.equal(repayInterestLeft({ ...line, interestRepaid: 100_000n }, now), 0n, "never below what was repaid");
  assert.equal(repayInterestLeft(line, t0 + 2n * year), 720_000n, "never above the interest fixed");
  // While it can draw too (decision 47): the window does not enter. In collection, all the
  // interest still owed.
  const canDraw = { ...line, principal: 8_000_000n };
  const closed = { ...canDraw, availabilityEnd: now };
  assert.equal(repayInterestLeft(canDraw, now), 60_000n, "window open");
  assert.equal(repayInterestLeft(closed, now), 60_000n, "window closed");
  assert.equal(repayInterestLeft({ ...line, status: 5, interestRepaid: 20_000n }, now), 700_000n);
});

test("bulletAccruedInterest matches the program's unit case", () => {
  const year = 31_536_000n;
  // 4.0 drawn at t = 0 and 2.0 at half a year, at 12%; marked at one year.
  const weight = 2_000_000n * (year / 2n);
  assert.equal(bulletAccruedInterest(6_000_000n, weight, 1_200, year), 600_000n);
  assert.equal(bulletAccruedInterest(0n, 0n, 1_200, year), 0n);
  assert.throws(() => bulletAccruedInterest(2_000_000n, 2_000_000n * year, 1_200, 0n));
});

test("bulletAccruedInterest at the last due date can pass interestDue; the program keeps interestDue", () => {
  const year = 31_536_000n;
  const start = 1_800_000_000n;
  const lastDue = start + year;
  const tranches: [bigint, bigint][] = [
    [1_000_003n, start + 1n],
    [2_000_001n, start + year / 3n + 1_589n],
    [3_000_007n, start + (2n * year) / 3n + 398n],
  ];
  // What draw fixes per tranche, rounded down each time.
  const interestDue = tranches.reduce((s, [a, t]) => s + (a * 1_200n * (lastDue - t)) / (10_000n * year), 0n);
  const drawn = tranches.reduce((s, [a]) => s + a, 0n);
  const weight = tranches.reduce((s, [a, t]) => s + a * t, 0n);
  const accrued = bulletAccruedInterest(drawn, weight, 1_200, lastDue);
  assert.deepEqual([interestDue, accrued], [399_982n, 399_984n]);
  assert.equal(accrued < interestDue ? accrued : interestDue, interestDue);
});

test("lineLateDue: an accelerated bullet line owes late interest from the mark", () => {
  const day = 86_400n;
  const line = {
    intervalSeconds: 0n, installments: 0, lateBps: 3_650, maxMissed: 0, lateCollected: 0n, latePaidUntil: 0n,
    drawn: 4_000_000n, interestDue: 0n, principalRepaid: 0n, interestRepaid: 0n, firstDueAt: 0n,
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
    drawn: 4_000_000n, interestDue: 400_000n, principalRepaid: 0n, interestRepaid: 0n, firstDueAt: 0n,
    acceleratedAt: 0n, termEnd,
  };
  for (const now of [termEnd - 1n, termEnd]) assert.equal(lineLateDue(line, now), 0n, `at ${now}`);
  // No grace period: a day late is a day of 36.5% on the 4.0 of principal (the program's case).
  assert.equal(lineLateDue(line, termEnd + day), 4_000n);
  assert.equal(lineLateDue(line, termEnd + year), 1_460_000n);
  assert.equal(lineLateDue(line, termEnd + 10n * day), 40_000n);
});
