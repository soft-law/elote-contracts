// Replays `arrears-vectors.json`, produced by `programs/elote_pool/tests/vectors.rs` from the
// program's own arithmetic, against the TypeScript mirror. Run with `npm run test:client`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { lineArrears, lineSchedule, maxLinePrincipal } from "../src/schedule";

type Vector = {
  total: string;
  principal: string;
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
        principalRepaid: BigInt(v.paid),
        interestRepaid: 0n,
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
