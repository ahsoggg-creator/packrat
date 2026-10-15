//! ZIP encryption: WinZip AES (AE-1 / AE-2, 128/192/256 bit) for reading and writing, and the old PKWARE
//! "ZipCrypto" for reading only. ZipCrypto is weak (known-plaintext attacks recover the keys), so Packrat
//! never creates it and warns when it meets it.

use crate::error::{Error, Result};
use aes::{Aes128, Aes192, Aes256};
use ctr::Ctr128LE;
use ctr::cipher::{KeyIvInit, StreamCipher};
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use std::io::{self, Read};

type HmacSha1 = Hmac<Sha1>;

const TAG_LEN: usize = 10;
const VERIFIER_LEN: usize = 2;

pub fn aes_key_len(strength: u8) -> Option<usize> {
    match strength {
        1 => Some(16),
        2 => Some(24),
        3 => Some(32),
        _ => None,
    }
}

pub fn aes_salt_len(strength: u8) -> Option<usize> {
    aes_key_len(strength).map(|k| k / 2)
}

/// Bytes added around the data of an AES entry: salt, password verifier and authentication code.
pub fn aes_overhead(strength: u8) -> usize {
    aes_salt_len(strength).unwrap_or(16) + VERIFIER_LEN + TAG_LEN
}

enum Ctr {
    A128(Ctr128LE<Aes128>),
    A192(Ctr128LE<Aes192>),
    A256(Ctr128LE<Aes256>),
}

impl Ctr {
    fn new(key: &[u8]) -> Ctr {
        // WinZip: a little-endian block counter that starts at 1
        let mut iv = [0u8; 16];
        iv[0] = 1;
        match key.len() {
            16 => Ctr::A128(Ctr128LE::<Aes128>::new_from_slices(key, &iv).expect("key length")),
            24 => Ctr::A192(Ctr128LE::<Aes192>::new_from_slices(key, &iv).expect("key length")),
            _ => Ctr::A256(Ctr128LE::<Aes256>::new_from_slices(key, &iv).expect("key length")),
        }
    }

    fn apply(&mut self, data: &mut [u8]) {
        match self {
            Ctr::A128(c) => c.apply_keystream(data),
            Ctr::A192(c) => c.apply_keystream(data),
            Ctr::A256(c) => c.apply_keystream(data),
        }
    }
}

struct Keys {
    cipher: Ctr,
    mac: HmacSha1,
    verifier: [u8; VERIFIER_LEN],
}

fn derive(password: &str, salt: &[u8], key_len: usize) -> Keys {
    let mut out = vec![0u8; 2 * key_len + VERIFIER_LEN];
    pbkdf2::pbkdf2_hmac::<Sha1>(password.as_bytes(), salt, 1000, &mut out);
    Keys {
        cipher: Ctr::new(&out[..key_len]),
        mac: HmacSha1::new_from_slice(&out[key_len..2 * key_len]).expect("HMAC takes any key length"),
        verifier: [out[2 * key_len], out[2 * key_len + 1]],
    }
}

/// Decrypts the data of one AES entry (`salt | verifier | ciphertext | tag`) and checks the tag at its end.
pub struct AesReader<R: Read> {
    inner: R,
    keys: Keys,
    remaining: u64,
    finished: bool,
}

impl<R: Read> AesReader<R> {
    /// `stored_len` is the compressed size from the ZIP headers (it includes salt, verifier and tag).
    pub fn new(mut inner: R, stored_len: u64, strength: u8, password: &str) -> Result<Self> {
        let key_len = aes_key_len(strength).ok_or_else(|| Error::Unsupported(format!("AES strength {strength}")))?;
        let salt_len = key_len / 2;
        let overhead = (salt_len + VERIFIER_LEN + TAG_LEN) as u64;
        if stored_len < overhead {
            return Err(Error::Corrupt("encrypted entry is too short".into()));
        }
        let mut salt = vec![0u8; salt_len];
        inner.read_exact(&mut salt)?;
        let mut verifier = [0u8; VERIFIER_LEN];
        inner.read_exact(&mut verifier)?;
        let keys = derive(password, &salt, key_len);
        if keys.verifier != verifier {
            return Err(Error::WrongPassword);
        }
        Ok(Self {
            inner,
            keys,
            remaining: stored_len - overhead,
            finished: false,
        })
    }
}

impl<R: Read> Read for AesReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished || buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut tag = [0u8; TAG_LEN];
            self.inner.read_exact(&mut tag)?;
            self.finished = true;
            // the tag covers the ciphertext: a wrong password that passed the 2-byte check, or damaged data, ends here
            let mac = std::mem::replace(&mut self.keys.mac, HmacSha1::new_from_slice(&[0]).expect("key"));
            return match mac.verify_truncated_left(&tag) {
                Ok(()) => Ok(0),
                Err(_) => Err(Error::Integrity("authentication failed: wrong password or damaged data".into()).into()),
            };
        }
        let want = buf.len().min(self.remaining.min(usize::MAX as u64) as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.keys.mac.update(&buf[..n]);
        self.keys.cipher.apply(&mut buf[..n]);
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Encrypts the data of one AES-256 entry as it is written: call `header` first, then `encrypt` for every chunk,
/// then `finish` for the authentication code.
pub struct AesWriter {
    keys: Keys,
    salt: [u8; 16],
}

pub const AES256_STRENGTH: u8 = 3;

impl AesWriter {
    pub fn new(password: &str) -> Result<Self> {
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|e| Error::Other(format!("no random numbers available: {e}")))?;
        Ok(Self::with_salt(password, salt))
    }

    fn with_salt(password: &str, salt: [u8; 16]) -> Self {
        Self {
            keys: derive(password, &salt, 32),
            salt,
        }
    }

    /// `salt | password verifier`.
    pub fn header(&self) -> Vec<u8> {
        let mut h = self.salt.to_vec();
        h.extend(self.keys.verifier);
        h
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        self.keys.cipher.apply(data);
        self.keys.mac.update(data);
    }

    pub fn finish(self) -> [u8; TAG_LEN] {
        let full = self.keys.mac.finalize().into_bytes();
        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&full[..TAG_LEN]);
        tag
    }
}

// ---- ZipCrypto (read only) -----------------------------------------------------------------------------------

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

fn crc_step(crc: u32, b: u8) -> u32 {
    CRC_TABLE[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8)
}

struct ZipCryptoKeys([u32; 3]);

impl ZipCryptoKeys {
    fn new(password: &[u8]) -> Self {
        let mut k = ZipCryptoKeys([0x1234_5678, 0x2345_6789, 0x3456_7890]);
        for &b in password {
            k.update(b);
        }
        k
    }

    fn update(&mut self, b: u8) {
        self.0[0] = crc_step(self.0[0], b);
        self.0[1] = self.0[1]
            .wrapping_add(self.0[0] & 0xff)
            .wrapping_mul(134_775_813)
            .wrapping_add(1);
        self.0[2] = crc_step(self.0[2], (self.0[1] >> 24) as u8);
    }

    fn stream_byte(&self) -> u8 {
        let t = (self.0[2] | 2) as u16;
        (t.wrapping_mul(t ^ 1) >> 8) as u8
    }

    fn decrypt(&mut self, c: u8) -> u8 {
        let p = c ^ self.stream_byte();
        self.update(p);
        p
    }
}

/// Decrypts a ZipCrypto entry (12 header bytes, then data). `check` is the byte the last header byte must equal.
pub struct ZipCryptoReader<R: Read> {
    inner: R,
    keys: ZipCryptoKeys,
}

impl<R: Read> ZipCryptoReader<R> {
    pub fn new(mut inner: R, password: &str, check: u8) -> Result<Self> {
        let mut keys = ZipCryptoKeys::new(password.as_bytes());
        let mut header = [0u8; 12];
        inner.read_exact(&mut header)?;
        let mut last = 0;
        for b in header {
            last = keys.decrypt(b);
        }
        if last != check {
            return Err(Error::WrongPassword);
        }
        Ok(Self { inner, keys })
    }
}

impl<R: Read> Read for ZipCryptoReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        for b in &mut buf[..n] {
            *b = self.keys.decrypt(*b);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// What a ZipCrypto encryptor does, to test the decryptor (Packrat never writes ZipCrypto archives itself).
    fn zipcrypto_encrypt(password: &str, check: u8, data: &[u8]) -> Vec<u8> {
        let mut k = ZipCryptoKeys::new(password.as_bytes());
        let mut header = [7u8; 12];
        header[11] = check;
        let mut out = Vec::new();
        for b in header.iter().chain(data) {
            let c = b ^ k.stream_byte();
            k.update(*b);
            out.push(c);
        }
        out
    }

    #[test]
    fn zipcrypto_roundtrip_and_wrong_password() {
        let packed = zipcrypto_encrypt("secret", 0xAB, b"hello zipcrypto");
        let mut out = Vec::new();
        ZipCryptoReader::new(Cursor::new(packed.clone()), "secret", 0xAB)
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, b"hello zipcrypto");
        assert!(matches!(
            ZipCryptoReader::new(Cursor::new(packed), "wrong", 0xAB),
            Err(Error::WrongPassword)
        ));
    }

    #[test]
    fn aes_roundtrip_detects_wrong_password_and_tampering() {
        let data: Vec<u8> = (0..50_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut w = AesWriter::with_salt("correct horse", [3u8; 16]);
        let mut stored = w.header();
        let mut body = data.clone();
        w.encrypt(&mut body);
        stored.extend(&body);
        stored.extend(w.finish());
        let len = stored.len() as u64;

        let mut out = Vec::new();
        AesReader::new(Cursor::new(stored.clone()), len, AES256_STRENGTH, "correct horse")
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, data);

        assert!(matches!(
            AesReader::new(Cursor::new(stored.clone()), len, AES256_STRENGTH, "wrong"),
            Err(Error::WrongPassword)
        ));

        let mut bad = stored.clone();
        bad[100] ^= 1;
        let mut out = Vec::new();
        let err = AesReader::new(Cursor::new(bad), len, AES256_STRENGTH, "correct horse")
            .unwrap()
            .read_to_end(&mut out)
            .unwrap_err();
        assert!(err.to_string().contains("authentication"), "{err}");
    }
}
