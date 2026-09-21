//! Encrypted persistence for model-manager settings.
//!
//! The key material is supplied by the host.  This store only keeps its SHA-256
//! derived key in memory; protecting the original key material is the host's
//! responsibility.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm,
};
use agent_api::{AgentError, ErrorCode};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

const FILE_MAGIC: &[u8] = b"MONA-MODEL-STORE";
const FILE_VERSION: u8 = 1;
const NONCE_SIZE: usize = 12;
const TAG_SIZE: usize = 16;
const MAX_SETTINGS_BYTES: usize = 2 * 1024 * 1024;
const HEADER_SIZE: usize = FILE_MAGIC.len() + 1 + NONCE_SIZE;
const MAX_FILE_BYTES: usize = HEADER_SIZE + MAX_SETTINGS_BYTES + TAG_SIZE;

const INVALID_STORE: &str = "invalid encrypted model settings";

/// A synchronous settings persistence boundary.
pub trait SettingsStore: Send + Sync {
    fn load(&self) -> agent_api::Result<Option<Vec<u8>>>;
    fn save(&self, bytes: &[u8]) -> agent_api::Result<()>;
}

/// AES-256-GCM encrypted settings backed by a single file.
///
/// The key material is deliberately not retained.  Callers must provide
/// externally managed, high-entropy material and protect it for the lifetime
/// of the host process.
pub struct EncryptedFileStore {
    path: PathBuf,
    key: [u8; 32],
}

impl EncryptedFileStore {
    /// Creates an encrypted file store.
    ///
    /// Key material is accepted only when it is at least 16 bytes long.  This
    /// is a boundary check, not password strengthening; the host must provide
    /// high-entropy material.
    pub fn new(path: PathBuf, key_material: &str) -> agent_api::Result<Self> {
        if key_material.is_empty() || key_material.as_bytes().len() < 16 {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "model settings key material must be at least 16 bytes",
            ));
        }

        let digest = Sha256::digest(key_material.as_bytes());
        let mut key = [0_u8; 32];
        key.copy_from_slice(&digest);

        Ok(Self { path, key })
    }

    fn cipher(&self) -> agent_api::Result<Aes256Gcm> {
        Aes256Gcm::new_from_slice(&self.key).map_err(|_| {
            AgentError::new(
                ErrorCode::Configuration,
                "model settings encryption is unavailable",
            )
        })
    }

    fn invalid_store() -> AgentError {
        AgentError::new(ErrorCode::Configuration, INVALID_STORE)
    }

    fn read_error() -> AgentError {
        AgentError::new(
            ErrorCode::Configuration,
            "model settings store could not be read",
        )
    }

    fn write_error() -> AgentError {
        AgentError::new(
            ErrorCode::Configuration,
            "model settings store could not be written",
        )
    }

    fn parent_directory(&self) -> &Path {
        self.path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
    }
}

impl SettingsStore for EncryptedFileStore {
    fn load(&self) -> agent_api::Result<Option<Vec<u8>>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Self::read_error()),
        };

        let metadata = file.metadata().map_err(|_| Self::read_error())?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(Self::invalid_store());
        }

        // Read one extra byte so a file that grows after metadata() is still
        // rejected without allocating unbounded memory.
        let mut limited = file.take((MAX_FILE_BYTES as u64) + 1);
        let mut stored = Vec::with_capacity(metadata.len() as usize);
        limited
            .read_to_end(&mut stored)
            .map_err(|_| Self::read_error())?;
        if stored.len() > MAX_FILE_BYTES || stored.len() < HEADER_SIZE + TAG_SIZE {
            return Err(Self::invalid_store());
        }

        if !stored.starts_with(FILE_MAGIC)
            || stored[FILE_MAGIC.len()] != FILE_VERSION
            || stored.len() < HEADER_SIZE + TAG_SIZE
        {
            return Err(Self::invalid_store());
        }

        let nonce_start = FILE_MAGIC.len() + 1;
        let nonce_end = nonce_start + NONCE_SIZE;
        let nonce = aes_gcm::Nonce::from_slice(&stored[nonce_start..nonce_end]);
        let header = &stored[..HEADER_SIZE];
        let ciphertext = &stored[HEADER_SIZE..];
        let plaintext = self
            .cipher()?
            .decrypt(
                nonce,
                Payload {
                    msg: ciphertext,
                    aad: header,
                },
            )
            .map_err(|_| Self::invalid_store())?;
        if plaintext.len() > MAX_SETTINGS_BYTES {
            return Err(Self::invalid_store());
        }

        Ok(Some(plaintext))
    }

    fn save(&self, bytes: &[u8]) -> agent_api::Result<()> {
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "model settings exceed the 2 MiB limit",
            ));
        }

        let cipher = self.cipher()?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut header = Vec::with_capacity(HEADER_SIZE);
        header.extend_from_slice(FILE_MAGIC);
        header.push(FILE_VERSION);
        header.extend_from_slice(nonce.as_slice());

        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: bytes,
                    aad: &header,
                },
            )
            .map_err(|_| Self::write_error())?;
        let mut stored = header;
        stored.extend_from_slice(&ciphertext);

        let parent = self.parent_directory();
        fs::create_dir_all(parent).map_err(|_| Self::write_error())?;

        let mut temporary = NamedTempFile::new_in(parent).map_err(|_| Self::write_error())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| Self::write_error())?;
        }

        temporary
            .as_file_mut()
            .write_all(&stored)
            .map_err(|_| Self::write_error())?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|_| Self::write_error())?;
        temporary
            .persist(&self.path)
            .map_err(|_| Self::write_error())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const KEY: &str = "0123456789abcdef0123456789abcdef";
    const OTHER_KEY: &str = "abcdef0123456789abcdef0123456789";

    fn store(path: PathBuf, key: &str) -> EncryptedFileStore {
        EncryptedFileStore::new(path, key).expect("test key is valid")
    }

    #[test]
    fn encrypted_roundtrip() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.bin");
        let store = store(path, KEY);
        let settings = br#"{"providers":[{"name":"private","api_key":"secret"}]}"#;

        store.save(settings).expect("save settings");

        assert_eq!(
            store.load().expect("load settings"),
            Some(settings.to_vec())
        );
    }

    #[test]
    fn file_does_not_contain_plaintext() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.bin");
        let store = store(path.clone(), KEY);
        let settings = b"provider-api-key-that-must-not-appear-in-the-file";

        store.save(settings).expect("save settings");

        let stored = fs::read(path).expect("read encrypted file");
        assert!(!stored
            .windows(settings.len())
            .any(|window| window == settings));
    }

    #[test]
    fn wrong_key_and_tampering_fail_as_configuration_errors() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.bin");
        let file_store = store(path.clone(), KEY);
        file_store.save(b"settings").expect("save settings");

        let wrong_key_error = store(path.clone(), OTHER_KEY)
            .load()
            .expect_err("wrong key must fail");
        assert_eq!(wrong_key_error.code, ErrorCode::Configuration);

        let mut stored = fs::read(&path).expect("read encrypted file");
        let last = stored.len() - 1;
        stored[last] ^= 1;
        fs::write(&path, stored).expect("tamper encrypted file");

        let tamper_error = store(path, KEY).load().expect_err("tampering must fail");
        assert_eq!(tamper_error.code, ErrorCode::Configuration);
    }

    #[test]
    fn missing_file_returns_none() {
        let directory = tempdir().expect("temporary directory");
        let store = store(directory.path().join("missing.bin"), KEY);

        assert_eq!(store.load().expect("load missing settings"), None);
    }

    #[test]
    fn save_overwrites_previous_settings() {
        let directory = tempdir().expect("temporary directory");
        let store = store(directory.path().join("settings.bin"), KEY);

        store.save(b"first").expect("save first settings");
        store.save(b"second").expect("save second settings");

        assert_eq!(
            store.load().expect("load settings"),
            Some(b"second".to_vec())
        );
    }

    #[test]
    fn oversized_settings_are_rejected() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.bin");
        let store = store(path.clone(), KEY);
        let oversized = vec![0_u8; MAX_SETTINGS_BYTES + 1];

        let error = store
            .save(&oversized)
            .expect_err("oversized settings must fail");
        assert_eq!(error.code, ErrorCode::Configuration);
        assert!(!path.exists());
    }

    #[test]
    fn short_key_material_is_rejected() {
        let directory = tempdir().expect("temporary directory");

        let error = match EncryptedFileStore::new(directory.path().join("settings.bin"), "short") {
            Err(error) => error,
            Ok(_) => panic!("short key material must fail"),
        };
        assert_eq!(error.code, ErrorCode::Configuration);
    }
}
