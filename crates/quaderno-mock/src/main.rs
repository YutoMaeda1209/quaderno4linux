//! 実機なしでデスクトップアプリを開発できるよう、シミュレートした Quaderno を動かす。
//!
//! ```sh
//! cargo run -p quaderno-mock -- 127.0.0.1 12345678
//! ```
//! そのあとアプリで `127.0.0.1` に接続し、PIN でペアリングする。

use std::net::IpAddr;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let ip: IpAddr = args.next().as_deref().unwrap_or("127.0.0.1").parse()?;
    let pin = args.next().unwrap_or_else(|| "12345678".into());

    let device = quaderno_mock::MockDevice::start(ip, &pin)?;
    let notes = device.add_folder("root", "Notes");
    device.add_folder("root", "論文");
    device.add_document("root", "manual.pdf", b"%PDF-1.4\n% mock manual\n");
    device.add_document(&notes, "meeting-2026-11.pdf", b"%PDF-1.4\n% meeting notes\n");
    device.add_document(&notes, "ideas.pdf", &vec![b'x'; 1_500_000]);

    println!("[mock] Quaderno mock device listening on {ip} (HTTP 8080 / HTTPS 8443), PIN {pin}");
    loop {
        std::thread::park();
    }
}
