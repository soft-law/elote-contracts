//! Test vectors for the client's schedule mirror.
//!
//! `clients/js/src/schedule.ts` reimplements `math::arrears` in TypeScript so the app and a
//! keeper can show arrears without a simulation. This test is the contract between the two: it
//! evaluates the program's arithmetic on a fixed set of cases (the unit-test cases of `math.rs`
//! plus a deterministic pseudo-random sweep) and compares the result with the committed file
//! `clients/js/test/arrears-vectors.json`, which `clients/js/test/schedule.test.ts` replays
//! against the TypeScript. Regenerate the file with `UPDATE_VECTORS=1 cargo test --test vectors`
//! after a deliberate change to the arithmetic, in the same commit.

use elote_pool::math::{arrears, ScheduleTerms};
use serde_json::{json, Value};

/// A small deterministic generator, so the sweep is the same on every machine.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

fn case(t: &ScheduleTerms, paid: u64, now: i64) -> Value {
    let a = arrears(t, paid, now).expect("every vector is a valid line");
    json!({
        "total": t.total.to_string(),
        "principal": t.principal.to_string(),
        "drawn_at": t.drawn_at.to_string(),
        "interval": t.interval.to_string(),
        "installments": t.installments,
        "late_bps": t.late_bps,
        "grace": t.grace,
        "late_paid_until": t.late_paid_until.to_string(),
        "late_collected": t.late_collected.to_string(),
        "paid": paid.to_string(),
        "now": now.to_string(),
        "due": a.due,
        "covered": a.covered,
        "missed": a.missed,
        "overdue": a.overdue.to_string(),
        "late_due": a.late_due.to_string(),
        "first_missed_due": a.first_missed_due.map(|v| v.to_string()),
    })
}

/// The cases of the `math.rs` unit tests, so a reader can match them by eye.
fn hand_cases() -> Vec<Value> {
    const YEAR: u64 = elote_pool::state::SECONDS_PER_YEAR;
    let terms = |total: u64, n: u16, late_bps: u16, grace: u32| ScheduleTerms {
        total,
        drawn_at: 1_000,
        interval: 100,
        installments: n,
        late_bps,
        grace,
        late_paid_until: 0,
        principal: total,
        late_collected: 0,
    };
    let mut v = Vec::new();
    let t = terms(600, 6, 0, 0);
    for (paid, now) in [(0, 1_099), (0, 1_100), (150, 1_300), (400, 1_300), (0, 9_999)] {
        v.push(case(&t, paid, now));
    }
    // Two installments of 365 overdue at 100% a year, grace 10: late interest 438.
    let mut t = terms(365 * 3 * YEAR / 1_000, 3, 10_000, 10);
    t.interval = 1_000;
    let now = t.drawn_at + 2_000 + 10 + 100;
    v.push(case(&t, 0, now));
    v.push(case(&t, 0, t.drawn_at + 1_000 + 10));
    t.late_paid_until = now;
    v.push(case(&t, 0, now));
    v.push(case(&t, 0, now + 10));
    // Capped at the overdue amount, then at what is left of the principal.
    let t = terms(600, 6, 10_000, 0);
    v.push(case(&t, 0, 1_000 + 600 + 50 * YEAR as i64));
    let mut t = terms(600, 6, 10_000, 0);
    t.principal = 500;
    t.late_collected = 450;
    v.push(case(&t, 0, 1_000 + 600 + 50 * YEAR as i64));
    t.late_collected = 500;
    v.push(case(&t, 0, 1_000 + 600 + 50 * YEAR as i64));
    // A partial payment lowers the overdue part.
    v.push(case(&terms(600, 6, 0, 0), 60, 1_200));
    v
}

fn generated_cases() -> Vec<Value> {
    let mut r = Lcg(0x5EED_2026_1019);
    (0..400)
        .map(|_| {
            let n = r.range(1, 120) as u16;
            let total = r.range(u64::from(n), 1 << 48);
            let interest = total / 10_000 * r.range(0, 10_000);
            let principal = total - interest;
            let late_collected = if r.range(0, 3) == 0 { principal / 10_000 * r.range(0, 10_000) } else { 0 };
            let interval = r.range(1, 10_000_000) as i64;
            let drawn_at = r.range(1_000_000_000, 2_000_000_000) as i64;
            let grace = r.range(0, 2_000_000) as u32;
            let late_bps = r.range(0, 10_000) as u16;
            let paid = (total / 10_000).max(1) * r.range(0, 12_000);
            let now = drawn_at + interval * (r.range(0, 230) as i64 - 100);
            let late_paid_until = if r.range(0, 1) == 0 {
                0
            } else {
                drawn_at + (now - drawn_at) * (r.range(0, 10_000) as i64) / 10_000
            };
            let t = ScheduleTerms {
                total,
                drawn_at,
                interval,
                installments: n,
                late_bps,
                grace,
                late_paid_until,
                principal,
                late_collected,
            };
            case(&t, paid, now)
        })
        .collect()
}

fn vectors_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../clients/js/test/arrears-vectors.json")
}

#[test]
fn committed_vectors_match_the_program_arithmetic() {
    let mut all = hand_cases();
    all.extend(generated_cases());
    let expected = serde_json::to_string_pretty(&Value::Array(all)).unwrap() + "\n";
    let path = vectors_path();
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::write(&path, &expected).expect("write vectors");
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}\nrun `UPDATE_VECTORS=1 cargo test --test vectors`", path.display()));
    assert!(
        committed == expected,
        "{} is out of date: run `UPDATE_VECTORS=1 cargo test --test vectors` and commit the file",
        path.display()
    );
}
