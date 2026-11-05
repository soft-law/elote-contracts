//! The FIX: Banxico's USD/MXN rate, published on chain by the pool's `fx_feeder` (or the council)
//! as `Pool::fix_rate` (MXN per USD times `FIX_SCALE`) and `Pool::fix_date` (the date Banxico
//! determined it, `yyyymmdd`). Lines and pagarés are in centavos of MXN; the vault holds USDC.
//!
//! The feeder's FIX is also checked against a market reference, RedStone's MXNe price on Solana
//! (`REFERENCE_ACCOUNT`); the reference never sets the FIX, it only decides whether the feeder may
//! publish without the council.

use anchor_lang::prelude::*;

use crate::errors::PoolError;

/// `fix_rate` is the FIX times 10^4, Banxico's four decimals: 18.3740 MXN per USD is 183_740.
pub const FIX_SCALE: u64 = 10_000;
/// centavos per peso times USDC base units per dollar, over `FIX_SCALE`: `usdc = mxn * 10^8 / fix`.
const MXN_USDC: u128 = 100_000_000;
/// A FIX older than this many days (Mexico City dates) is stale for a draw and an approval, and the
/// council publishes none older.
pub const MAX_FIX_AGE_DAYS: i64 = 5;
/// The feeder's band: the largest move from the FIX in force it may publish, in basis points per
/// day between their dates; a larger one needs the council.
pub const MAX_FIX_JUMP_BPS_PER_DAY: u64 = 500;
/// The days the band counts at most: 15% however long the FIX in force has been.
pub const MAX_FIX_JUMP_DAYS: i64 = 3;
/// RedStone's Solana price adapter (`redstone-solana-price-adapter`, the `solanaMultiFeed`
/// deployment), which verifies the signed RedStone payload when it writes a price.
pub const REFERENCE_PROGRAM: Pubkey = pubkey!("REDSTBDUecGjwXd6YGPzHSvEUBHQqVRfCcjUVgPiHsr");
/// The adapter's MXNe price account: the PDA `["price", "MXNe"]`, each seed zero-padded to 32
/// bytes. Mainnet only; it exists on no other cluster.
pub const REFERENCE_ACCOUNT: Pubkey = pubkey!("8QuwResPgdQdja4gdTbvNxeUznc58XX8baUM3cCwfVW2");
/// Anchor's discriminator of the adapter's `PriceData`: `sha256("account:PriceData")[..8]`.
const REFERENCE_DISCRIMINATOR: [u8; 8] = [0xe8, 0x71, 0xc1, 0xe7, 0x85, 0xd1, 0xce, 0x9a];
/// The feed id the account must carry, zero-padded to 32 bytes.
const REFERENCE_FEED_ID: &[u8] = b"MXNe";
/// The adapter writes every value with eight decimals.
const REFERENCE_DECIMALS: u8 = 8;
/// `10^REFERENCE_DECIMALS * FIX_SCALE`: `rate = REFERENCE_SCALE / value`.
const REFERENCE_SCALE: u128 = 1_000_000_000_000;
/// The oldest reference the feeder publishes against: the feed's 24-hour heartbeat and two hours.
pub const MAX_REFERENCE_AGE_SECONDS: i64 = 26 * 3_600;
/// The largest gap between the feeder's FIX and the reference, in basis points of the reference.
pub const MAX_FIX_REFERENCE_GAP_BPS: u64 = 200;
/// Mexico City is UTC-6 all year since 2022.
const MEXICO_CITY_OFFSET_SECONDS: i64 = -6 * 3_600;

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The day number of a `yyyymmdd` date; `FixDateInvalid` for a date that does not exist or falls
/// outside 2000–2199.
pub fn fix_day(fix_date: u32) -> Result<i64> {
    let (y, m, d) = (i64::from(fix_date / 10_000), i64::from(fix_date / 100 % 100), i64::from(fix_date % 100));
    require!((2000..2200).contains(&y) && (1..=12).contains(&m) && d >= 1, PoolError::FixDateInvalid);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in_month = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    require!(d <= days_in_month, PoolError::FixDateInvalid);
    Ok(days_from_civil(y, m, d))
}

/// Today's day number in Mexico City.
pub fn mexico_city_day(now: i64) -> i64 {
    (now + MEXICO_CITY_OFFSET_SECONDS).div_euclid(86_400)
}

/// USDC base units delivered for `mxn` centavos at `fix_rate`, rounded down.
pub fn usdc_for_mxn(mxn: u64, fix_rate: u64) -> Option<u64> {
    (u128::from(mxn).checked_mul(MXN_USDC)? / u128::from(fix_rate).max(1)).try_into().ok()
}

/// Centavos credited for `usdc` base units at `fix_rate`, rounded down.
pub fn mxn_for_usdc(usdc: u64, fix_rate: u64) -> Option<u64> {
    (u128::from(usdc).checked_mul(u128::from(fix_rate))? / MXN_USDC).try_into().ok()
}

/// USDC base units that pay `mxn` centavos at `fix_rate`, rounded up: what a repayment takes, so the
/// pool never credits more MXN than it received.
pub fn usdc_ceil_for_mxn(mxn: u64, fix_rate: u64) -> Option<u64> {
    let fix = u128::from(fix_rate).max(1);
    u128::from(mxn).checked_mul(MXN_USDC)?.div_ceil(fix).try_into().ok()
}

/// The cost of `applied` centavos of a pagaré of `amount_mxn` that delivered `usdc_delivered`,
/// rounded down; the whole pagaré costs exactly `usdc_delivered`.
pub fn cost_of(usdc_delivered: u64, amount_mxn: u64, applied: u64) -> Option<u64> {
    if amount_mxn == 0 {
        return Some(0);
    }
    (u128::from(usdc_delivered).checked_mul(u128::from(applied.min(amount_mxn)))? / u128::from(amount_mxn)).try_into().ok()
}

/// Whether `new` is within the feeder's band of `old` for `days` between their dates:
/// `MAX_FIX_JUMP_BPS_PER_DAY` a day, counting at least one day and at most `MAX_FIX_JUMP_DAYS`.
pub fn within_band(old: u64, new: u64, days: i64) -> bool {
    let days = days.clamp(1, MAX_FIX_JUMP_DAYS) as u128;
    let diff = u128::from(old.abs_diff(new));
    diff * 10_000 <= u128::from(old) * u128::from(MAX_FIX_JUMP_BPS_PER_DAY) * days
}

/// The reference in FIX terms, MXN per USD times `FIX_SCALE` rounded down, and its timestamp in
/// seconds, from the bytes of the adapter's `PriceData` account: the discriminator (0..8), the feed
/// id (8..40), the value as a big-endian u256 of USD per MXNe times 10^8 (40..72), the timestamp in
/// milliseconds, little-endian (72..80), `write_timestamp: Option<u64>` (from 80), the write slot,
/// and the decimals. `FxReferenceInvalid` for any other feed, layout or decimals, or for a value
/// that is zero or does not convert to a positive rate.
pub fn reference_rate(data: &[u8]) -> Result<(u64, i64)> {
    require!(data.len() >= 98 && data[..8] == REFERENCE_DISCRIMINATOR, PoolError::FxReferenceInvalid);
    require!(
        data[8..12] == *REFERENCE_FEED_ID && data[12..40].iter().all(|b| *b == 0),
        PoolError::FxReferenceInvalid
    );
    // The decimals follow the Option's tag (and its eight bytes when Some) and the write slot.
    let decimals_at = match data[80] {
        0 => 89,
        1 => 97,
        _ => return err!(PoolError::FxReferenceInvalid),
    };
    require!(data[decimals_at] == REFERENCE_DECIMALS, PoolError::FxReferenceInvalid);
    require!(data[40..64].iter().all(|b| *b == 0), PoolError::FxReferenceInvalid);
    let mut value = [0u8; 8];
    value.copy_from_slice(&data[64..72]);
    let value = u64::from_be_bytes(value);
    require!(value > 0, PoolError::FxReferenceInvalid);
    let rate = u64::try_from(REFERENCE_SCALE / u128::from(value)).map_err(|_| PoolError::FxReferenceInvalid)?;
    require!(rate > 0, PoolError::FxReferenceInvalid);
    let mut ms = [0u8; 8];
    ms.copy_from_slice(&data[72..80]);
    let at = i64::try_from(u64::from_le_bytes(ms) / 1_000).map_err(|_| PoolError::FxReferenceInvalid)?;
    Ok((rate, at))
}

/// Whether `value` is within `bps` basis points of `reference`.
pub fn within_bps(reference: u64, value: u64, bps: u64) -> bool {
    u128::from(reference.abs_diff(value)) * 10_000 <= u128::from(reference) * u128::from(bps)
}

/// Whether `min ..= max` is a valid pair of FIX bounds: both zero (no bounds), or `min <= max` with
/// `max` positive.
pub fn bounds_valid(min: u64, max: u64) -> bool {
    (min == 0 && max == 0) || (max > 0 && min <= max)
}

/// Whether `rate` is within the bounds `min ..= max`; zero and zero bound nothing.
pub fn within_bounds(rate: u64, min: u64, max: u64) -> bool {
    (min == 0 && max == 0) || (min..=max).contains(&rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_count_days_and_refuse_impossible_ones() {
        assert_eq!(fix_day(19700101 + 300_000_000).ok(), None, "year 2270 is out of range");
        assert_eq!(fix_day(20000101).unwrap(), 10_957);
        assert_eq!(fix_day(20261103).unwrap() - fix_day(20261031).unwrap(), 3);
        assert_eq!(fix_day(20240301).unwrap() - fix_day(20240228).unwrap(), 2, "2024 is a leap year");
        assert_eq!(fix_day(21000301).unwrap() - fix_day(21000228).unwrap(), 1, "2100 is not");
        for bad in [20261301, 20261100, 20261131, 20250229, 19991231, 0] {
            assert!(fix_day(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mexico_city_dates_run_six_hours_behind_utc() {
        let noon_utc_nov_3 = 1_793_707_200; // 2026-11-03T12:00:00Z
        assert_eq!(mexico_city_day(noon_utc_nov_3), fix_day(20261103).unwrap());
        assert_eq!(mexico_city_day(noon_utc_nov_3 - 12 * 3_600 + 5 * 3_600), fix_day(20261102).unwrap(), "05:00Z is still Nov 2 in CDMX");
    }

    #[test]
    fn conversions_round_down_both_ways() {
        // MXN 50,000.00 at 18.3740: 2,721.23 USDC (2_721_236_529.88 base units, floored).
        assert_eq!(usdc_for_mxn(5_000_000, 183_740), Some(2_721_236_529));
        // 100 USDC at 18.3740: MXN 1,837.40.
        assert_eq!(mxn_for_usdc(100_000_000, 183_740), Some(183_740));
        assert_eq!(mxn_for_usdc(1, 183_740), Some(0), "dust credits nothing");
        // A round trip never credits more than was owed.
        let usdc = usdc_for_mxn(123_456, 177_777).unwrap();
        assert!(mxn_for_usdc(usdc, 177_777).unwrap() <= 123_456);
        assert_eq!(usdc_for_mxn(u64::MAX, 1), None);
    }

    #[test]
    fn a_repayment_takes_usdc_rounded_up_and_costs_add_up_to_the_delivery() {
        assert_eq!(usdc_ceil_for_mxn(183_740, 183_740), Some(100_000_000));
        assert_eq!(usdc_ceil_for_mxn(1, 183_740), Some(545), "one centavo costs 544.24 base units, rounded up");
        let delivered = usdc_for_mxn(5_000_000, 183_740).unwrap();
        let half = cost_of(delivered, 5_000_000, 2_500_000).unwrap();
        assert_eq!(half + (cost_of(delivered, 5_000_000, 5_000_000).unwrap() - half), delivered);
        assert_eq!(cost_of(delivered, 5_000_000, 9_999_999), Some(delivered), "capped at the pagaré");
    }

    #[test]
    fn the_band_is_five_percent_a_day_either_way_for_one_to_three_days() {
        for days in [-1, 0, 1] {
            assert!(within_band(200_000, 210_000, days), "at least one day");
            assert!(within_band(200_000, 190_000, days));
            assert!(!within_band(200_000, 210_001, days));
            assert!(!within_band(200_000, 189_999, days));
        }
        assert!(within_band(200_000, 220_000, 2));
        assert!(!within_band(200_000, 220_001, 2));
        for days in [3, 10, i64::MAX] {
            assert!(within_band(200_000, 230_000, days), "at most three days");
            assert!(within_band(200_000, 170_000, days));
            assert!(!within_band(200_000, 230_001, days));
            assert!(!within_band(200_000, 169_999, days));
        }
        assert!(within_band(u64::MAX, u64::MAX, 3), "no overflow");
    }

    /// RedStone's MXNe account on mainnet as read on 2026-11-05 (the first 98 of its 176 bytes;
    /// the rest is zero): 0.05380741 USD per MXNe at 12:25:10 UTC.
    const MAINNET_REFERENCE: &str = "e871c1e785d1ce9a4d584e65000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000521a857090ababa1010000014098ababa10100005d16c21b0000000008";

    fn mainnet_reference() -> Vec<u8> {
        let mut data: Vec<u8> = (0..MAINNET_REFERENCE.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&MAINNET_REFERENCE[i..i + 2], 16).unwrap())
            .collect();
        data.resize(176, 0);
        data
    }

    #[test]
    fn the_reference_account_is_redstones_mxne_pda_and_price_data() {
        let mut price = [0u8; 32];
        price[..5].copy_from_slice(b"price");
        let mut feed = [0u8; 32];
        feed[..4].copy_from_slice(REFERENCE_FEED_ID);
        assert_eq!(Pubkey::find_program_address(&[&price, &feed], &REFERENCE_PROGRAM).0, REFERENCE_ACCOUNT);
        assert_eq!(solana_sha256_hasher::hash(b"account:PriceData").to_bytes()[..8], REFERENCE_DISCRIMINATOR);
    }

    #[test]
    fn the_reference_reads_as_mxn_per_usd_rounded_down() {
        // 10^12 / 5_380_741 = 185_848.6: 18.5848 MXN per USD.
        assert_eq!(reference_rate(&mainnet_reference()).unwrap(), (185_848, 1_793_881_510));
        let mut none = mainnet_reference();
        // write_timestamp None: the slot and the decimals move eight bytes back.
        none.copy_within(89..98, 81);
        none[80] = 0;
        assert_eq!(reference_rate(&none).unwrap(), (185_848, 1_793_881_510));
        for (at, byte) in [(0, 0xe9), (11, b'x'), (12, 1), (39, 1), (40, 1), (63, 1), (80, 2), (97, 6)] {
            let mut bad = mainnet_reference();
            bad[at] = byte;
            assert!(reference_rate(&bad).is_err(), "byte {at}");
        }
        let mut zero = mainnet_reference();
        zero[64..72].fill(0);
        assert!(reference_rate(&zero).is_err());
        let mut tiny = mainnet_reference();
        tiny[64..72].copy_from_slice(&(REFERENCE_SCALE as u64 + 1).to_be_bytes());
        assert!(reference_rate(&tiny).is_err(), "a rate that rounds to zero");
        assert!(reference_rate(&mainnet_reference()[..97]).is_err());
    }

    #[test]
    fn the_gap_to_the_reference_is_two_percent_either_way() {
        assert!(within_bps(200_000, 204_000, MAX_FIX_REFERENCE_GAP_BPS));
        assert!(within_bps(200_000, 196_000, MAX_FIX_REFERENCE_GAP_BPS));
        assert!(!within_bps(200_000, 204_001, MAX_FIX_REFERENCE_GAP_BPS));
        assert!(!within_bps(200_000, 195_999, MAX_FIX_REFERENCE_GAP_BPS));
    }

    #[test]
    fn bounds_of_zero_and_zero_bound_nothing() {
        assert!(bounds_valid(0, 0) && bounds_valid(0, 1) && bounds_valid(5, 5));
        assert!(!bounds_valid(6, 5) && !bounds_valid(1, 0));
        assert!(within_bounds(1, 0, 0) && within_bounds(u64::MAX, 0, 0));
        assert!(within_bounds(5, 5, 6) && within_bounds(6, 5, 6));
        assert!(!within_bounds(4, 5, 6) && !within_bounds(7, 5, 6));
    }
}
