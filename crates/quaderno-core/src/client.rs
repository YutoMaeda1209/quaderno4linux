//! HTTPS API (ポート 8443) 用の認証済みクライアント。

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use reqwest::Method;
use reqwest::blocking::{Client, RequestBuilder, Response, multipart};
use rsa::Pkcs1v15Sign;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::address::DeviceAddress;
use crate::credentials::Credentials;
use crate::error::{Error, Result};
use crate::http::{build_client, check, http_url, https_url};
use crate::model::{
    BatteryStatus, DeviceInfo, DeviceSettings, Entry, EntryList, ROOT_FOLDER_ID, StorageStatus, WifiConfig,
    WifiNetwork, WifiNetworkList, WifiStatus, format_utc,
};

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
        Ok(value_string(self.get_json("/system/status/firmware_version")?))
    }

    // ----- 設定 ---------------------------------------------------------

    /// `/system/configs/{key}` の値を読む。
    pub fn config(&self, key: &str) -> Result<String> {
        Ok(value_string(self.get_json(&config_api_path(key))?))
    }

    /// `/system/configs/{key}` に値を書き込む。
    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        self.send_json(Method::PUT, &config_api_path(key), json!({ "value": value }))?;
        Ok(())
    }

    pub fn settings(&self) -> Result<DeviceSettings> {
        let mut settings = DeviceSettings::default();
        for (key, value) in DeviceSettings::KEYS.into_iter().zip(settings.values_mut()) {
            *value = self.config(key)?;
        }
        Ok(settings)
    }

    /// `current` から変わった設定だけを書き込む。
    pub fn update_settings(&self, current: &DeviceSettings, new: &DeviceSettings) -> Result<()> {
        for (key, value) in current.changes(new) {
            self.set_config(key, value)?;
        }
        Ok(())
    }

    /// 端末の時計を `time` に合わせる。
    pub fn set_datetime(&self, time: SystemTime) -> Result<()> {
        self.set_config("datetime", &format_utc(time))
    }

    // ----- Wi-Fi --------------------------------------------------------

    pub fn wifi_status(&self) -> Result<WifiStatus> {
        // Wi-Fi がオフの間、端末は Wi-Fi の API に `403 40304` を返す。
        if self.config("wifi")? != "on" {
            return Ok(WifiStatus::default());
        }
        let state: WifiNetwork = self.get_json("/system/status/wifi_state")?;
        let current = matches!(state.state.as_deref(), Some("connected" | "connecting")).then_some(state);
        let saved = self.get_json::<WifiNetworkList>("/system/configs/wifi_accesspoints")?.aplist;
        Ok(WifiStatus { enabled: true, current, saved })
    }

    pub fn set_wifi_enabled(&self, enabled: bool) -> Result<()> {
        self.set_config("wifi", if enabled { "on" } else { "off" })
    }

    /// 周囲のネットワークを探す。
    pub fn scan_wifi(&self) -> Result<Vec<WifiNetwork>> {
        let resp = self.send(Method::POST, "/system/controls/wifi_accesspoints/scan", |r| {
            Ok(r.timeout(Duration::from_secs(60)))
        })?;
        Ok(resp.json::<WifiNetworkList>()?.aplist)
    }

    /// ネットワークを登録する。端末は範囲内であれば登録したネットワークに接続し直す。
    pub fn add_wifi(&self, config: &WifiConfig) -> Result<()> {
        let body = config.to_json();
        // 端末と Wi-Fi で接続していると、端末がネットワークを切り替えるため応答が返らない。
        self.send(Method::PUT, "/system/controls/wifi_accesspoints/register", |r| {
            Ok(r.json(&body).timeout(Duration::from_secs(20)))
        })?;
        Ok(())
    }

    /// 保存済みのネットワークを削除する。パスの SSID は Base64 ではなく、そのままの文字列を使う。
    pub fn delete_wifi(&self, network: &WifiNetwork) -> Result<()> {
        let path = format!(
            "/system/configs/wifi_accesspoints/{}/{}",
            urlencoding::encode(&network.ssid),
            urlencoding::encode(&network.security)
        );
        self.send(Method::DELETE, &path, |r| Ok(r.timeout(Duration::from_secs(30))))?;
        Ok(())
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
        self.send(Method::DELETE, &entry_api_path(entry), |r| Ok(r.timeout(Duration::from_secs(120))))?;
        Ok(())
    }

    /// 項目の名前をその場で変更する。
    pub fn rename(&self, entry: &Entry, new_name: &str) -> Result<()> {
        self.send_json(Method::PUT, &entry_api_path(entry), json!({ name_key(entry): new_name }))?;
        Ok(())
    }

    /// 項目を (名前はそのままで) `dest_folder_id` に移動する。
    pub fn move_to(&self, entry: &Entry, dest_folder_id: &str) -> Result<()> {
        let body = json!({ name_key(entry): entry.entry_name, "parent_folder_id": dest_folder_id });
        self.send_json(Method::PUT, &entry_api_path(entry), body)?;
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
            self.send_json(Method::POST, &format!("{}/copy", entry_api_path(entry)), body)?;
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
        let document_path = document_api_path(&created.document_id);

        let result = self.send(Method::PUT, &format!("{document_path}/file"), |r| {
            let reader = ProgressReader { inner: File::open(local)?, done: 0, total, progress: progress.clone() };
            let part = multipart::Part::reader_with_length(reader, total)
                .file_name(name.clone())
                .mime_str("application/pdf")?;
            Ok(r.multipart(multipart::Form::new().part("file", part)))
        });
        if let Err(e) = result {
            // 再試行で競合しないよう、空のプレースホルダー文書を削除する。
            let _ = self.send(Method::DELETE, &document_path, Ok);
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
        let mut resp = self.send(Method::GET, &format!("{}/file", document_api_path(document_id)), Ok)?;
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

fn config_api_path(key: &str) -> String {
    format!("/system/configs/{}", urlencoding::encode(key))
}

/// `{"value": ...}` の形の応答から値を取り出す。文字列でなければ JSON のまま返す。
fn value_string(v: Value) -> String {
    match v.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => v.to_string(),
    }
}

fn document_api_path(document_id: &str) -> String {
    format!("/documents/{}", urlencoding::encode(document_id))
}

/// 項目の種類に応じた API のパス (`/folders/{id}` または `/documents/{id}`)。
fn entry_api_path(entry: &Entry) -> String {
    if entry.is_folder() {
        format!("/folders/{}", urlencoding::encode(&entry.entry_id))
    } else {
        document_api_path(&entry.entry_id)
    }
}

/// 名前の変更や移動のリクエストで、項目の名前を渡すキー。
fn name_key(entry: &Entry) -> &'static str {
    if entry.is_folder() { "folder_name" } else { "file_name" }
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
