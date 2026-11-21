//! Reads v1's secure.dat. v1 encrypted it with Electron's safeStorage: AES-256-GCM ("v10" + 12-byte nonce +
//! ciphertext + tag) with a key stored in the app's "Local State" file, itself protected by Windows (DPAPI) for
//! this user. Only this Windows user can read it, which is also what keeps it safe.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use serde_json::{Map, Value};
use std::path::Path;

/// Decrypts with DPAPI for the current user.
#[cfg(windows)]
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: input points at `data` for the call; output is allocated by Windows and freed below.
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output).map_err(|e| format!("Windows could not unlock the v1 key: {e}"))?;
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(output.pbData as _)));
        Ok(plain)
    }
}

#[cfg(not(windows))]
pub fn unprotect(_data: &[u8]) -> Result<Vec<u8>, String> {
    Err("v1 settings can only be read on Windows.".into())
}

/// Encrypts with DPAPI (tests make v1-like files with it).
#[cfg(windows)]
pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `unprotect`.
    unsafe {
        CryptProtectData(&input, None, None, None, None, 0, &mut output).map_err(|e| e.to_string())?;
        let sealed = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(output.pbData as _)));
        Ok(sealed)
    }
}

/// The AES key from "Local State" (os_crypt.encrypted_key = base64("DPAPI" + protected key)).
pub fn master_key(folder: &Path) -> Result<Vec<u8>, String> {
    let text = std::fs::read_to_string(folder.join("Local State")).map_err(|_| "v1's Local State file is missing.".to_string())?;
    let state: Value = serde_json::from_str(&text).map_err(|_| "v1's Local State file is damaged.".to_string())?;
    let encoded = state["os_crypt"]["encrypted_key"].as_str().ok_or("v1's Local State has no key.")?;
    let wrapped = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| "v1's key is damaged.")?;
    let protected = wrapped.strip_prefix(b"DPAPI").ok_or("v1's key has an unknown format.")?;
    let key = unprotect(protected)?;
    if key.len() != 32 {
        return Err("v1's key has an unexpected size.".into());
    }
    Ok(key)
}

/// Decrypts one safeStorage value ("v10" format).
pub fn decrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let body = data.strip_prefix(b"v10").ok_or("Unknown v1 encryption format.")?;
    if body.len() < 12 + 16 {
        return Err("v1 settings file is too short.".into());
    }
    let (nonce, sealed) = body.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "bad key size")?;
    cipher.decrypt(Nonce::from_slice(nonce), sealed).map_err(|_| "v1 settings could not be decrypted (another Windows user?).".to_string())
}

/// secure.dat as a JSON object. Plain JSON (no encryption available in v1) is read as it is.
pub fn read(folder: &Path) -> Result<Map<String, Value>, String> {
    let raw = std::fs::read(folder.join("secure.dat")).map_err(|_| "v1 has no secure.dat.".to_string())?;
    let text = if raw.first() == Some(&b'{') { raw } else { decrypt(&master_key(folder)?, &raw)? };
    match serde_json::from_slice::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err("v1's secure.dat has unexpected content.".into()),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use aes_gcm::aead::AeadCore;

    #[test]
    fn a_v1_like_secure_file_is_read_back() {
        let dir = std::env::temp_dir().join(format!("anytask-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = Aes256Gcm::generate_key(&mut aes_gcm::aead::OsRng);
        let mut wrapped = b"DPAPI".to_vec();
        wrapped.extend(protect(&key).unwrap());
        let state = serde_json::json!({ "os_crypt": { "encrypted_key": base64::engine::general_purpose::STANDARD.encode(wrapped) } });
        std::fs::write(dir.join("Local State"), state.to_string()).unwrap();

        let nonce = Aes256Gcm::generate_nonce(&mut aes_gcm::aead::OsRng);
        let sealed = Aes256Gcm::new(&key).encrypt(&nonce, br#"{"ANYTASK_USER_NAME":"Sara"}"#.as_ref()).unwrap();
        let mut file = b"v10".to_vec();
        file.extend_from_slice(&nonce);
        file.extend(sealed);
        std::fs::write(dir.join("secure.dat"), file).unwrap();

        let map = read(&dir).unwrap();
        assert_eq!(map["ANYTASK_USER_NAME"], "Sara");

        // A damaged file is an error, not a crash.
        std::fs::write(dir.join("secure.dat"), b"v10short").unwrap();
        assert!(read(&dir).is_err());
        std::fs::write(dir.join("secure.dat"), br#"{"plain":true}"#).unwrap();
        assert_eq!(read(&dir).unwrap()["plain"], true);
        let _ = std::fs::remove_dir_all(dir);
    }
}
