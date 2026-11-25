//! ペアリングで得たクライアントの認証情報 (クライアント ID と RSA 鍵)。

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey};
use rsa::pkcs8::{DecodePrivateKey, LineEnding};
use rsa::RsaPrivateKey;

use crate::error::{Error, Result};

const CLIENT_ID_FILE: &str = "client_id";
const PRIVATE_KEY_FILE: &str = "private_key.pem";
const CERTIFICATE_FILE: &str = "device_certificate.pem";

/// このクライアントの認証情報。
#[derive(Clone)]
pub struct Credentials {
    pub client_id: String,
    pub private_key: RsaPrivateKey,
    /// ペアリング時に端末の CA が発行した証明書 (参考情報)。
    pub certificate: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("client_id", &self.client_id).finish_non_exhaustive()
    }
}

impl Credentials {
    pub fn parse_key(pem: &str) -> Result<RsaPrivateKey> {
        RsaPrivateKey::from_pkcs1_pem(pem)
            .or_else(|_| RsaPrivateKey::from_pkcs8_pem(pem))
            .map_err(|e| Error::Auth(format!("秘密鍵を読み込めません: {e}")))
    }
}

/// ディスク上の認証情報の保存先 (既定は `~/.config/quaderno4linux`)。
#[derive(Clone, Debug)]
pub struct CredentialStore {
    dir: PathBuf,
}

impl CredentialStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_location() -> Self {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config"));
        Self::new(base.join("quaderno4linux"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 保存した認証情報を読み込む。なければ `dpt-rp1-py` が書き出したファイル (
    /// `~/.config/dpt/deviceid.dat`、`privatekey.dat`) を使う。
    pub fn load(&self) -> Result<Option<Credentials>> {
        if let Some(c) = Self::load_pair(&self.dir.join(CLIENT_ID_FILE), &self.dir.join(PRIVATE_KEY_FILE))? {
            let certificate = fs::read_to_string(self.dir.join(CERTIFICATE_FILE)).ok();
            return Ok(Some(Credentials { certificate, ..c }));
        }
        if let Some(cfg) = dirs::config_dir() {
            let dpt = cfg.join("dpt");
            if let Some(c) = Self::load_pair(&dpt.join("deviceid.dat"), &dpt.join("privatekey.dat"))? {
                log::info!("dpt-rp1-py の認証情報を使用します: {}", dpt.display());
                return Ok(Some(c));
            }
        }
        Ok(None)
    }

    fn load_pair(id_path: &Path, key_path: &Path) -> Result<Option<Credentials>> {
        if !id_path.exists() || !key_path.exists() {
            return Ok(None);
        }
        let client_id = fs::read_to_string(id_path)?.trim().to_string();
        let private_key = Credentials::parse_key(&fs::read_to_string(key_path)?)?;
        Ok(Some(Credentials { client_id, private_key, certificate: None }))
    }

    pub fn save(&self, creds: &Credentials) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let pem = creds
            .private_key
            .to_pkcs1_pem(LineEnding::LF)
            .map_err(|e| Error::Other(format!("秘密鍵を保存できません: {e}")))?;
        write_private(&self.dir.join(CLIENT_ID_FILE), creds.client_id.as_bytes())?;
        write_private(&self.dir.join(PRIVATE_KEY_FILE), pem.as_bytes())?;
        if let Some(cert) = &creds.certificate {
            write_private(&self.dir.join(CERTIFICATE_FILE), cert.as_bytes())?;
        }
        Ok(())
    }

    pub fn delete(&self) -> Result<()> {
        for f in [CLIENT_ID_FILE, PRIVATE_KEY_FILE, CERTIFICATE_FILE] {
            let p = self.dir.join(f);
            if p.exists() {
                fs::remove_file(p)?;
            }
        }
        Ok(())
    }
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut f = fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        let key = RsaPrivateKey::new(&mut rand::thread_rng(), 1024).unwrap();
        let creds = Credentials { client_id: "abc".into(), private_key: key.clone(), certificate: Some("cert".into()) };
        store.save(&creds).unwrap();
        let loaded = CredentialStore::load_pair(&dir.path().join(CLIENT_ID_FILE), &dir.path().join(PRIVATE_KEY_FILE))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.client_id, "abc");
        assert_eq!(loaded.private_key, key);
    }
}
