//! 共通の HTTP 処理。

use std::time::Duration;

use reqwest::blocking::{Client, Response};
use serde::Deserialize;

use crate::address::{DeviceAddress, DEVICE_HOST, HTTPS_PORT, HTTP_PORT};
use crate::error::{Error, Result};

/// プレースホルダーのホスト名を通して `addr` と通信するクライアントを作る。端末は自己署名証明書を使
/// うので、証明書の検証はしない。
pub(crate) fn build_client(addr: &DeviceAddress, timeout: Option<Duration>) -> Result<Client> {
    let mut builder = Client::builder()
        // 端末は常にローカルリンクか LAN 上にあるので、システムのプロキシは使わない。
        .no_proxy()
        .resolve(DEVICE_HOST, addr.socket_addr(HTTPS_PORT))
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .connect_timeout(Duration::from_secs(5))
        .user_agent(concat!("quaderno4linux/", env!("CARGO_PKG_VERSION")));
    builder = builder.timeout(timeout);
    Ok(builder.build()?)
}

pub(crate) fn http_url(path: &str) -> String {
    format!("http://{DEVICE_HOST}:{HTTP_PORT}{path}")
}

pub(crate) fn https_url(path: &str) -> String {
    format!("https://{DEVICE_HOST}:{HTTPS_PORT}{path}")
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error_code: Option<serde_json::Value>,
    #[serde(default)]
    message: Option<String>,
}

/// 成功以外の応答を [`Error::Device`] に変換する。
pub(crate) fn check(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let text = resp.text().unwrap_or_default();
    let (code, message) = match serde_json::from_str::<ErrorBody>(&text) {
        Ok(body) => (
            body.error_code
                .map(|c| c.as_str().map(str::to_owned).unwrap_or_else(|| c.to_string()))
                .unwrap_or_else(|| status.as_u16().to_string()),
            body.message.unwrap_or_else(|| status.to_string()),
        ),
        Err(_) => (status.as_u16().to_string(), if text.is_empty() { status.to_string() } else { text }),
    };
    Err(Error::Device { status: status.as_u16(), code, message })
}
