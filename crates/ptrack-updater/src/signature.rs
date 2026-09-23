//! Release manifest authenticity.
//!
//! The release workflow signs the exact bytes of `checksums.txt` with the
//! project's Ed25519 release key and publishes the raw 64-byte signature as
//! `checksums.txt.sig`. The updater trusts a package digest only after that
//! signature verifies against the public key compiled in below, so a leaked
//! release token or a tampered release run cannot get a binary installed.

use ring::signature::{ED25519, UnparsedPublicKey};

use crate::discovery::UpdateError;

/// Raw Ed25519 public key of the p-track release signing key.
pub const RELEASE_SIGNING_PUBLIC_KEY: [u8; 32] = [
    0x2b, 0xe4, 0x96, 0x64, 0x34, 0x64, 0x75, 0xe5, 0xa5, 0xeb, 0x31, 0x84, 0xdb, 0x3b, 0x46, 0xf4,
    0xa9, 0x7a, 0x72, 0x5f, 0x60, 0xae, 0x44, 0xd0, 0xdd, 0xd1, 0xe2, 0x09, 0xab, 0x71, 0x3e, 0xb0,
];

/// Release asset holding the manifest signature.
pub const SIGNATURE_ASSET_NAME: &str = "checksums.txt.sig";

/// Size of a raw Ed25519 signature.
pub const SIGNATURE_BYTES: u64 = 64;

/// Verifies a raw Ed25519 `signature` over the exact `manifest` bytes.
///
/// # Errors
/// Returns [`UpdateError::InvalidSignature`] for a signature of the wrong
/// length or one that does not verify against `public_key`.
pub(crate) fn verify_manifest_signature(
    public_key: &[u8; 32],
    manifest: &[u8],
    signature: &[u8],
) -> Result<(), UpdateError> {
    if u64::try_from(signature.len()).ok() != Some(SIGNATURE_BYTES) {
        return Err(UpdateError::InvalidSignature);
    }
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(manifest, signature)
        .map_err(|_| UpdateError::InvalidSignature)
}
