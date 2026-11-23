use thiserror::Error;

/// Errors returned by the Quaderno protocol layer.
#[derive(Debug, Error)]
pub enum Error {
    /// The device answered with an error document (`{"error_code": "...", "message": "..."}`).
    #[error("デバイスエラー {code}: {message}")]
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

pub type Result<T> = std::result::Result<T, Error>;
