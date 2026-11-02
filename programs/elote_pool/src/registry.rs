//! The pagaré's registry: an append-only hash chain in the sidecar. Each entry's content goes in
//! the `PagareEntry` event; the sidecar keeps `log_root = sha256(log_root || entry_hash)`, the
//! count and the last kind, so anyone can replay the events and check the chain, and nobody can
//! rewrite or remove an entry.

use anchor_lang::prelude::Pubkey;
use solana_sha256_hasher::hashv;

use crate::state::Pagare;

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
