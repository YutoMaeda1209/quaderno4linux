//! HTTPS API (ポート 8443) 用の認証済みクライアント。

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use reqwest::blocking::{multipart, Client, RequestBuilder, Response};
use reqwest::Method;
use rsa::Pkcs1v15Sign;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::address::DeviceAddress;
use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::http::{build_client, check, http_url, https_url};
use crate::model::{BatteryStatus, DeviceInfo, Entry, EntryList, StorageStatus, ROOT_FOLDER_ID};

/// 認証不要の端末情報 (`:8080/register/information`) を読み取る。
pub fn device_information(addr: &DeviceAddress, timeout: Duration) -> Result<DeviceInfo> {
    let http = build_client(addr, Some(timeout))?;
    Ok(check(http.get(http_url("/register/information")).send()?)?.json()?)
}

/// 進捗コールバック: 現在のファイルの `(bytes_done, bytes_total)`。
pub type Progress = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// 更新を無視する進捗コールバック。
pub fn no_progress() -> Progress {
    Arc::new(|_, _| {})
}

/// ペアリング済みの端末への接続。
pub struct DeviceClient {
    addr: DeviceAddress,
    http: Client,
    credentials: Credentials,
    cookie: Mutex<Option<String>>,
}

impl DeviceClient {
    /// 接続して認証する。
    pub fn connect(addr: DeviceAddress, credentials: Credentials) -> Result<Self> {
        let http = build_client(&addr, None)?;
        let client = Self { addr, http, credentials, cookie: Mutex::new(None) };
        client.authenticate()?;
        Ok(client)
    }

    pub fn address(&self) -> &DeviceAddress {
        &self.addr
    }

    fn authenticate(&self) -> Result<()> {
        #[derive(Deserialize)]
        struct Nonce {
            nonce: String,
        }
        let id = &self.credentials.client_id;
        let resp = self
            .http
            .get(https_url(&format!("/auth/nonce/{}", urlencoding::encode(id))))
            .timeout(Duration::from_secs(15))
            .send()?;
        let nonce: Nonce = check(resp)
            .map_err(|e| match e.status() {
                Some(404) | Some(401) | Some(403) => {
                    Error::Auth("このクライアントは端末に登録されていません。ペアリングしてください".into())
                }
                _ => e,
            })?
            .json()?;

        let digest = Sha256::digest(nonce.nonce.as_bytes());
        let signature = self
            .credentials
            .private_key
            .sign(Pkcs1v15Sign::new::<Sha256>(), &digest)
            .map_err(|e| Error::Auth(e.to_string()))?;
        let body = json!({ "client_id": id, "nonce_signed": B64.encode(signature) });
        let resp = check(self.http.put(https_url("/auth")).json(&body).timeout(Duration::from_secs(15)).send()?)?;

        let credentials = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .filter_map(|kv| kv.trim().split_once('='))
            .find(|(k, _)| *k == "Credentials")
            .map(|(_, v)| v.to_string())
            .ok_or_else(|| Error::Auth("Credentials クッキーが返されませんでした".into()))?;
        *self.cookie.lock().unwrap() = Some(credentials);
        Ok(())
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut req = self.http.request(method, https_url(path));
        if let Some(c) = self.cookie.lock().unwrap().as_ref() {
            req = req.header(reqwest::header::COOKIE, format!("Credentials={c}"));
        }
        req
    }

    /// リクエストを送り、セッションが切れていれば 1 回だけ認証し直す。
    fn send(
        &self,
        method: Method,
        path: &str,
        build: impl Fn(RequestBuilder) -> Result<RequestBuilder>,
    ) -> Result<Response> {
        let resp = build(self.request(method.clone(), path))?.send()?;
        if resp.status().as_u16() == 401 {
            self.authenticate()?;
            return check(build(self.request(method, path))?.send()?);
        }
        check(resp)
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        Ok(self.send(Method::GET, path, |r| Ok(r.timeout(Duration::from_secs(30))))?.json()?)
    }

    fn send_json(&self, method: Method, path: &str, body: Value) -> Result<Response> {
        self.send(method, path, |r| Ok(r.json(&body).timeout(Duration::from_secs(60))))
    }

    /// `GET /ping`: セッションが有効かを確かめる軽い確認。
    pub fn ping(&self) -> Result<()> {
        self.send(Method::GET, "/ping", |r| Ok(r.timeout(Duration::from_secs(10))))?;
        Ok(())
    }

    // ----- 状態 ---------------------------------------------------------

    pub fn battery(&self) -> Result<BatteryStatus> {
        self.get_json("/system/status/battery")
    }

    pub fn storage(&self) -> Result<StorageStatus> {
        self.get_json("/system/status/storage")
    }

    pub fn firmware_version(&self) -> Result<String> {
        let v: Value = self.get_json("/system/status/firmware_version")?;
        Ok(v.get("value").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| v.to_string()))
    }

    // ----- 閲覧 ---------------------------------------------------------

    /// フォルダ内の項目を列挙する (最上位は `root`)。
    pub fn list_folder(&self, folder_id: &str) -> Result<Vec<Entry>> {
        let list: EntryList = self.get_json(&format!("/folders/{}/entries", urlencoding::encode(folder_id)))?;
        Ok(list.entry_list)
    }

    /// `Document/Notes/a.pdf` のような端末上のパスを解決する。
    pub fn resolve_path(&self, path: &str) -> Result<Entry> {
        self.get_json(&format!("/resolve/entry/path/{}", urlencoding::encode(path)))
    }

    /// `folder_id` 以下のすべての項目を再帰的に列挙する (フォルダはその中身より先)。
    pub fn walk(&self, folder_id: &str) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        for entry in self.list_folder(folder_id)? {
            let is_folder = entry.is_folder();
            let id = entry.entry_id.clone();
            out.push(entry);
            if is_folder {
                out.extend(self.walk(&id)?);
            }
        }
        Ok(out)
    }

    // ----- フォルダ -----------------------------------------------------

    /// フォルダを作成し、その ID を返す。
    pub fn create_folder(&self, parent_id: &str, name: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Created {
            folder_id: String,
        }
        let body = json!({ "folder_name": name, "parent_folder_id": parent_id });
        let c: Created = self.send_json(Method::POST, "/folders2", body)?.json()?;
        Ok(c.folder_id)
    }

    // ----- 共通の操作 ---------------------------------------------------

    /// 文書、またはフォルダ (再帰的に) を削除する。
    pub fn delete(&self, entry: &Entry) -> Result<()> {
        let path = if entry.is_folder() {
            format!("/folders/{}", urlencoding::encode(&entry.entry_id))
        } else {
            format!("/documents/{}", urlencoding::encode(&entry.entry_id))
        };
        self.send(Method::DELETE, &path, |r| Ok(r.timeout(Duration::from_secs(120))))?;
        Ok(())
    }

    /// 項目の名前をその場で変更する。
    pub fn rename(&self, entry: &Entry, new_name: &str) -> Result<()> {
        if entry.is_folder() {
            let body = json!({ "folder_name": new_name });
            self.send_json(Method::PUT, &format!("/folders/{}", urlencoding::encode(&entry.entry_id)), body)?;
        } else {
            let body = json!({ "file_name": new_name });
            self.send_json(Method::PUT, &format!("/documents/{}", urlencoding::encode(&entry.entry_id)), body)?;
        }
        Ok(())
    }

    /// 項目を (名前はそのままで) `dest_folder_id` に移動する。
    pub fn move_to(&self, entry: &Entry, dest_folder_id: &str) -> Result<()> {
        if entry.is_folder() {
            let body = json!({ "folder_name": entry.entry_name, "parent_folder_id": dest_folder_id });
            self.send_json(Method::PUT, &format!("/folders/{}", urlencoding::encode(&entry.entry_id)), body)?;
        } else {
            let body = json!({ "file_name": entry.entry_name, "parent_folder_id": dest_folder_id });
            self.send_json(Method::PUT, &format!("/documents/{}", urlencoding::encode(&entry.entry_id)), body)?;
        }
        Ok(())
    }

    /// 項目を `dest_folder_id` にコピーする。フォルダは再帰的にコピーする。`new_name` を指定すると
    /// コピーの名前を変える。
    pub fn copy_to(&self, entry: &Entry, dest_folder_id: &str, new_name: Option<&str>) -> Result<()> {
        let name = new_name.unwrap_or(&entry.entry_name);
        if entry.is_folder() {
            let new_id = self.create_folder(dest_folder_id, name)?;
            for child in self.list_folder(&entry.entry_id)? {
                self.copy_to(&child, &new_id, None)?;
            }
        } else {
            let body = json!({ "parent_folder_id": dest_folder_id, "file_name": name });
            self.send_json(Method::POST, &format!("/documents/{}/copy", urlencoding::encode(&entry.entry_id)), body)?;
        }
        Ok(())
    }

    // ----- 転送 ---------------------------------------------------------

    /// ローカルのファイルを `parent_id` にアップロードし、新しい文書の ID を返す。
    pub fn upload_file(&self, local: &Path, parent_id: &str, progress: &Progress) -> Result<String> {
        #[derive(Deserialize)]
        struct Created {
            document_id: String,
        }
        let name = local
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::Other(format!("ファイル名が不正です: {}", local.display())))?
            .to_string();
        let total = std::fs::metadata(local)?.len();

        let body = json!({ "file_name": name, "parent_folder_id": parent_id, "document_source": "" });
        let created: Created = self.send_json(Method::POST, "/documents2", body)?.json()?;
        let path = format!("/documents/{}/file", urlencoding::encode(&created.document_id));

        let result = self.send(Method::PUT, &path, |r| {
            let reader = ProgressReader { inner: File::open(local)?, done: 0, total, progress: progress.clone() };
            let part = multipart::Part::reader_with_length(reader, total)
                .file_name(name.clone())
                .mime_str("application/pdf")?;
            Ok(r.multipart(multipart::Form::new().part("file", part)))
        });
        if let Err(e) = result {
            // 再試行で競合しないよう、空のプレースホルダー文書を削除する。
            let _ = self.send(Method::DELETE, &format!("/documents/{}", created.document_id), Ok);
            return Err(e);
        }
        Ok(created.document_id)
    }

    /// ローカルのディレクトリツリーを `parent_id` にアップロードする。
    pub fn upload_dir(&self, local: &Path, parent_id: &str, progress: &Progress) -> Result<()> {
        let name = local
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::Other(format!("フォルダ名が不正です: {}", local.display())))?;
        let folder_id = self.create_folder(parent_id, name)?;
        let mut children: Vec<PathBuf> = std::fs::read_dir(local)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        children.sort();
        for child in children {
            if child.is_dir() {
                self.upload_dir(&child, &folder_id, progress)?;
            } else if child.is_file() {
                self.upload_file(&child, &folder_id, progress)?;
            }
        }
        Ok(())
    }

    /// 文書を `dest` (ファイルパス) にダウンロードする。
    pub fn download_document(&self, document_id: &str, dest: &Path, progress: &Progress) -> Result<()> {
        let path = format!("/documents/{}/file", urlencoding::encode(document_id));
        let mut resp = self.send(Method::GET, &path, Ok)?;
        let total = resp.content_length().unwrap_or(0);
        let tmp = dest.with_extension("part");
        let mut out = File::create(&tmp)?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            progress(done, total);
        }
        out.flush()?;
        drop(out);
        std::fs::rename(&tmp, dest)?;
        Ok(())
    }

    /// 項目 (文書またはフォルダ全体) をローカルのディレクトリ `dest_dir` にダウンロードする。
    pub fn download_entry(&self, entry: &Entry, dest_dir: &Path, progress: &Progress) -> Result<PathBuf> {
        let target = dest_dir.join(sanitize(&entry.entry_name));
        if entry.is_folder() {
            std::fs::create_dir_all(&target)?;
            for child in self.list_folder(&entry.entry_id)? {
                self.download_entry(&child, &target, progress)?;
            }
        } else {
            self.download_document(&entry.entry_id, &target, progress)?;
        }
        Ok(target)
    }

    /// ルートフォルダを列挙する (簡便のため)。
    pub fn list_root(&self) -> Result<Vec<Entry>> {
        self.list_folder(ROOT_FOLDER_ID)
    }
}

/// ローカルのファイル名に使えない文字を置き換える。
fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c == '/' || c == '\0' { '_' } else { c }).collect();
    match s.as_str() {
        "" | "." | ".." => "_".into(),
        _ => s,
    }
}

struct ProgressReader<R> {
    inner: R,
    done: u64,
    total: u64,
    progress: Progress,
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        (self.progress)(self.done, self.total);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize("a/b.pdf"), "a_b.pdf");
        assert_eq!(sanitize(".."), "_");
        assert_eq!(sanitize("ノート.pdf"), "ノート.pdf");
    }
}
