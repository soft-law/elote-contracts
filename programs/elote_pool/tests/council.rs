//! The council of curators: constants and states fixed by the design (PR 4).

use elote_pool::state::*;

#[test]
fn council_constants_match_the_design() {
    assert_eq!(MAX_COUNCIL_MEMBERS, 10);
    assert_eq!(REQUEST_TTL_SECONDS, 15 * 86_400);
    assert_eq!(MIN_ACCELERATION_NOTICE_SECONDS, 7 * 86_400);
    assert_eq!(MIN_CANCEL_GRACE_SECONDS, 30 * 86_400);
    assert_eq!((COUNCIL_SQUADS, COUNCIL_GENERIC), (0, 1));
    assert_eq!(SQUADS_PROGRAM_ID.to_string(), "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
}

#[test]
fn a_requested_line_comes_before_an_approved_one() {
    assert_eq!(LineStatus::Requested as u8, 0);
    assert_eq!(LineStatus::Approved as u8, 1);
}
