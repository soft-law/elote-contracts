//! A `Pagare` record read and written in place, at its Borsh offsets, for `repay` and `condone`:
//! they read a few fields of each record and change only its payment state and registry, so they
//! neither deserialize nor serialize the whole account. `tests/layout.rs` pins every offset
//! against the Borsh encoding of `state::Pagare`.

use anchor_lang::prelude::*;
use anchor_lang::{Discriminator, Space};

use crate::registry;
use crate::state::Pagare;

/// Byte offsets in the account, discriminator included.
pub mod offset {
    pub const LINE: usize = 8;
    pub const DRAW_INDEX: usize = 40;
    pub const ASSET: usize = 42;
    pub const AMOUNT_MXN: usize = 106;
    pub const DRAWN_THROUGH: usize = 130;
    /// `status` through `last_kind` are contiguous: the only bytes `store` writes.
    pub const STATUS: usize = 171;
    pub const PAID_AT: usize = 172;
    pub const RECEIPT_HASH: usize = 180;
    pub const PROCURADOR: usize = 212;
    pub const FLAGS: usize = 244;
    pub const LOG_ROOT: usize = 245;
    pub const ENTRY_COUNT: usize = 277;
    pub const LAST_KIND: usize = 281;
    pub const MUTABLE_END: usize = 282;
    pub const USDC_DELIVERED: usize = 295;
}

/// The fields of a record that a payment or a quita reads, and the ones it may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidecarFields {
    pub line: Pubkey,
    pub draw_index: u16,
    pub asset: Pubkey,
    pub amount_mxn: u64,
    pub drawn_through: u64,
    pub usdc_delivered: u64,
    pub status: u8,
    pub paid_at: i64,
    pub receipt_hash: [u8; 32],
    pub procurador: Pubkey,
    pub flags: u8,
    pub log_root: [u8; 32],
    pub entry_count: u32,
    pub last_kind: u8,
}

fn bytes32(d: &[u8], at: usize) -> [u8; 32] {
    let mut a = [0u8; 32];
    a.copy_from_slice(&d[at..at + 32]);
    a
}

fn bytes8(d: &[u8], at: usize) -> [u8; 8] {
    let mut a = [0u8; 8];
    a.copy_from_slice(&d[at..at + 8]);
    a
}

impl SidecarFields {
    /// The fields of `data`, a whole `Pagare` account; `None` unless it has the record's exact
    /// length and discriminator.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() != 8 + Pagare::INIT_SPACE || &data[..8] != Pagare::DISCRIMINATOR {
            return None;
        }
        use offset::*;
        Some(Self {
            line: Pubkey::new_from_array(bytes32(data, LINE)),
            draw_index: u16::from_le_bytes([data[DRAW_INDEX], data[DRAW_INDEX + 1]]),
            asset: Pubkey::new_from_array(bytes32(data, ASSET)),
            amount_mxn: u64::from_le_bytes(bytes8(data, AMOUNT_MXN)),
            drawn_through: u64::from_le_bytes(bytes8(data, DRAWN_THROUGH)),
            usdc_delivered: u64::from_le_bytes(bytes8(data, USDC_DELIVERED)),
            status: data[STATUS],
            paid_at: i64::from_le_bytes(bytes8(data, PAID_AT)),
            receipt_hash: bytes32(data, RECEIPT_HASH),
            procurador: Pubkey::new_from_array(bytes32(data, PROCURADOR)),
            flags: data[FLAGS],
            log_root: bytes32(data, LOG_ROOT),
            entry_count: u32::from_le_bytes([data[ENTRY_COUNT], data[ENTRY_COUNT + 1], data[ENTRY_COUNT + 2], data[ENTRY_COUNT + 3]]),
            last_kind: data[LAST_KIND],
        })
    }

    /// Writes `status` through `last_kind` into `data`, a whole `Pagare` account; nothing else.
    pub fn write_into(&self, data: &mut [u8]) {
        use offset::*;
        let d = &mut data[STATUS..MUTABLE_END];
        let at = |o: usize| o - STATUS;
        d[at(STATUS)] = self.status;
        d[at(PAID_AT)..at(RECEIPT_HASH)].copy_from_slice(&self.paid_at.to_le_bytes());
        d[at(RECEIPT_HASH)..at(PROCURADOR)].copy_from_slice(&self.receipt_hash);
        d[at(PROCURADOR)..at(FLAGS)].copy_from_slice(self.procurador.as_ref());
        d[at(FLAGS)] = self.flags;
        d[at(LOG_ROOT)..at(ENTRY_COUNT)].copy_from_slice(&self.log_root);
        d[at(ENTRY_COUNT)..at(LAST_KIND)].copy_from_slice(&self.entry_count.to_le_bytes());
        d[at(LAST_KIND)] = self.last_kind;
    }
}

/// A record loaded from an account owned by this program with the `Pagare` discriminator.
pub struct Sidecar<'info> {
    info: &'info AccountInfo<'info>,
    pub fields: SidecarFields,
}

impl<'info> Sidecar<'info> {
    /// `None` unless `info` is owned by this program and holds a `Pagare` record.
    pub fn load(info: &'info AccountInfo<'info>) -> Option<Self> {
        if info.owner != &crate::ID {
            return None;
        }
        let fields = SidecarFields::parse(&info.try_borrow_data().ok()?)?;
        Some(Self { info, fields })
    }

    /// Appends an entry to the record's chain (`registry::append_entry`).
    pub fn append(&mut self, kind: u8, from: &Pubkey, to: &Pubkey, hash: &[u8; 32], stated_at: i64) -> Option<[u8; 32]> {
        let f = &mut self.fields;
        registry::append_entry(&f.asset, &mut f.log_root, &mut f.entry_count, &mut f.last_kind, kind, from, to, hash, stated_at)
    }

    /// Writes the changed fields back.
    pub fn store(&self) -> Result<()> {
        self.fields.write_into(&mut self.info.try_borrow_mut_data()?);
        Ok(())
    }
}

impl core::ops::Deref for Sidecar<'_> {
    type Target = SidecarFields;
    fn deref(&self) -> &SidecarFields {
        &self.fields
    }
}

impl core::ops::DerefMut for Sidecar<'_> {
    fn deref_mut(&mut self) -> &mut SidecarFields {
        &mut self.fields
    }
}
