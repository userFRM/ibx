//! Diffie-Hellman key exchange for establishing shared secrets.

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use num_bigint::BigUint;
use rand::RngCore;

use crate::auth::crypto::{aes_cbc_decrypt, aes_cbc_encrypt, hmac_sha1, strip_leading_zeros, tls10_prf};
use crate::auth::srp::SRP_N_STR;
use crate::protocol::ns::NS_MAGIC;

/// DH uses the same prime as SRP.
fn dh_n() -> BigUint {
    SRP_N_STR.parse().unwrap()
}

/// DH-based encrypted channel.
pub struct SecureChannel {
    client_random: [u8; 32],
    private_key: BigUint,
    public_key: BigUint,
    /// The cipher state, absent until the server hello derives it.
    keys: Option<Keys>,
}

/// What the handshake derived, and the two initialisation vectors that move
/// with the conversation.
///
/// One value rather than an option per piece. Every field here is written in
/// the same place from the same block, so separate options would let the type
/// hold a channel with a key for one direction and none for the other — a
/// state no handshake produces, and one every method would still have to
/// answer for.
struct Keys {
    /// The block the PRF produced. The AES and HMAC keys are fixed windows on
    /// it, read from it rather than copied out of it.
    block: Vec<u8>,
    /// Advances to the last cipher block of each message this side encrypts.
    write_iv: Vec<u8>,
    /// Advances to the last cipher block of each message this side decrypts.
    read_iv: Vec<u8>,
}

impl Keys {
    /// Client to server, `block[0..16]`.
    fn write_aes(&self) -> &[u8] {
        &self.block[0..16]
    }

    /// Server to client, `block[16..32]`.
    fn read_aes(&self) -> &[u8] {
        &self.block[16..32]
    }

    /// The write IV as derived, before any message advanced it,
    /// `block[32..48]`.
    fn initial_write_iv(&self) -> &[u8] {
        &self.block[32..48]
    }

    /// Client to server, `block[64..84]`.
    fn write_mac(&self) -> &[u8] {
        &self.block[64..84]
    }

    /// Server to client, `block[84..104]`.
    fn read_mac(&self) -> &[u8] {
        &self.block[84..104]
    }
}

/// What a caller has done wrong if the cipher state is missing: the channel
/// carries no keys until a server hello has been processed.
const BEFORE_HELLO: &str = "the channel enciphers only after a server hello derived its keys";

impl Default for SecureChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl SecureChannel {
    /// A fresh one.
    pub fn new() -> Self {
        let mut client_random = [0u8; 32];
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;
        client_random[0..4].copy_from_slice(&timestamp.to_be_bytes());
        rand::rng().fill_bytes(&mut client_random[4..]);

        let mut priv_bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut priv_bytes);
        let private_key = BigUint::from_bytes_be(&priv_bytes);
        let n = dh_n();
        let g = BigUint::from(2u32);
        let public_key = g.modpow(&private_key, &n);

        Self {
            client_random,
            private_key,
            public_key,
            keys: None,
        }
    }

    /// Build key exchange initiation message.
    pub fn build_secure_connect(&self, version: u32, negotiated_version: u32) -> Vec<u8> {
        let cr_b64 = B64.encode(self.client_random);

        // Encode public key as 128-byte big-endian, zero-padded
        let pub_bytes = self.public_key.to_bytes_be();
        let mut pub_padded = vec![0u8; 128];
        if pub_bytes.len() <= 128 {
            pub_padded[128 - pub_bytes.len()..].copy_from_slice(&pub_bytes);
        } else {
            // Shouldn't happen with 2048-bit prime, but strip leading zeros
            let stripped = strip_leading_zeros(&pub_bytes);
            let start = 128usize.saturating_sub(stripped.len());
            pub_padded[start..].copy_from_slice(&stripped[..128.min(stripped.len())]);
        }
        let pub_b64 = B64.encode(&pub_padded);

        let payload = format!(
            "{version};532;0;{negotiated_version};{cr_b64};{pub_b64};"
        );
        let payload_bytes = payload.as_bytes();
        let mut msg = Vec::with_capacity(8 + payload_bytes.len());
        msg.extend_from_slice(NS_MAGIC);
        msg.extend_from_slice(&(payload_bytes.len() as u32).to_be_bytes());
        msg.extend_from_slice(payload_bytes);
        msg
    }

    /// Parse server hello fields and derive keys.
    ///
    /// `fields` are the semicolon-split parts after version and msg_type:
    /// `[server_random_b64, server_pub_b64, ...]`
    ///
    /// # Server identity is not verified
    ///
    /// Only the first two fields are read. Any further field is ignored, and
    /// no signature or certificate presented by the server is parsed, checked
    /// for validity, or chained to a trust anchor. Key derivation therefore
    /// proceeds against whatever peer answered the connect, and every value in
    /// the key block descends from a public value that peer chose. The channel
    /// MAC then verifies against that key, so a substituted peer is not
    /// detectable downstream.
    ///
    /// The range check on the public value below is the only check made here.
    /// The primary connection is carried inside TLS, verified against the
    /// system trust store, so there the peer is authenticated before any of
    /// this runs. A farm connection has no such transport, and what
    /// authenticates its peer is the logon that follows: the venue states a
    /// proof of the session key it could only compute holding this account's
    /// verifier, and a logon whose proof does not match is refused. So a
    /// substituted peer completes this exchange and fails the next one.
    ///
    /// ## What the hello carries
    ///
    /// Eight fields, of which two are read:
    ///
    /// | | |
    /// |---|---|
    /// | 0 | the server's random, 32 bytes |
    /// | 1 | the server's public value, 128 bytes |
    /// | 2 | a 256-byte signature |
    /// | 3 | how many certificates follow |
    /// | 4.. | that many certificates, leaf first |
    ///
    /// The chain runs from a leaf naming the venue's own host, through an
    /// intermediate of the venue's, to a public certificate authority — so
    /// checking it needs no pinned copy and carries no rotation hazard.
    ///
    /// Reading them is worth doing on the farm connections, where nothing
    /// else authenticates the peer. It is worth little on the primary, where
    /// TLS has already done it. What the signature covers is not established:
    /// the two randoms and the two public values, alone and combined, in
    /// their raw and encoded forms, do not verify against the leaf under
    /// PKCS#1 v1.5 with SHA-1, SHA-256 or SHA-384.
    pub fn process_server_hello(&mut self, fields: &[&str]) -> std::io::Result<()> {
        let invalid = |what: &str| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("DH server hello: {what}"))
        };
        // Fields are matched and decoded rather than indexed: a short or
        // non-base64 hello fails the connection instead of panicking.
        let [server_random_b64, server_pub_b64, ..] = fields else {
            return Err(invalid(&format!("expected at least 2 fields, got {}", fields.len())));
        };
        let server_random = B64.decode(server_random_b64)
            .map_err(|e| invalid(&format!("undecodable server random: {e}")))?;
        let server_pub_bytes = B64.decode(server_pub_b64)
            .map_err(|e| invalid(&format!("undecodable server public value: {e}")))?;
        let server_pub = BigUint::from_bytes_be(&server_pub_bytes);

        let n = dh_n();

        // The public value must lie in [2, N-2]. 0 and 1 pin the pre-master
        // secret to a known constant, and with it the whole key block: both
        // AES keys, both IVs, both HMAC keys. The channel MAC would then
        // verify against that key, so nothing downstream would detect it.
        let two = BigUint::from(2u32);
        if server_pub < two || server_pub > &n - &two {
            return Err(invalid("public value outside [2, N-2]"));
        }

        // Pre-master secret = server_pub ^ client_private mod N
        let shared = server_pub.modpow(&self.private_key, &n);
        let shared_bytes = shared.to_bytes_be();
        let pre_master = strip_leading_zeros(&shared_bytes);

        // Master secret = PRF(pre_master, "master secret", client_random ||
        // server_random)
        let mut seed = Vec::with_capacity(64);
        seed.extend_from_slice(&self.client_random);
        seed.extend_from_slice(&server_random);
        let master_secret = tls10_prf(pre_master, "master secret", &seed, 48);

        // Key block = PRF(master_secret, "key expansion", client_random ||
        // server_random)
        let key_block = tls10_prf(&master_secret, "key expansion", &seed, 104);

        // Parse key block (104 bytes):
        // [0:16]   = client→server AES key
        // [16:32]  = server→client AES key
        // [32:48]  = client→server IV
        // [48:64]  = server→client IV
        // [64:84]  = client→server HMAC key
        // [84:104] = server→client HMAC key
        self.keys = Some(Keys {
            write_iv: key_block[32..48].to_vec(),
            read_iv: key_block[48..64].to_vec(),
            block: key_block,
        });
        Ok(())
    }

    /// Encrypt plaintext using Encrypt-then-MAC.
    ///
    /// Wire layout: `aes_cbc(plaintext) || hmac_sha1(mac_key, iv || ciphertext)`.
    /// Both auth and farm channels share this HMAC formula.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let keys = self.keys.as_mut().expect(BEFORE_HELLO);
        let ciphertext = aes_cbc_encrypt(keys.write_aes(), &keys.write_iv, plaintext);

        let mut mac_input = Vec::with_capacity(keys.write_iv.len() + ciphertext.len());
        mac_input.extend_from_slice(&keys.write_iv);
        mac_input.extend_from_slice(&ciphertext);
        let mac = hmac_sha1(keys.write_mac(), &mac_input);

        // CBC chaining: next message's IV = last 16 bytes of THIS ciphertext.
        keys.write_iv = ciphertext[ciphertext.len() - 16..].to_vec();

        let mut result = ciphertext;
        result.extend_from_slice(&mac);
        result
    }

    /// Verify MAC then decrypt.
    pub fn decrypt(&mut self, data: &[u8]) -> Result<Vec<u8>, &'static str> {
        if data.len() < 20 {
            return Err("data too short for MAC");
        }
        let ciphertext = &data[..data.len() - 20];
        let received_mac = &data[data.len() - 20..];

        let keys = self.keys.as_mut().expect(BEFORE_HELLO);

        let mut mac_input = Vec::with_capacity(keys.read_iv.len() + ciphertext.len());
        mac_input.extend_from_slice(&keys.read_iv);
        mac_input.extend_from_slice(ciphertext);
        let expected_mac = hmac_sha1(keys.read_mac(), &mac_input);

        if received_mac != expected_mac {
            return Err("HMAC verification failed");
        }

        let plaintext = aes_cbc_decrypt(keys.read_aes(), &keys.read_iv, ciphertext)?;

        // CBC chaining: next message's IV = last 16 bytes of THIS ciphertext.
        keys.read_iv = ciphertext[ciphertext.len() - 16..].to_vec();

        Ok(plaintext)
    }

    /// Encrypt with initial IVs from key derivation (for logon).
    pub fn encrypt_fresh(&self, plaintext: &[u8]) -> Vec<u8> {
        let keys = self.keys.as_ref().expect(BEFORE_HELLO);
        // The IV as derived, deliberately: this is the logon message, which is
        // enciphered under the initial vector rather than one a previous
        // message advanced.
        let iv = keys.initial_write_iv();

        let ciphertext = aes_cbc_encrypt(keys.write_aes(), iv, plaintext);

        let mut mac_input = Vec::with_capacity(iv.len() + ciphertext.len());
        mac_input.extend_from_slice(iv);
        mac_input.extend_from_slice(&ciphertext);
        let mac = hmac_sha1(keys.write_mac(), &mac_input);

        let mut result = ciphertext;
        result.extend_from_slice(&mac);
        result
    }

    /// Access the raw key block.
    pub fn key_block(&self) -> Option<&[u8]> {
        self.keys.as_ref().map(|k| k.block.as_slice())
    }

    /// Current write IV (updated after each encrypt call).
    pub fn write_iv(&self) -> Option<&[u8]> {
        self.keys.as_ref().map(|k| k.write_iv.as_slice())
    }

    /// Current read IV (updated after each decrypt call).
    pub fn read_iv(&self) -> Option<&[u8]> {
        self.keys.as_ref().map(|k| k.read_iv.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A channel whose key material is all zeroes, so what a test observes is
    /// the cipher and the chaining rather than the block behind them.
    fn make_test_channel() -> SecureChannel {
        let block = vec![0u8; 104];
        SecureChannel {
            client_random: [0u8; 32],
            private_key: BigUint::from(0u32),
            public_key: BigUint::from(0u32),
            keys: Some(Keys {
                write_iv: block[32..48].to_vec(),
                read_iv: block[48..64].to_vec(),
                block,
            }),
        }
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let mut enc_ch = make_test_channel();
        let mut dec_ch = make_test_channel();

        let plaintext = b"hello secure channel";
        let encrypted = enc_ch.encrypt(plaintext);
        let decrypted = dec_ch.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_multiple() {
        let mut enc_ch = make_test_channel();
        let mut dec_ch = make_test_channel();

        for i in 0..5 {
            let msg = format!("message {i}");
            let encrypted = enc_ch.encrypt(msg.as_bytes());
            let decrypted = dec_ch.decrypt(&encrypted).unwrap();
            assert_eq!(decrypted, msg.as_bytes());
        }
    }

    #[test]
    fn decrypt_bad_mac() {
        let mut enc_ch = make_test_channel();
        let mut dec_ch = make_test_channel();

        let encrypted = enc_ch.encrypt(b"test");
        let mut corrupted = encrypted.clone();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0xFF;
        assert!(dec_ch.decrypt(&corrupted).is_err());
    }

    #[test]
    fn encrypt_fresh_deterministic() {
        let ch = make_test_channel();
        let ct1 = ch.encrypt_fresh(b"logon message");
        let ct2 = ch.encrypt_fresh(b"logon message");
        // Same initial IVs → same ciphertext
        assert_eq!(ct1, ct2);
    }

    #[test]
    fn iv_chains_across_messages() {
        let mut ch = make_test_channel();
        let ct1 = ch.encrypt(b"first");
        let ct2 = ch.encrypt(b"second");
        // Different ciphertexts due to IV chaining
        assert_ne!(ct1, ct2);
    }

    #[test]
    fn build_secure_connect_format() {
        let ch = SecureChannel::new();
        let msg = ch.build_secure_connect(50, 50);
        assert_eq!(&msg[..4], NS_MAGIC);
        let payload = &msg[8..];
        let text = std::str::from_utf8(payload).unwrap();
        assert!(text.starts_with("50;532;0;50;"));
        assert!(text.ends_with(';'));
    }

    #[test]
    fn encrypt_fresh_output_is_valid_base64() {
        let ch = make_test_channel();
        let ct = ch.encrypt_fresh(b"some payload data");
        // The raw output is ciphertext || HMAC, not base64 itself.
        // But when base64-encoded, it should produce valid base64.
        let encoded = B64.encode(&ct);
        let decoded = B64.decode(&encoded).unwrap();
        assert_eq!(decoded, ct);
        // Ciphertext should be at least 16 (one AES block) + 20 (HMAC) = 36 bytes
        assert!(ct.len() >= 36);
    }

    #[test]
    fn encrypt_decrypt_max_payload() {
        let mut enc_ch = make_test_channel();
        let mut dec_ch = make_test_channel();
        let plaintext = vec![0xABu8; 4096];
        let encrypted = enc_ch.encrypt(&plaintext);
        let decrypted = dec_ch.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_one_byte() {
        let mut enc_ch = make_test_channel();
        let mut dec_ch = make_test_channel();
        let plaintext = &[0x42u8];
        let encrypted = enc_ch.encrypt(plaintext);
        let decrypted = dec_ch.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_empty_payload() {
        let mut enc_ch = make_test_channel();
        let mut dec_ch = make_test_channel();
        let plaintext: &[u8] = &[];
        let encrypted = enc_ch.encrypt(plaintext);
        let decrypted = dec_ch.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn build_secure_connect_different_versions() {
        let ch = SecureChannel::new();
        for &(ver, neg_ver) in &[(50, 50), (48, 50), (50, 52), (100, 200)] {
            let msg = ch.build_secure_connect(ver, neg_ver);
            assert_eq!(&msg[..4], NS_MAGIC);
            let payload = std::str::from_utf8(&msg[8..]).unwrap();
            let expected_prefix = format!("{ver};532;0;{neg_ver};");
            assert!(
                payload.starts_with(&expected_prefix),
                "Expected prefix '{expected_prefix}' but got '{payload}'"
            );
        }
    }

    #[test]
    fn key_block_none_before_server_hello() {
        let ch = SecureChannel::new();
        assert!(ch.key_block().is_none());
    }

    #[test]
    fn key_block_some_after_server_hello() {
        // Create two channels and exchange keys between them to simulate
        // a real handshake without needing a server.
        let mut channel_a = SecureChannel::new();
        let mut channel_b = SecureChannel::new();

        // Channel A builds its SECURE_CONNECT message
        let msg_a = channel_a.build_secure_connect(50, 50);
        let payload_a = std::str::from_utf8(&msg_a[8..]).unwrap();
        let parts_a: Vec<&str> = payload_a.trim_end_matches(';').split(';').collect();
        // parts_a: [version, 532, 0, negotiated_version, client_random_b64, pub_b64]
        let a_random = parts_a[4];
        let a_pub = parts_a[5];

        // Channel B builds its SECURE_CONNECT message
        let msg_b = channel_b.build_secure_connect(50, 50);
        let payload_b = std::str::from_utf8(&msg_b[8..]).unwrap();
        let parts_b: Vec<&str> = payload_b.trim_end_matches(';').split(';').collect();
        let b_random = parts_b[4];
        let b_pub = parts_b[5];

        // Each channel processes the other's hello as if it were a server response
        // process_server_hello expects [server_random_b64, server_pub_b64]
        channel_a.process_server_hello(&[b_random, b_pub]).unwrap();
        channel_b.process_server_hello(&[a_random, a_pub]).unwrap();

        // Both should now have key_blocks of 104 bytes
        let kb_a = channel_a.key_block().expect("channel_a should have key_block");
        assert_eq!(kb_a.len(), 104);
        let kb_b = channel_b.key_block().expect("channel_b should have key_block");
        assert_eq!(kb_b.len(), 104);
    }

    /// Both channels complete a key exchange and derive a usable key block.
    ///
    /// Each side seeds its key block with its own `client_random`, so two
    /// independent channels do not derive identical key blocks and this does
    /// not compare shared secrets. It checks only that both derivations
    /// produce a 104-byte key block and that both channels then encrypt.
    #[test]
    fn two_channels_exchange_keys_shared_secret_matches() {
        let mut channel_a = SecureChannel::new();
        let mut channel_b = SecureChannel::new();

        let msg_a = channel_a.build_secure_connect(50, 50);
        let payload_a = std::str::from_utf8(&msg_a[8..]).unwrap();
        let parts_a: Vec<&str> = payload_a.trim_end_matches(';').split(';').collect();

        let msg_b = channel_b.build_secure_connect(50, 50);
        let payload_b = std::str::from_utf8(&msg_b[8..]).unwrap();
        let parts_b: Vec<&str> = payload_b.trim_end_matches(';').split(';').collect();

        // Each processes the other's hello
        channel_a.process_server_hello(&[parts_b[4], parts_b[5]]).unwrap();
        channel_b.process_server_hello(&[parts_a[4], parts_a[5]]).unwrap();

        // Both have valid key blocks
        assert_eq!(channel_a.key_block().unwrap().len(), 104);
        assert_eq!(channel_b.key_block().unwrap().len(), 104);

        // Each can encrypt with their derived keys (no panics)
        let ct_a = channel_a.encrypt(b"message from A");
        let ct_b = channel_b.encrypt(b"message from B");
        // Ciphertexts are non-empty: at least 16 (AES block) + 20 (HMAC)
        assert!(ct_a.len() >= 36);
        assert!(ct_b.len() >= 36);

        // encrypt_fresh also works on both
        let fresh_a = channel_a.encrypt_fresh(b"fresh from A");
        let fresh_b = channel_b.encrypt_fresh(b"fresh from B");
        assert!(fresh_a.len() >= 36);
        assert!(fresh_b.len() >= 36);
    }

    /// The server's public value is range-checked. 0 and 1 pin the pre-master
    /// secret to a known constant, and every key derived from it follows,
    /// including the one the channel's own MAC verifies against.
    #[test]
    fn a_degenerate_public_value_is_refused() {
        let n = dh_n();
        let random = B64.encode([0u8; 32]);
        for (label, value) in [
            ("zero", BigUint::from(0u32)),
            ("one", BigUint::from(1u32)),
            ("n-1", &n - BigUint::from(1u32)),
            ("n", n.clone()),
        ] {
            let mut channel = SecureChannel::new();
            let pub_b64 = B64.encode(value.to_bytes_be());
            let err = channel.process_server_hello(&[&random, &pub_b64])
                .expect_err(&format!("{label} must be refused"));
            assert!(err.to_string().contains("outside [2, N-2]"), "{label}: {err}");
        }

        // The positive control: a value inside the range is still accepted, so
        // the assertions above are not passing for want of a working path.
        let mut channel = SecureChannel::new();
        let ok = B64.encode((&n - BigUint::from(2u32)).to_bytes_be());
        assert!(channel.process_server_hello(&[&random, &ok]).is_ok());
    }

    /// The half that needs no adversary: a short or non-base64 hello arrives
    /// on the ordinary connect path and is refused rather than indexed into.
    #[test]
    fn a_malformed_hello_is_an_error_not_a_panic() {
        let random = B64.encode([0u8; 32]);

        let mut channel = SecureChannel::new();
        assert!(channel.process_server_hello(&[]).is_err(), "no fields");

        let mut channel = SecureChannel::new();
        assert!(channel.process_server_hello(&[&random]).is_err(), "one field");

        let mut channel = SecureChannel::new();
        let err = channel.process_server_hello(&[&random, "not base64!!"]).unwrap_err();
        assert!(err.to_string().contains("undecodable"), "{err}");

        let mut channel = SecureChannel::new();
        let err = channel.process_server_hello(&["not base64!!", &random]).unwrap_err();
        assert!(err.to_string().contains("undecodable"), "{err}");
    }
}
