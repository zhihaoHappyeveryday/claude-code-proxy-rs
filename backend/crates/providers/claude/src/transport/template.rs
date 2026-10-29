use wreq::tls::{AlpnProtocol, ExtensionType, KeyShare, TlsOptions, TlsVersion};

// 来自本地 Claude Code 2.1.312/macOS arm64 捕获，不由被测实现生成期望值。
pub(super) fn claude_template() -> TlsOptions {
    TlsOptions::builder()
        .alpn_protocols([AlpnProtocol::HTTP1])
        .min_tls_version(TlsVersion::TLS_1_2)
        .max_tls_version(TlsVersion::TLS_1_3)
        .cipher_list(concat!(
            "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:",
            "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:",
            "ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:",
            "ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:",
            "ECDHE-ECDSA-AES128-SHA:ECDHE-RSA-AES128-SHA:",
            "ECDHE-ECDSA-AES256-SHA:ECDHE-RSA-AES256-SHA:",
            "AES128-GCM-SHA256:AES256-GCM-SHA384:AES128-SHA:AES256-SHA"
        ))
        .preserve_tls13_cipher_list(true)
        .curves_list("X25519MLKEM768:X25519:P-256:P-384")
        .key_shares(vec![KeyShare::X25519_MLKEM768, KeyShare::X25519])
        .sigalgs_list(concat!(
            "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:",
            "ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:",
            "rsa_pss_rsae_sha512:rsa_pkcs1_sha512:ed25519:ecdsa_secp521r1_sha512:rsa_pkcs1_sha1"
        ))
        .extension_permutation(
            [0, 23, 65281, 10, 11, 35, 16, 5, 13, 18, 51, 45, 43]
                .map(ExtensionType::from)
                .to_vec(),
        )
        .permute_extensions(false)
        .grease_enabled(false)
        .enable_ech_grease(false)
        .enable_ocsp_stapling(true)
        .enable_signed_cert_timestamps(true)
        .pre_shared_key(false)
        .build()
}
