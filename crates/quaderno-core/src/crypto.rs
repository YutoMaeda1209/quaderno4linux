//! Cryptographic primitives used by the registration (pairing) protocol.
//!
//! * Diffie-Hellman over the RFC 3526 2048-bit MODP group (group 14, g = 2)
//! * PBKDF2-HMAC-SHA256 key derivation
//! * HMAC-SHA256 message authentication
//! * AES-128-CBC "key wrap" (`AES-CBC(PKCS7(data || HMAC(data)[..8])) || IV`)

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use num_bigint::{BigUint, RandBigInt};
use rand::RngCore;
use sha2::Sha256;

use crate::error::{Error, Result};

type HmacSha256 = Hmac<Sha256>;
type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// RFC 3526, 2048-bit MODP group (id 14).
const MODP_2048_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD1",
    "29024E088A67CC74020BBEA63B139B22514A08798E3404DD",
    "EF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245",
    "E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3D",
    "C2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F",
    "83655D23DCA3AD961C62F356208552BB9ED529077096966D",
    "670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B",
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9",
    "DE2BCBF6955817183995497CEA956AE515D2261898FA0510",
    "15728E5A8AACAA68FFFFFFFFFFFFFFFF",
);

/// Length in bytes of a group-14 element.
pub const DH_LEN: usize = 256;

/// Ephemeral Diffie-Hellman key pair.
pub struct DiffieHellman {
    prime: BigUint,
    secret: BigUint,
}

impl DiffieHellman {
    pub fn new() -> Self {
        let prime = BigUint::parse_bytes(MODP_2048_HEX.as_bytes(), 16).expect("valid prime");
        let mut rng = rand::thread_rng();
        let secret = rng.gen_biguint(256) | BigUint::from(1u8) << 255;
        Self { prime, secret }
    }

    /// Public value `g^a mod p` as 256 big-endian bytes.
    pub fn public_key(&self) -> [u8; DH_LEN] {
        let public = BigUint::from(2u8).modpow(&self.secret, &self.prime);
        to_fixed(&public)
    }

    /// Shared secret `peer^a mod p` as 256 big-endian bytes.
    pub fn shared_secret(&self, peer: &[u8]) -> Result<[u8; DH_LEN]> {
        let peer = BigUint::from_bytes_be(peer);
        if peer <= BigUint::from(1u8) || peer >= &self.prime - 1u8 {
            return Err(Error::Registration("不正な DH 公開鍵".into()));
        }
        Ok(to_fixed(&peer.modpow(&self.secret, &self.prime)))
    }
}

impl Default for DiffieHellman {
    fn default() -> Self {
        Self::new()
    }
}

fn to_fixed(n: &BigUint) -> [u8; DH_LEN] {
    let bytes = n.to_bytes_be();
    let mut out = [0u8; DH_LEN];
    out[DH_LEN - bytes.len()..].copy_from_slice(&bytes);
    out
}

/// Normalises a big-endian integer received from the device to exactly 256 bytes
/// (the device may prepend a `0x00` sign byte).
pub fn normalize_dh(bytes: &[u8]) -> Result<[u8; DH_LEN]> {
    let n = BigUint::from_bytes_be(bytes);
    if n.bits() > (DH_LEN * 8) as u64 {
        return Err(Error::Registration("DH 公開鍵が長すぎます".into()));
    }
    Ok(to_fixed(&n))
}

pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut out);
    out
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    rand::thread_rng().fill_bytes(&mut out);
    out
}

/// Session keys derived from the DH exchange.
pub struct SessionKeys {
    pub auth_key: [u8; 32],
    pub key_wrap_key: [u8; 16],
}

impl SessionKeys {
    pub fn derive(shared: &[u8], salt: &[u8]) -> Self {
        let derived = pbkdf2_sha256(shared, salt, 10_000, 48);
        let mut auth_key = [0u8; 32];
        let mut key_wrap_key = [0u8; 16];
        auth_key.copy_from_slice(&derived[..32]);
        key_wrap_key.copy_from_slice(&derived[32..]);
        Self { auth_key, key_wrap_key }
    }

    pub fn hmac(&self, parts: &[&[u8]]) -> [u8; 32] {
        hmac_sha256(&self.auth_key, parts)
    }

    /// `AES-128-CBC(PKCS7(data || HMAC(data)[..8])) || IV`
    pub fn wrap(&self, data: &[u8]) -> Vec<u8> {
        let iv: [u8; 16] = random_bytes();
        self.wrap_with_iv(data, &iv)
    }

    fn wrap_with_iv(&self, data: &[u8], iv: &[u8; 16]) -> Vec<u8> {
        let kwa = self.hmac(&[data]);
        let mut plain = Vec::with_capacity(data.len() + 8);
        plain.extend_from_slice(data);
        plain.extend_from_slice(&kwa[..8]);
        let mut out = Aes128CbcEnc::new(&self.key_wrap_key.into(), iv.into()).encrypt_padded_vec_mut::<Pkcs7>(&plain);
        out.extend_from_slice(iv);
        out
    }

    pub fn unwrap(&self, wrapped: &[u8]) -> Result<Vec<u8>> {
        if wrapped.len() < 32 || !wrapped.len().is_multiple_of(16) {
            return Err(Error::Registration("ラップされたデータの長さが不正です".into()));
        }
        let (cipher, iv) = wrapped.split_at(wrapped.len() - 16);
        let iv: [u8; 16] = iv.try_into().expect("16 bytes");
        let plain = Aes128CbcDec::new(&self.key_wrap_key.into(), &iv.into())
            .decrypt_padded_vec_mut::<Pkcs7>(cipher)
            .map_err(|_| Error::Registration("復号に失敗しました".into()))?;
        if plain.len() < 8 {
            return Err(Error::Registration("復号データが短すぎます".into()));
        }
        let (data, kwa) = plain.split_at(plain.len() - 8);
        if self.hmac(&[data])[..8] != *kwa {
            return Err(Error::Registration("ラップデータの HMAC が一致しません".into()));
        }
        Ok(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dh_agreement() {
        let a = DiffieHellman::new();
        let b = DiffieHellman::new();
        let sa = a.shared_secret(&b.public_key()).unwrap();
        let sb = b.shared_secret(&a.public_key()).unwrap();
        assert_eq!(sa, sb);
    }

    #[test]
    fn normalize_strips_sign_byte() {
        let mut v = vec![0u8];
        v.extend_from_slice(&[0xffu8; 256]);
        assert_eq!(normalize_dh(&v).unwrap(), [0xffu8; 256]);
        assert_eq!(normalize_dh(&[1, 2]).unwrap()[254..], [1, 2]);
    }

    #[test]
    fn wrap_roundtrip() {
        let keys = SessionKeys::derive(b"shared", b"salt");
        for len in [0usize, 1, 15, 16, 17, 500] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let wrapped = keys.wrap(&data);
            assert_eq!(wrapped.len() % 16, 0);
            assert_eq!(keys.unwrap(&wrapped).unwrap(), data);
        }
    }

    #[test]
    fn unwrap_rejects_tampering() {
        let keys = SessionKeys::derive(b"shared", b"salt");
        let mut wrapped = keys.wrap(b"hello world");
        wrapped[0] ^= 1;
        assert!(keys.unwrap(&wrapped).is_err());
    }

    #[test]
    fn pbkdf2_known_vector() {
        // RFC 7914 section 11 (PBKDF2-HMAC-SHA256, c = 1)
        let out = pbkdf2_sha256(b"passwd", b"salt", 1, 16);
        assert_eq!(
            out,
            [0x55, 0xac, 0x04, 0x6e, 0x56, 0xe3, 0x08, 0x9f, 0xec, 0x16, 0x91, 0xc2, 0x25, 0x44, 0xb6, 0x05]
        );
    }
}
