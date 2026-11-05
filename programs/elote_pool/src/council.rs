//! The council: the vault of a Squads v4 multisig, checked by reading the multisig account at
//! fixed offsets (no Squads crate, no CPI).

use anchor_lang::prelude::*;

use crate::errors::PoolError;
use crate::state::{MAX_COUNCIL_MEMBERS, SQUADS_PROGRAM_ID};

/// `sha256("account:Multisig")[..8]`, Squads v4's account discriminator.
const MULTISIG_DISCRIMINATOR: [u8; 8] = [224, 116, 121, 186, 68, 161, 79, 236];
const CONFIG_AUTHORITY: usize = 40;
const THRESHOLD: usize = 72;
const RENT_COLLECTOR: usize = 94;
/// `Permissions` bit of a member who votes.
const PERMISSION_VOTE: u8 = 1 << 1;
/// One member: its key and its permission mask.
const MEMBER_LEN: usize = 33;

/// A council decision carries `valid_until`: Squads proposals never expire and execute even when
/// stale after a config change, so the program refuses one past its date (`ProposalExpired`; equal
/// is accepted).
pub fn fresh(valid_until: i64) -> Result<()> {
    require!(Clock::get()?.unix_timestamp <= valid_until, PoolError::ProposalExpired);
    Ok(())
}

/// `["multisig", multisig, "vault", 0, vault_bump]` under Squads: the multisig's vault 0.
pub fn vault_address(multisig: &Pubkey, vault_bump: u8) -> Result<Pubkey> {
    Pubkey::create_program_address(&[b"multisig", multisig.as_ref(), b"vault", &[0], &[vault_bump]], &SQUADS_PROGRAM_ID)
        .map_err(|_| error!(PoolError::CouncilInvalid))
}

/// Vault 0 of `multisig` with its canonical bump, the one Squads signs with.
pub fn canonical_vault(multisig: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"multisig", multisig.as_ref(), b"vault", &[0]], &SQUADS_PROGRAM_ID)
}

/// The members of `multisig`, read from its account: `(key, permission mask)` slices of 33 bytes.
fn members(data: &[u8]) -> Result<(u16, &[u8])> {
    require!(data.len() > RENT_COLLECTOR, PoolError::CouncilInvalid);
    require!(data[..8] == MULTISIG_DISCRIMINATOR, PoolError::CouncilInvalid);
    let threshold = u16::from_le_bytes([data[THRESHOLD], data[THRESHOLD + 1]]);
    let collector = match data[RENT_COLLECTOR] {
        0 => 0,
        1 => 32,
        _ => return err!(PoolError::CouncilInvalid),
    };
    // rent_collector, then the bump, then the members' length.
    let len_at = RENT_COLLECTOR + 1 + collector + 1;
    let list_at = len_at + 4;
    require!(data.len() >= list_at, PoolError::CouncilInvalid);
    let len = u32::from_le_bytes(data[len_at..list_at].try_into().map_err(|_| PoolError::CouncilInvalid)?) as usize;
    require!(len <= MAX_COUNCIL_MEMBERS, PoolError::CouncilTooLarge);
    let end = list_at + len * MEMBER_LEN;
    require!(data.len() >= end, PoolError::CouncilInvalid);
    Ok((threshold, &data[list_at..end]))
}

/// `council` is vault 0 of `multisig`, an autonomous Squads v4 multisig (no config authority, so
/// only its members' votes change it) of at most `MAX_COUNCIL_MEMBERS` whose threshold is a
/// majority of its voters. `CouncilInvalid` or `CouncilTooLarge` otherwise.
pub fn check(multisig: &AccountInfo, council: &Pubkey, vault_bump: u8) -> Result<()> {
    require_keys_eq!(*multisig.owner, SQUADS_PROGRAM_ID, PoolError::CouncilInvalid);
    require_keys_eq!(vault_address(multisig.key, vault_bump)?, *council, PoolError::CouncilInvalid);
    let data = multisig.try_borrow_data()?;
    require!(data.len() > RENT_COLLECTOR, PoolError::CouncilInvalid);
    require!(data[CONFIG_AUTHORITY..THRESHOLD].iter().all(|b| *b == 0), PoolError::CouncilInvalid);
    let (threshold, list) = members(&data)?;
    let voters = list.chunks_exact(MEMBER_LEN).filter(|m| m[32] & PERMISSION_VOTE != 0).count();
    require!(voters > 0 && usize::from(threshold) > voters / 2, PoolError::CouncilInvalid);
    Ok(())
}

/// Whether `key` is a member of `multisig` (read the same way as `check`, owner included:
/// `CouncilInvalid` for an account Squads does not own).
pub fn is_member(multisig: &AccountInfo, key: &Pubkey) -> Result<bool> {
    require_keys_eq!(*multisig.owner, SQUADS_PROGRAM_ID, PoolError::CouncilInvalid);
    let data = multisig.try_borrow_data()?;
    let (_, list) = members(&data)?;
    Ok(list.chunks_exact(MEMBER_LEN).any(|m| m[..32] == key.as_ref()[..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_discriminator_is_squads_multisig() {
        let d = solana_sha256_hasher::hashv(&[b"account:Multisig"]).to_bytes();
        assert_eq!(MULTISIG_DISCRIMINATOR, d[..8]);
    }
}
