use super::sha256::Sha256;

#[test]
fn nist_sha256_vectors() {
    for (input, expected) in [
        (
            b"".as_slice(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc".as_slice(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".as_slice(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
    ] {
        let mut hash = Sha256::new();
        for chunk in input.chunks(2) {
            hash.update(chunk);
        }
        assert_eq!(hex(hash.finish()), expected);
    }
}

#[test]
fn nist_million_a_vector() {
    let mut hash = Sha256::new();
    let chunk = [b'a'; 1_000];
    for _ in 0..1_000 {
        hash.update(&chunk);
    }
    assert_eq!(
        hex(hash.finish()),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

fn hex(bytes: [u8; 32]) -> String {
    use std::fmt::Write;

    bytes.iter().fold(String::new(), |mut output, byte| {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
        output
    })
}
