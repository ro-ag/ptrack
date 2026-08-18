use ptrack_core::{check_identity_name, is_identity_id};
use ptrack_store::{ActorIdentity, GlobalStore};

use crate::{AppError, AppResult};

/// Global-database config key holding `<identity-id>\t<display-name>`.
pub const IDENTITY_CONFIG_KEY: &[u8] = b"user.identity";

const IDENTITY_ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
const MALFORMED: &str =
    "stored user identity is malformed; run 'ptrack config set user <name>' to repair it";

/// Reads the configured identity, `None` when none was ever set.
///
/// # Errors
/// Fails closed on a malformed stored value rather than guessing or re-minting.
pub fn load_identity(store: &GlobalStore) -> AppResult<Option<ActorIdentity>> {
    let stored = store.config(IDENTITY_CONFIG_KEY)?;
    if stored.is_empty() {
        return Ok(None);
    }
    parse_identity(&stored).map_or_else(|| Err(malformed()), |identity| Ok(Some(identity)))
}

/// Sets the display name, minting the stable identity ID on first use.
/// A malformed stored value is repaired by re-minting (set is the documented
/// repair path; load never re-mints).
///
/// # Errors
/// Returns a printable error for an unusable name or a storage failure.
pub fn set_identity_name(store: &GlobalStore, name: &str) -> AppResult<ActorIdentity> {
    let name = name.trim();
    check_identity_name(name).map_err(AppError::Message)?;
    let minted = mint_identity_id()?;
    let identity = store.update_config(IDENTITY_CONFIG_KEY, |stored| {
        let id = if stored.is_empty() {
            minted.clone()
        } else {
            parse_identity(stored).map_or_else(|| minted.clone(), |existing| existing.id)
        };
        let identity = ActorIdentity {
            id: id.clone(),
            name: name.to_owned(),
        };
        Ok((format!("{id}\t{name}").into_bytes(), identity))
    })?;
    Ok(identity)
}

fn parse_identity(stored: &[u8]) -> Option<ActorIdentity> {
    let text = std::str::from_utf8(stored).ok()?;
    let (id, name) = text.split_once('\t')?;
    if !is_identity_id(id) || check_identity_name(name).is_err() {
        return None;
    }
    Some(ActorIdentity {
        id: id.to_owned(),
        name: name.to_owned(),
    })
}

fn malformed() -> AppError {
    AppError::Message(MALFORMED.to_owned())
}

/// Mints a 26-character lowercase Crockford-base32 ULID: 48 bits of Unix
/// milliseconds followed by 80 random bits.
fn mint_identity_id() -> AppResult<String> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_millis());
    let mut random = [0_u8; 10];
    getrandom::fill(&mut random)
        .map_err(|error| AppError::Message(format!("identity randomness unavailable: {error}")))?;
    let mut low: u128 = 0;
    for byte in random {
        low = (low << 8) | u128::from(byte);
    }
    let value = ((millis & 0xFFFF_FFFF_FFFF) << 80) | low;
    let mut id = String::with_capacity(26);
    for index in 0..26 {
        let shift = 125 - index * 5;
        let digit = usize::try_from((value >> shift) & 0x1F).expect("5-bit digit fits usize");
        id.push(char::from(IDENTITY_ALPHABET[digit]));
    }
    Ok(id)
}
