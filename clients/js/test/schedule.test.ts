// Replays `arrears-vectors.json`, produced by `programs/elote_pool/tests/vectors.rs` from the
// program's own arithmetic, against the TypeScript mirror. Run with `npm run test:client`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  LINE_VERSION_SCHEDULED,
  decodeSchedule,
  lineArrears,
  poolGracePeriodSeconds,
} from "../src/schedule";

type Vector = {
  total: string;
  principal: string;
  drawn_at: string;
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
        drawnAt: BigInt(v.drawn_at),
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
        lateDue: a.lateDue.toString(),
        firstMissedDue: a.firstMissedDue === null ? null : a.firstMissedDue.toString(),
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
  assert.deepEqual(decodeSchedule({ version: LINE_VERSION_SCHEDULED, reserved }), {
    intervalSeconds: 2_592_000n,
    installments: 6,
    lateBps: 5_000,
    maxMissed: 3,
    flags: 0,
    lateCollected: 1_234n,
    latePaidUntil: 1_700_000_000n,
  });
  assert.equal(decodeSchedule({ version: 1, reserved }), null);
});

test("poolGracePeriodSeconds reads Pool.reserved[0..4]", () => {
  const reserved = new Uint8Array(32);
  new DataView(reserved.buffer).setUint32(0, 86_400, true);
  assert.equal(poolGracePeriodSeconds(reserved), 86_400n);
});
