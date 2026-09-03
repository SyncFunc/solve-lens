use crate::domain::{AnswerResult, QuestionDraft};
use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{Duration, Utc};
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use uuid::Uuid;

const NONCE_LENGTH: usize = 12;

#[derive(Clone)]
pub struct HistoryStore {
    root: PathBuf,
    enabled: bool,
}

#[derive(Serialize, Deserialize)]
struct HistoryPayload {
    draft: StoredDraft,
    answer: AnswerResult,
}

#[derive(Serialize, Deserialize)]
struct StoredDraft {
    id: Uuid,
    preset_id: String,
    created_at: chrono::DateTime<Utc>,
    images: Vec<StoredImage>,
}

#[derive(Serialize, Deserialize)]
struct StoredImage {
    captured_at: chrono::DateTime<Utc>,
    png_base64: String,
}

impl HistoryStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("entries"))?;
        let store = Self {
            root,
            enabled: true,
        };
        store.connection()?.execute_batch(
            "CREATE TABLE IF NOT EXISTS history_entries (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                path TEXT NOT NULL
            );",
        )?;
        store.cleanup()?;
        Ok(store)
    }

    /// Creates a non-fatal fallback for environments where the app data
    /// directory is read-only (for example, a locked-down portable install).
    /// The main UI can still solve questions; save() reports the limitation.
    pub fn unavailable(root: PathBuf) -> Self {
        Self {
            root,
            enabled: false,
        }
    }

    pub fn save(&self, draft: &QuestionDraft, answer: &AnswerResult) -> Result<()> {
        if !self.enabled {
            anyhow::bail!("本地历史目录不可写，已跳过历史保存");
        }
        let id = Uuid::new_v4();
        let images = draft
            .images
            .iter()
            .map(|image| -> Result<_> {
                Ok(StoredImage {
                    captured_at: image.captured_at,
                    png_base64: BASE64.encode(
                        fs::read(&image.path).context("read screenshot for encrypted history")?,
                    ),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let stored_draft = StoredDraft {
            id: draft.id,
            preset_id: draft.preset_id.clone(),
            created_at: draft.created_at,
            images,
        };
        let plaintext = serde_json::to_vec(&HistoryPayload {
            draft: stored_draft,
            answer: answer.clone(),
        })?;
        let ciphertext = self.encrypt(&plaintext)?;
        let relative_path = format!("entries/{id}.bin");
        fs::write(self.root.join(&relative_path), ciphertext)
            .context("write encrypted history entry")?;
        self.connection()?.execute(
            "INSERT INTO history_entries(id, created_at, path) VALUES (?1, ?2, ?3)",
            params![id.to_string(), Utc::now().timestamp(), relative_path],
        )?;
        Ok(())
    }

    pub fn cleanup(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let cutoff = (Utc::now() - Duration::days(30)).timestamp();
        let connection = self.connection()?;
        let mut statement =
            connection.prepare("SELECT path FROM history_entries WHERE created_at < ?1")?;
        let paths = statement
            .query_map(params![cutoff], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for path in paths {
            let _ = fs::remove_file(self.root.join(path));
        }
        connection.execute(
            "DELETE FROM history_entries WHERE created_at < ?1",
            params![cutoff],
        )?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection> {
        Ok(Connection::open(self.root.join("history.sqlite3"))?)
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let key = self.master_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("invalid AES-256 history key"))?;
        let mut nonce = [0u8; NONCE_LENGTH];
        OsRng.fill_bytes(&mut nonce);
        let mut sealed = nonce.to_vec();
        sealed.extend(
            cipher
                .encrypt(Nonce::from_slice(&nonce), plaintext)
                .map_err(|_| anyhow::anyhow!("encrypt history payload"))?,
        );
        Ok(sealed)
    }

    fn master_key(&self) -> Result<[u8; 32]> {
        let key_path = self.root.join("master-key.dpapi");
        let raw = if key_path.exists() {
            unprotect_dpapi(&fs::read(&key_path)?)?
        } else {
            let mut new_key = [0u8; 32];
            OsRng.fill_bytes(&mut new_key);
            fs::write(&key_path, protect_dpapi(&new_key)?)?;
            new_key.to_vec()
        };
        raw.try_into()
            .map_err(|_| anyhow::anyhow!("invalid local history key"))
    }
}

#[cfg(windows)]
fn protect_dpapi(plaintext: &[u8]) -> Result<Vec<u8>> {
    use std::{ffi::c_void, ptr};
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{LocalFree, HLOCAL},
            Security::Cryptography::{
                CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
            },
        },
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: plaintext.len() as u32,
            pbData: plaintext.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| anyhow::anyhow!("DPAPI key protection failed: {error}"))?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut c_void));
        Ok(bytes)
    }
}

#[cfg(windows)]
fn unprotect_dpapi(ciphertext: &[u8]) -> Result<Vec<u8>> {
    use std::{ffi::c_void, ptr};
    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: ciphertext.len() as u32,
            pbData: ciphertext.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| anyhow::anyhow!("DPAPI key unprotection failed: {error}"))?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut c_void));
        Ok(bytes)
    }
}

#[cfg(not(windows))]
fn protect_dpapi(_: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("DPAPI is available only on Windows")
}
#[cfg(not(windows))]
fn unprotect_dpapi(_: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("DPAPI is available only on Windows")
}
