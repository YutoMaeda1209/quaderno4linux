//! Protocol library for the Fujitsu Quaderno (and Sony DPT-RP1 family) e-paper devices.
//!
//! * [`usb`] – switch a USB-attached device to USB Ethernet mode
//! * [`discovery`] – find devices with mDNS over USB, Wi-Fi and Bluetooth
//! * [`bluetooth`] – Bluetooth PAN (PANU) link to a paired device
//! * [`register`] – pair this client with a device (PIN based)
//! * [`client`] – authenticated file operations over HTTPS

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
