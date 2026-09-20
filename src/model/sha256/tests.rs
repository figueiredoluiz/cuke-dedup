use super::Sha256;

fn digest(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex(hasher.finalize())
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn matches_sha256_known_answer_vectors() {
    let cases = [
        (
            "empty",
            Vec::new(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            "short",
            b"abc".to_vec(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            "fips single block",
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".to_vec(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            "fips multi block",
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu".to_vec(),
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
        ),
        (
            "million bytes",
            vec![b'a'; 1_000_000],
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
        ),
    ];
    for (name, input, expected) in cases {
        assert_eq!(digest(&input), expected, "{name}");
    }
}

#[test]
fn padding_boundaries_are_correct() {
    for (length, expected) in [
        (
            55,
            "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
        ),
        (
            56,
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
        ),
        (
            57,
            "f13b2d724659eb3bf47f2dd6af1accc87b81f09f59f2b75e5c0bed6589dfe8c6",
        ),
        (
            64,
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
        ),
        (
            65,
            "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
        ),
    ] {
        assert_eq!(digest(&vec![b'a'; length]), expected, "{length} bytes");
    }
}

#[test]
fn chunk_boundaries_do_not_change_the_digest() {
    let input = (0..=255)
        .cycle()
        .take(1_025)
        .map(|byte| byte as u8)
        .collect::<Vec<_>>();
    let expected = digest(&input);
    for chunk_size in [1, 2, 3, 7, 31, 55, 56, 63, 64, 65, 127, 256, 1_024] {
        let mut hasher = Sha256::new();
        for chunk in input.chunks(chunk_size) {
            hasher.update(chunk);
        }
        assert_eq!(hex(hasher.finalize()), expected, "chunk size {chunk_size}");
    }
}
