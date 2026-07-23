//! Decrypt Granola's encrypted local token store.
//!
//! Recent Granola versions no longer keep the Supabase auth token in a
//! plaintext `supabase.json`. Instead they use the standard Chromium/Electron
//! safeStorage scheme, with three layers:
//!
//! ```text
//! safeStorage key   (from the OS credential store)
//! storage.dek       (safeStorage-encrypted data key, base64-encoded plaintext)
//! supabase.json.enc (data-key-encrypted token JSON)
//! ```
//!
//! Only the innermost unseal of `storage.dek` is platform-specific; everything
//! after it (base64-decoding the data key, then AES-256-GCM decrypting
//! `supabase.json.enc`) is shared:
//!
//! - **Windows** (`os_crypt`): `Local State` holds a DPAPI-wrapped AES-256
//!   master key, keyed to the current user. `storage.dek` is a `"v10"`-prefixed
//!   AES-256-GCM blob whose auth tag makes a wrong framing fail loudly.
//! - **macOS** (safeStorage): a password lives in the login Keychain (service
//!   `"Granola Safe Storage"`, account `"Granola"`). An AES-128 key is derived
//!   from it via PBKDF2-HMAC-SHA1, and `storage.dek` is a `"v10"`-prefixed
//!   AES-128-CBC blob with an all-spaces IV and PKCS#7 padding (no auth tag).

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use anyhow::{bail, Context, Result};
use base64::Engine;
use log::debug;
use sha1::Sha1;
use std::path::Path;

const NONCE_LEN: usize = 12;

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct LocalState {
    os_crypt: OsCrypt,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct OsCrypt {
    encrypted_key: String,
}

/// Decrypt the Granola token store in `granola_dir` and return the raw JSON
/// string that the plaintext `supabase.json` used to contain.
pub fn decrypt_token_json(granola_dir: &Path) -> Result<String> {
    // storage.dek unseals to the base64-encoded data key; the unseal step is
    // platform-specific, everything after it is not.
    let dek_plaintext = unseal_dek(granola_dir)?;
    let data_key = base64::engine::general_purpose::STANDARD
        .decode(dek_plaintext.trim_ascii())
        .context("storage.dek plaintext was not valid base64")?;
    debug!("Recovered safeStorage data key ({} bytes)", data_key.len());

    let token_blob = std::fs::read(granola_dir.join("supabase.json.enc"))
        .context("Failed to read supabase.json.enc")?;
    // supabase.json.enc carries no version prefix: nonce + ciphertext + tag.
    let token_plaintext =
        gcm_open(&data_key, &token_blob, 0).context("Failed to decrypt supabase.json.enc")?;

    String::from_utf8(token_plaintext).context("Decrypted token payload was not valid UTF-8")
}

/// Read `storage.dek` and unseal it to the base64 data-key plaintext using the
/// Windows os_crypt master key (`Local State` + DPAPI).
#[cfg(windows)]
fn unseal_dek(granola_dir: &Path) -> Result<Vec<u8>> {
    let master = master_key(granola_dir)?;
    let dek_blob = std::fs::read(granola_dir.join("storage.dek"))
        .context("Failed to read storage.dek")?;
    // storage.dek is a "v10"-prefixed os_crypt GCM blob.
    gcm_open(&master, &dek_blob, 3).context("Failed to decrypt storage.dek")
}

/// Read `storage.dek` and unseal it to the base64 data-key plaintext using the
/// macOS safeStorage key (Keychain password + PBKDF2 + AES-128-CBC).
#[cfg(target_os = "macos")]
fn unseal_dek(granola_dir: &Path) -> Result<Vec<u8>> {
    let key = derive_cbc_key(&keychain_password()?);
    let dek_blob = std::fs::read(granola_dir.join("storage.dek"))
        .context("Failed to read storage.dek")?;
    // storage.dek is a "v10"-prefixed safeStorage CBC blob.
    cbc_open(&key, &dek_blob, 3).context("Failed to decrypt storage.dek")
}

/// Reading Granola's encrypted token store is not implemented on this platform.
/// This bail runs before any credential-store or `storage.dek` access so users
/// see the `--token` guidance instead of a confusing parse error.
#[cfg(not(any(windows, target_os = "macos")))]
fn unseal_dek(_granola_dir: &Path) -> Result<Vec<u8>> {
    bail!(
        "Reading Granola's encrypted token store is currently implemented only on \
         Windows and macOS. On this platform, pass a token explicitly with --token."
    )
}

/// Read Electron's safeStorage password from the macOS login Keychain.
///
/// The first read triggers a Keychain access prompt; approving "Always Allow"
/// suppresses it on later runs.
#[cfg(target_os = "macos")]
fn keychain_password() -> Result<Vec<u8>> {
    security_framework::passwords::get_generic_password("Granola Safe Storage", "Granola").context(
        "Failed to read the 'Granola Safe Storage' password from the macOS Keychain. \
         Ensure Granola is installed and allow access when prompted, or pass --token.",
    )
}

/// Read and OS-unwrap the AES-256 master key from Granola's `Local State`.
#[cfg(windows)]
fn master_key(granola_dir: &Path) -> Result<Vec<u8>> {
    let content = std::fs::read_to_string(granola_dir.join("Local State"))
        .context("Failed to read Local State")?;
    let state: LocalState =
        serde_json::from_str(&content).context("Failed to parse Local State")?;

    let wrapped = base64::engine::general_purpose::STANDARD
        .decode(&state.os_crypt.encrypted_key)
        .context("os_crypt.encrypted_key was not valid base64")?;

    let stripped = wrapped
        .strip_prefix(b"DPAPI")
        .context("os_crypt.encrypted_key missing expected 'DPAPI' prefix")?;

    let key = os_unwrap(stripped)?;
    debug!("Unwrapped os_crypt master key ({} bytes)", key.len());
    Ok(key)
}

/// Decrypt an os_crypt-style AES-256-GCM blob laid out as
/// `[prefix][12-byte nonce][ciphertext + 16-byte tag]`.
fn gcm_open(key: &[u8], blob: &[u8], prefix_len: usize) -> Result<Vec<u8>> {
    let body = blob
        .get(prefix_len..)
        .context("Encrypted blob shorter than its version prefix")?;
    let (nonce, ciphertext) = body
        .split_at_checked(NONCE_LEN)
        .context("Encrypted blob too short to contain a nonce")?;

    if key.len() != 32 {
        bail!("AES-256 key must be 32 bytes, got {}", key.len());
    }
    let key = Key::<Aes256Gcm>::from_slice(key);
    Aes256Gcm::new(key)
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| anyhow::anyhow!("AES-GCM authentication failed (wrong key or corrupt data)"))
}

/// Derive the AES-128 safeStorage key from a Keychain password using Electron's
/// fixed PBKDF2 parameters (HMAC-SHA1, salt "saltysalt", 1003 iterations).
///
/// Compiled on every platform so the derivation stays unit-testable; only the
/// macOS unseal path calls it in a real build.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn derive_cbc_key(password: &[u8]) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<Sha1>(password, b"saltysalt", 1003, &mut key);
    key
}

/// Decrypt a safeStorage AES-128-CBC blob laid out as `[prefix][ciphertext]`
/// with a fixed all-spaces IV and PKCS#7 padding. Unlike the GCM layout there
/// is no nonce or auth tag, so a wrong key surfaces as a padding error rather
/// than an authentication failure (and can occasionally decode to garbage).
///
/// Compiled on every platform so the CBC layer stays unit-testable; only the
/// macOS unseal path calls it in a real build.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn cbc_open(key: &[u8], blob: &[u8], prefix_len: usize) -> Result<Vec<u8>> {
    use aes::Aes128;
    use cbc::cipher::block_padding::Pkcs7;
    use cbc::cipher::{BlockDecryptMut, KeyIvInit};

    type Aes128CbcDec = cbc::Decryptor<Aes128>;

    let ciphertext = blob
        .get(prefix_len..)
        .context("Encrypted blob shorter than its version prefix")?;

    let iv = [b' '; 16];
    let cipher = Aes128CbcDec::new_from_slices(key, &iv)
        .map_err(|_| anyhow::anyhow!("AES-128 key must be 16 bytes, got {}", key.len()))?;

    let mut buf = ciphertext.to_vec();
    let plaintext = cipher
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| anyhow::anyhow!("AES-CBC decryption failed (wrong key or corrupt data)"))?;
    Ok(plaintext.to_vec())
}

/// Unwrap a key blob using the OS credential store.
#[cfg(windows)]
fn os_unwrap(blob: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: blob.len() as u32,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    // SAFETY: `input` points at `blob` for the duration of the call; on success
    // Windows allocates `output.pbData`, which we copy out and then LocalFree.
    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    if ok == 0 {
        bail!("CryptUnprotectData failed; is Granola installed under this Windows user?");
    }

    // SAFETY: on success `output.pbData` is valid for `output.cbData` bytes.
    let key = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };
    // SAFETY: freeing the buffer Windows allocated for us.
    unsafe { LocalFree(output.pbData as _) };
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Aead;

    /// Encrypt with the same framing Granola uses, so we can exercise gcm_open
    /// without depending on the OS credential store.
    fn seal(key: &[u8], nonce: &[u8], plaintext: &[u8], prefix: &[u8]) -> Vec<u8> {
        let key = Key::<Aes256Gcm>::from_slice(key);
        let ciphertext = Aes256Gcm::new(key)
            .encrypt(Nonce::from_slice(nonce), plaintext)
            .unwrap();
        [prefix, nonce, &ciphertext].concat()
    }

    #[test]
    fn gcm_open_round_trips_with_v10_prefix() {
        let key = [7u8; 32];
        let nonce = [3u8; NONCE_LEN];
        let blob = seal(&key, &nonce, b"the data key", b"v10");

        let out = gcm_open(&key, &blob, 3).unwrap();
        assert_eq!(out, b"the data key");
    }

    #[test]
    fn gcm_open_round_trips_without_prefix() {
        let key = [9u8; 32];
        let nonce = [1u8; NONCE_LEN];
        let blob = seal(&key, &nonce, b"{\"workos_tokens\":{}}", b"");

        let out = gcm_open(&key, &blob, 0).unwrap();
        assert_eq!(out, b"{\"workos_tokens\":{}}");
    }

    #[test]
    fn gcm_open_rejects_wrong_key() {
        let nonce = [1u8; NONCE_LEN];
        let blob = seal(&[9u8; 32], &nonce, b"secret", b"");

        let result = gcm_open(&[0u8; 32], &blob, 0);
        assert!(result.is_err());
    }

    #[test]
    fn gcm_open_rejects_short_blob() {
        assert!(gcm_open(&[0u8; 32], b"v10short", 3).is_err());
    }

    #[test]
    fn gcm_open_rejects_bad_key_length() {
        let blob = seal(&[9u8; 32], &[1u8; NONCE_LEN], b"x", b"");
        assert!(gcm_open(&[0u8; 16], &blob, 0).is_err());
    }

    /// Encrypt with the same AES-128-CBC framing macOS safeStorage uses (fixed
    /// all-spaces IV, PKCS#7 padding), so we can exercise cbc_open without a
    /// live Keychain.
    fn cbc_seal(key: &[u8; 16], plaintext: &[u8], prefix: &[u8]) -> Vec<u8> {
        use aes::Aes128;
        use cbc::cipher::block_padding::Pkcs7;
        use cbc::cipher::{BlockEncryptMut, KeyIvInit};

        type Aes128CbcEnc = cbc::Encryptor<Aes128>;

        let iv = [b' '; 16];
        let mut buf = vec![0u8; plaintext.len() + 16];
        let ciphertext = Aes128CbcEnc::new_from_slices(key, &iv)
            .unwrap()
            .encrypt_padded_b2b_mut::<Pkcs7>(plaintext, &mut buf)
            .unwrap();
        [prefix, ciphertext].concat()
    }

    #[test]
    fn cbc_open_round_trips_with_v10_prefix() {
        let key = [0x11u8; 16];
        let blob = cbc_seal(&key, b"the data key", b"v10");

        let out = cbc_open(&key, &blob, 3).unwrap();
        assert_eq!(out, b"the data key");
    }

    #[test]
    fn cbc_open_round_trips_without_prefix() {
        let key = [0x22u8; 16];
        let blob = cbc_seal(&key, b"{\"workos_tokens\":{}}", b"");

        let out = cbc_open(&key, &blob, 0).unwrap();
        assert_eq!(out, b"{\"workos_tokens\":{}}");
    }

    #[test]
    fn cbc_open_rejects_bad_key_length() {
        let blob = cbc_seal(&[0x22u8; 16], b"x", b"");
        assert!(cbc_open(&[0u8; 15], &blob, 0).is_err());
    }

    #[test]
    fn cbc_open_rejects_short_prefix() {
        assert!(cbc_open(&[0u8; 16], b"v1", 3).is_err());
    }

    #[test]
    fn cbc_open_rejects_non_block_aligned_ciphertext() {
        // 6 bytes after the prefix is not a whole AES block, so unpadding fails.
        assert!(cbc_open(&[0u8; 16], b"v10short!", 3).is_err());
    }

    #[test]
    fn derive_cbc_key_matches_known_vector() {
        // PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1003, 16 bytes) is Chromium's
        // documented safeStorage vector; pins salt, iteration count, and digest.
        let key = derive_cbc_key(b"peanuts");
        assert_eq!(
            key,
            [217, 160, 157, 73, 155, 78, 27, 116, 97, 242, 142, 103, 151, 44, 109, 189]
        );
    }

    #[test]
    fn derive_cbc_key_round_trips_through_cbc_open() {
        let key = derive_cbc_key(b"Granola Safe Storage password");
        let blob = cbc_seal(&key, b"YmFzZTY0IGRhdGEga2V5", b"v10");

        let out = cbc_open(&key, &blob, 3).unwrap();
        assert_eq!(out, b"YmFzZTY0IGRhdGEga2V5");
    }
}
