use ring::signature::{Ed25519KeyPair, KeyPair};

use super::UpdateError;
use super::signature::{RELEASE_SIGNING_PUBLIC_KEY, verify_manifest_signature};

/// `openssl pkeyutl -sign -rawin` output for the six bytes `hello\n`, made
/// with the production release key. Pins that the compiled-in key and the
/// release workflow's raw signature format agree.
const PRODUCTION_VECTOR_MESSAGE: &[u8] = b"hello\n";
const PRODUCTION_VECTOR_SIGNATURE: &str = concat!(
    "4f4e5f0cd12ae5245acf62d934ea67a4cb595e3f854b1d06e1eec77138278657",
    "40197b357ea52081d51e6c7ee034ee423685e65ac433a0e80383b53fab209f06"
);

#[test]
fn production_key_verifies_an_openssl_raw_signature() {
    let signature = decode_hex(PRODUCTION_VECTOR_SIGNATURE);
    assert_eq!(
        verify_manifest_signature(
            &RELEASE_SIGNING_PUBLIC_KEY,
            PRODUCTION_VECTOR_MESSAGE,
            &signature
        ),
        Ok(())
    );
    assert_eq!(
        verify_manifest_signature(&RELEASE_SIGNING_PUBLIC_KEY, b"hello!", &signature),
        Err(UpdateError::InvalidSignature)
    );
}

#[test]
fn signature_is_bound_to_exact_bytes_key_and_length() {
    let key_pair = Ed25519KeyPair::from_seed_unchecked(&[9; 32]).unwrap();
    let key: [u8; 32] = key_pair.public_key().as_ref().try_into().unwrap();
    let manifest = b"00  ptrack_1.2.4_linux_amd64.tar.gz\n";
    let signature = key_pair.sign(manifest).as_ref().to_vec();
    assert_eq!(
        verify_manifest_signature(&key, manifest, &signature),
        Ok(())
    );

    let mut tampered = manifest.to_vec();
    tampered[0] = b'1';
    assert_eq!(
        verify_manifest_signature(&key, &tampered, &signature),
        Err(UpdateError::InvalidSignature)
    );
    let other = Ed25519KeyPair::from_seed_unchecked(&[10; 32]).unwrap();
    let other: [u8; 32] = other.public_key().as_ref().try_into().unwrap();
    assert_eq!(
        verify_manifest_signature(&other, manifest, &signature),
        Err(UpdateError::InvalidSignature)
    );
    for length in [0, 63, 65] {
        let mut resized = signature.clone();
        resized.resize(length, 0);
        assert_eq!(
            verify_manifest_signature(&key, manifest, &resized),
            Err(UpdateError::InvalidSignature)
        );
    }
}

fn decode_hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
        .collect()
}
