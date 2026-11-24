//! 富士通 Quaderno (と Sony DPT-RP1 系) の電子ペーパー端末のプロトコルライブラリ。
//!
//! * [`usb`] – USB 接続の端末を USB Ethernet モードに切り替える
//! * [`discovery`] – USB、Wi-Fi、Bluetooth 上で mDNS により端末を探す
//! * [`bluetooth`] – ペアリング済みの端末との Bluetooth PAN (PANU) リンク
//! * [`register`] – このクライアントを端末とペアリングする (PIN を使う)
//! * [`client`] – HTTPS 上の認証済みファイル操作

pub mod address;
pub mod bluetooth;
pub mod client;
pub mod credentials;
pub mod crypto;
pub mod discovery;
pub mod error;
mod http;
pub mod model;
pub mod register;
pub mod usb;

pub use address::DeviceAddress;
pub use client::{device_information, no_progress, DeviceClient, Progress};
pub use credentials::{CredentialStore, Credentials};
pub use discovery::{discover, DiscoveredDevice, Transport};
pub use error::{Error, Result};
pub use model::{format_date, format_size, DeviceInfo, Entry, EntryType, ROOT_FOLDER_ID};
pub use register::Registration;
