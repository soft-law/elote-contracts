//! The pagaré's registry: an append-only hash chain in the sidecar. Each entry's content goes in
//! the `PagareEntry` event; the sidecar keeps `log_root = sha256(log_root || entry_hash)`, the
//! count and the last kind, so anyone can replay the events and check the chain, and nobody can
//! rewrite or remove an entry.

use anchor_lang::prelude::Pubkey;
use solana_sha256_hasher::hashv;

use crate::state::Pagare;

/// The domain of a payment entry's annotation hash (LGTOC 17, 130: every abono states the amount
/// and the balance).
pub const PAYMENT_DOMAIN: &[u8] = b"elote.pago.v1";

/// What a payment entry commits (decision 38): the receipt, the principal applied to this pagaré,
/// the accessories (ordinary and late interest) the payment settled, the pagaré's unpaid principal
/// after it, and whether the suscriptor himself paid. The preimages travel in the `PaymentEntry`
/// of the `PagarePayment` event, so anyone can replay
/// `annotation_hash -> entry_hash -> log_root`. `interest_reserved` is the ordinary interest the
/// line still owes after the payment (decision 46): the entry reserves it, zero when none is left.
pub fn annotation_hash(
    receipt_hash: &[u8; 32],
    principal_applied: u64,
    accessory_applied: u64,
    principal_balance: u64,
    by_borrower: bool,
    interest_reserved: u64,
) -> [u8; 32] {
    hashv(&[
        PAYMENT_DOMAIN,
        receipt_hash,
        &principal_applied.to_le_bytes(),
        &accessory_applied.to_le_bytes(),
        &principal_balance.to_le_bytes(),
        &[u8::from(by_borrower)],
        &interest_reserved.to_le_bytes(),
    ])
    .to_bytes()
}

/// The domain of a delivery entry's hash.
pub const DELIVERY_DOMAIN: &[u8] = b"elote.entrega.v1";

/// What an `Entrega` entry commits: the payment's receipt and the ordinary interest the line still
/// owes when the título leaves (decision 46), zero when none.
pub fn delivery_hash(receipt_hash: &[u8; 32], interest_reserved: u64) -> [u8; 32] {
    hashv(&[DELIVERY_DOMAIN, receipt_hash, &interest_reserved.to_le_bytes()]).to_bytes()
}

/// `sha256(asset || kind || from || to || hash || stated_at LE || entry_count LE)`: the asset first,
/// so two pagarés with the same facts never share a chain.
pub fn entry_hash(asset: &Pubkey, kind: u8, from: &Pubkey, to: &Pubkey, hash: &[u8; 32], stated_at: i64, entry_count: u32) -> [u8; 32] {
    hashv(&[asset.as_ref(), &[kind], from.as_ref(), to.as_ref(), hash, &stated_at.to_le_bytes(), &entry_count.to_le_bytes()]).to_bytes()
}

/// Appends an entry to the sidecar's chain and returns its hash. `None` when the count would
/// overflow.
pub fn append(p: &mut Pagare, kind: u8, from: &Pubkey, to: &Pubkey, hash: &[u8; 32], stated_at: i64) -> Option<[u8; 32]> {
    let count = p.entry_count.checked_add(1)?;
    let entry = entry_hash(&p.asset, kind, from, to, hash, stated_at, count);
    p.log_root = hashv(&[&p.log_root, &entry]).to_bytes();
    p.entry_count = count;
    p.last_kind = kind;
    Some(entry)
}
