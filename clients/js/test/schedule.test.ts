// Replays `arrears-vectors.json`, produced by `programs/elote_pool/tests/vectors.rs` from the
// program's own arithmetic, against the TypeScript mirror. Run with `npm run test:client`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  LINE_VERSION_SCHEDULED,
  decodeSchedule,
  lineArrears,
  maxLinePrincipal,
  poolFlags,
  poolGracePeriodSeconds,
  poolMaxLineBps,
} from "../src/schedule";

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
        principal: BigInt(v.principal),
        interestDue: BigInt(v.total) - BigInt(v.principal),
        principalRepaid: BigInt(v.paid),
        interestRepaid: 0n,
        drawnAt: BigInt(v.anchor),
      },
      {
        intervalSeconds: BigInt(v.interval),
        installments: v.installments,
        lateBps: v.late_bps,
        maxMissed: 3,
        flags: 0,
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

test("decodeSchedule reads the packed CreditLine.reserved layout", () => {
  const reserved = new Uint8Array(32);
  const dv = new DataView(reserved.buffer);
  dv.setBigInt64(0, 2_592_000n, true); // interval_seconds
  dv.setUint16(8, 6, true); // installments
  dv.setUint16(10, 5_000, true); // late_bps
  dv.setUint8(12, 3); // max_missed
  dv.setUint8(13, 0); // flags
  dv.setBigUint64(14, 1_234n, true); // late_collected
  dv.setBigInt64(22, 1_700_000_000n, true); // late_paid_until
  assert.deepEqual(
    decodeSchedule({ version: LINE_VERSION_SCHEDULED, reserved }),
    {
      intervalSeconds: 2_592_000n,
      installments: 6,
      lateBps: 5_000,
      maxMissed: 3,
      flags: 0,
      lateCollected: 1_234n,
      latePaidUntil: 1_700_000_000n,
    }
  );
  assert.equal(decodeSchedule({ version: 1, reserved }), null);
});

test("poolGracePeriodSeconds reads Pool.reserved[0..4]", () => {
  const reserved = new Uint8Array(32);
  new DataView(reserved.buffer).setUint32(0, 86_400, true);
  assert.equal(poolGracePeriodSeconds(reserved), 86_400n);
});

test("poolFlags reads the permissioned and asset-control bytes of Pool.reserved", () => {
  const reserved = new Uint8Array(32);
  assert.deepEqual(poolFlags(reserved), {
    permissioned: false,
    assetFreezable: false,
    assetPausable: false,
  });
  reserved[6] = 1;
  reserved[7] = 0b11;
  assert.deepEqual(poolFlags(reserved), {
    permissioned: true,
    assetFreezable: true,
    assetPausable: true,
  });
  reserved[7] = 0b10;
  assert.equal(poolFlags(reserved).assetFreezable, false);
  assert.equal(poolFlags(reserved).assetPausable, true);
});

test("poolMaxLineBps reads Pool.reserved[4..6] and maxLinePrincipal applies it to the NAV", () => {
  const reserved = new Uint8Array(32);
  const dv = new DataView(reserved.buffer);
  dv.setUint32(0, 86_400, true);
  dv.setUint16(4, 1_000, true);
  assert.equal(poolMaxLineBps(reserved), 1_000);
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
      principal: 600n,
      interestDue: 0n,
      principalRepaid: 0n,
      interestRepaid: 0n,
      drawnAt: 1_000n,
    },
    {
      intervalSeconds: 100n,
      installments: 6,
      lateBps: 10_000,
      maxMissed: 3,
      flags: 0,
      lateCollected: 601n,
      latePaidUntil: 0n,
    },
    0n,
    1_000n + 600n + 50n * 31_536_000n
  );
  assert.equal(a.overdue, 600n);
  assert.equal(a.lateDue, 0n);
});
