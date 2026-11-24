use thiserror::Error;

/// Errors returned by the Quaderno protocol layer.
#[derive(Debug, Error)]
pub enum Error {
    /// The device answered with an error document (`{"error_code": "...", "message": "..."}`).
    #[error("デバイスエラー {code}: {message}{}", device_hint(code))]
    Device { status: u16, code: String, message: String },

    #[error("通信エラー: {0}")]
    Http(#[from] reqwest::Error),

    #[error("入出力エラー: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON エラー: {0}")]
    Json(#[from] serde_json::Error),

    #[error("認証に失敗しました: {0}")]
    Auth(String),

    #[error("ペアリングに失敗しました: {0}")]
    Registration(String),

    #[error("認証情報がありません。先にペアリングしてください")]
    NotRegistered,

    #[error("不正なアドレスです: {0}")]
    InvalidAddress(String),

    #[error("USB エラー: {0}")]
    Usb(String),

    #[error("Bluetooth エラー: {0}")]
    Bluetooth(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// HTTP status of a device error, if any.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Device { status, .. } => Some(*status),
            Error::Http(e) => e.status().map(|s| s.as_u16()),
            _ => None,
        }
    }
}

/// Explanation for documented device error codes (`<HTTP status><2 digits>`).
fn device_hint(code: &str) -> &'static str {
    match code {
        "40007" => " (同じ名前の項目が既に存在します)",
        "40100" => " (認証が必要です)",
        "40304" => " (端末の Wi-Fi がオフです)",
        "40401" => " (対象が見つかりません)",
        _ => "",
    }
}

pub type Result<T> = std::result::Result<T, Error>;
