//! The terms hash: what the council approves. `request_line` computes it from the stored terms and
//! `approve_line` must be given the same value, which the curators compute from the documents.
//! The encoding is fixed; `clients/js/src/terms.ts` reproduces it and a shared vector pins both.

use anchor_lang::prelude::*;

use crate::state::ScheduleParams;

/// Domain separator of the encoding; a change of encoding changes it.
pub const TERMS_DOMAIN: &[u8] = b"elote_pool:terms:v1";

/// The terms of a line as `request_line` receives them.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct LineTerms {
    /// In centavos of `CURRENCY_MXN`.
    pub principal: u64,
    pub rate_bps: u16,
    /// Annual late interest on the overdue principal, in basis points, scheduled or bullet.
    pub late_bps: u16,
    pub term_end: i64,
    pub agreement_hash: [u8; 32],
    pub sponsor: Option<Pubkey>,
    pub sponsor_role: u8,
    pub schedule: Option<ScheduleParams>,
    pub expediente_root: [u8; 32],
    pub pagare_template_hash: [u8; 32],
}

/// `sha256(TERMS_DOMAIN || pool || line_id u64 || borrower || currency u16 || principal u64 ||
/// rate_bps u16 || term_end i64 || first_due_at i64 || interval_seconds i64 || installments u16 ||
/// late_bps u16 || max_missed u8 || sponsor_present u8 || sponsor (zero key when none) ||
/// sponsor_role u8 || agreement_hash || pagare_template_hash || expediente_root)`, integers little
/// endian, the schedule fields zero on a bullet line.
pub fn terms_hash(pool: &Pubkey, line_id: u64, borrower: &Pubkey, currency: u16, t: &LineTerms) -> [u8; 32] {
    let s = t.schedule.unwrap_or(ScheduleParams { first_due_at: 0, interval_seconds: 0, installments: 0, max_missed: 0 });
    let sponsor = t.sponsor.unwrap_or_default();
    solana_sha256_hasher::hashv(&[
        TERMS_DOMAIN,
        pool.as_ref(),
        &line_id.to_le_bytes(),
        borrower.as_ref(),
        &currency.to_le_bytes(),
        &t.principal.to_le_bytes(),
        &t.rate_bps.to_le_bytes(),
        &t.term_end.to_le_bytes(),
        &s.first_due_at.to_le_bytes(),
        &s.interval_seconds.to_le_bytes(),
        &s.installments.to_le_bytes(),
        &t.late_bps.to_le_bytes(),
        &[s.max_missed, u8::from(t.sponsor.is_some())],
        sponsor.as_ref(),
        &[t.sponsor_role],
        &t.agreement_hash,
        &t.pagare_template_hash,
        &t.expediente_root,
    ])
    .to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vector `clients/js/test/terms.test.ts` also checks.
    pub fn vector() -> (Pubkey, u64, Pubkey, u16, LineTerms) {
        let terms = LineTerms {
            principal: 5_000_000,
            rate_bps: 2_400,
            late_bps: 5_000,
            term_end: 1_800_000_000,
            agreement_hash: [0xa7; 32],
            sponsor: Some(Pubkey::new_from_array([0x55; 32])),
            sponsor_role: 1,
            schedule: Some(ScheduleParams {
                first_due_at: 1_770_000_000,
                interval_seconds: 2_592_000,
                installments: 12,
                max_missed: 3,
            }),
            expediente_root: [0xe7; 32],
            pagare_template_hash: [0x7e; 32],
        };
        (Pubkey::new_from_array([0x11; 32]), 7, Pubkey::new_from_array([0x22; 32]), 484, terms)
    }

    #[test]
    fn the_terms_hash_matches_the_pinned_vector() {
        let (pool, line_id, borrower, currency, t) = vector();
        let h = terms_hash(&pool, line_id, &borrower, currency, &t);
        let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "aac738788a0f850ca79bf7c0513d07af3a13e2fb6bff4758273e82d46b98cbfc");
    }

    #[test]
    fn every_term_moves_the_hash() {
        let (pool, line_id, borrower, currency, t) = vector();
        let base = terms_hash(&pool, line_id, &borrower, currency, &t);
        let mut bullet = t;
        bullet.schedule = None;
        let mut no_sponsor = t;
        no_sponsor.sponsor = None;
        let mut template = t;
        template.pagare_template_hash[0] ^= 1;
        for (label, h) in [
            ("line id", terms_hash(&pool, line_id + 1, &borrower, currency, &t)),
            ("borrower", terms_hash(&pool, line_id, &pool, currency, &t)),
            ("currency", terms_hash(&pool, line_id, &borrower, 840, &t)),
            ("schedule", terms_hash(&pool, line_id, &borrower, currency, &bullet)),
            ("sponsor", terms_hash(&pool, line_id, &borrower, currency, &no_sponsor)),
            ("template", terms_hash(&pool, line_id, &borrower, currency, &template)),
        ] {
            assert_ne!(h, base, "{label}");
        }
    }
}
