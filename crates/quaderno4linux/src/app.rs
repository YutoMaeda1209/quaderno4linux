//! メインウィンドウ: 接続の管理とレイアウト。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::group_box::GroupBox;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::table::TableState;
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, TitleBar, h_flex, v_flex};
use gpui_kit::{ClickEvent, Context, Entity, FocusHandle, Focusable, StyleRefinement, Window, div, prelude::*, px};
use quaderno_core::bluetooth::{self, BluetoothDevice};
use quaderno_core::usb::UsbNetMode;
use quaderno_core::{
    CredentialStore, Credentials, DeviceAddress, DeviceClient, DiscoveredDevice, Entry, Error, ROOT_FOLDER_ID,
    Registration, Transport, format_size, usb,
};

use crate::explorer::EntryTable;
use crate::tasks::TaskProgress;

pub const CONTEXT: &str = "QuadernoApp";

pub mod actions {
    gpui_kit::actions!(
        quaderno,
        [
            Refresh,
            GoUp,
            OpenSelected,
            DeleteSelected,
            RenameSelected,
            CopySelected,
            CutSelected,
            Paste,
            SelectAll,
            SelectPrev,
            SelectNext,
            ClearSelection,
            NewFolder,
            Upload,
            Download,
            Quit
        ]
    );
}

/// 端末との接続の状態。
pub enum Connection {
    Disconnected,
    Connected { client: Arc<DeviceClient>, name: String, status: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

impl ClipboardMode {
    pub fn label(self) -> &'static str {
        match self {
            ClipboardMode::Copy => "コピー",
            ClipboardMode::Cut => "切り取り",
        }
    }
}

pub struct Clipboard {
    pub mode: ClipboardMode,
    pub entries: Vec<Entry>,
    pub source_folder: String,
}

pub struct QuadernoApp {
    pub focus_handle: FocusHandle,
    pub store: CredentialStore,
    pub credentials: Option<Credentials>,
    pub devices: Vec<DiscoveredDevice>,
    pub usb_state: usb::UsbState,
    /// NAP サービスを提供するペアリング済みの Bluetooth 端末。
    pub bt_devices: Vec<BluetoothDevice>,
    /// このアプリが PAN リンクを確立した Bluetooth 端末。
    pub bt_link: Option<BluetoothDevice>,
    pub usb_mode: UsbNetMode,
    pub connection: Connection,

    /// パンくずリスト: ルートから始まる `(フォルダ ID, フォルダ名)`。
    pub path: Vec<(String, String)>,
    pub entries: Vec<Entry>,
    pub selection: HashSet<String>,
    pub anchor: Option<usize>,
    pub cursor: Option<usize>,
    pub clipboard: Option<Clipboard>,
    pub table: Entity<TableState<EntryTable>>,

    pub busy: Option<TaskProgress>,
    pub log: Vec<String>,
    /// 次の通知の ID。通知を 1 つずつ消せるようにするため。
    pub next_toast_id: u64,
}

impl QuadernoApp {
    pub fn new(initial_address: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = CredentialStore::default_location();
        let mut log = Vec::new();
        let credentials = match store.load() {
            Ok(c) => c,
            Err(e) => {
                log.push(format!("認証情報を読み込めません: {e}"));
                None
            }
        };
        let focus_handle = cx.focus_handle();
        let app = cx.weak_entity();
        let table = cx.new(|cx| {
            TableState::new(EntryTable::new(app), window, cx)
                .row_selectable(false)
                .col_selectable(false)
                .col_movable(false)
                .sortable(false)
        });
        // キーボード操作 (選択、ショートカット) はアプリ自身のフォーカスハンドルで扱う。
        cx.on_focus(&table.focus_handle(cx), window, |this, window, cx| window.focus(&this.focus_handle, cx)).detach();
        let mut this = Self {
            focus_handle,
            store,
            credentials,
            devices: Vec::new(),
            usb_state: usb::scan(),
            bt_devices: Vec::new(),
            bt_link: None,
            usb_mode: UsbNetMode::Rndis,
            connection: Connection::Disconnected,
            path: vec![(ROOT_FOLDER_ID.into(), "Document".into())],
            entries: Vec::new(),
            selection: HashSet::new(),
            anchor: None,
            cursor: None,
            clipboard: None,
            table,
            busy: None,
            log,
            next_toast_id: 0,
        };
        this.push_log(match &this.credentials {
            Some(c) => format!("クライアント ID: {}", c.client_id),
            None => "未ペアリングです。端末に接続するとペアリングを開始します".into(),
        });
        // ダイアログと通知にはウィンドウの `Root` が必要で、これはこのビューを作ったあとにしか存在
        // しない。
        cx.defer_in(window, move |this, window, cx| match initial_address {
            Some(addr) => match DeviceAddress::parse(&addr) {
                Ok(addr) => this.connect_to(addr, window, cx),
                Err(e) => this.notify_error("アドレスが正しくありません", &e, window, cx),
            },
            None => this.discover(window, cx),
        });
        this
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        log::info!("{line}");
        self.record_log(line);
    }

    /// アプリ内のログ (ステータスバーに表示) に 1 行追加する。ログファイルには書き込まない。
    pub fn record_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    pub fn client(&self) -> Option<Arc<DeviceClient>> {
        match &self.connection {
            Connection::Connected { client, .. } => Some(client.clone()),
            Connection::Disconnected => None,
        }
    }

    pub fn current_folder(&self) -> String {
        self.path.last().map(|(id, _)| id.clone()).unwrap_or_else(|| ROOT_FOLDER_ID.into())
    }

    // ----- 検索と接続 --------------------------------------------------

    /// USB とネットワーク上の端末を探す。
    pub fn discover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_task(
            "端末を検索しています",
            window,
            cx,
            |_| {
                // Bluetooth は必須ではない。BlueZ がなければ一覧が空になるだけ。
                let bt = bluetooth::paired_devices().unwrap_or_else(|e| {
                    log::info!("{e}");
                    Vec::new()
                });
                Ok((usb::scan(), bt, quaderno_core::discover(Duration::from_millis(2500))))
            },
            |this, result, window, cx| match result {
                Ok((usb_state, bt_devices, devices)) => {
                    this.usb_state = usb_state;
                    this.bt_devices = bt_devices;
                    this.devices = devices;
                    if this.devices.is_empty() {
                        this.push_log("端末が見つかりませんでした");
                    } else {
                        this.push_log(format!("{} 台の端末が見つかりました", this.devices.len()));
                    }
                }
                Err(e) => this.notify_error("検索に失敗しました", &e, window, cx),
            },
        );
    }

    /// USB 接続の端末を USB Ethernet モードに切り替え、見つけて接続する。
    pub fn connect_usb(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.usb_mode;
        self.run_task(
            "USB で接続しています",
            window,
            cx,
            move |progress| {
                progress.set_detail("USB Ethernet モードに切り替えています");
                let ifaces = usb::ensure_ethernet(mode, Duration::from_secs(15))?;
                if let Some(down) = ifaces.iter().find(|i| !i.up) {
                    anyhow::bail!(
                        "インターフェース {} が無効になっています。NetworkManager で有効にするか `sudo ip link set {} up` を実行してください",
                        down.name,
                        down.name
                    );
                }
                progress.set_detail("端末を探しています");
                // インターフェースが有効になってから、リンクローカルアドレスが使えるまで少し (DAD)
                // かかる。
                for _ in 0..6 {
                    let found: Vec<_> =
                        quaderno_core::discover(Duration::from_millis(1500)).into_iter().filter(|d| d.transport == Transport::Usb).collect();
                    if !found.is_empty() {
                        return Ok(found);
                    }
                }
                anyhow::bail!(
                    "USB インターフェース ({}) 上で端末が応答しません",
                    ifaces.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ")
                )
            },
            |this, result, window, cx| match result {
                Ok(found) => {
                    let addr = found[0].address;
                    for d in found {
                        if !this.devices.iter().any(|x| x.address == d.address) {
                            this.devices.insert(0, d);
                        }
                    }
                    this.usb_state = usb::scan();
                    this.connect_to(addr, window, cx);
                }
                Err(e) => {
                    this.usb_state = usb::scan();
                    if matches!(e.downcast_ref::<Error>(), Some(Error::UsbPermission(_))) {
                        this.offer_udev_rule_install(e, window, cx);
                    } else {
                        this.notify_error("USB 接続に失敗しました", &e, window, cx);
                    }
                }
            },
        );
    }

    /// ユーザーがシリアルポートを開けるようにする udev ルールのインストールを提案し、USB 接続をやり
    /// 直す。
    fn offer_udev_rule_install(&mut self, e: anyhow::Error, window: &mut Window, cx: &mut Context<Self>) {
        if usb::udev_rule_installed() {
            // ルールはローカルの席でログインしているユーザーにだけ適用される。
            self.notify_error(
                "USB 接続に失敗しました。端末を挿し直してください",
                format!("{e:#} (udev ルールはインストール済み。挿し直しても解決しない場合は dialout グループに参加してください)"),
                window,
                cx,
            );
            return;
        }
        self.push_log(format!("{e:#}"));
        self.confirm(
            "USB 接続の設定",
            "USB で接続するには、端末のシリアルポートへのアクセスを許可する udev ルール (/etc/udev/rules.d/70-quaderno.rules) をインストールする必要があります。管理者パスワードの入力を求められます。この設定は初回のみ必要です。",
            "インストール",
            false,
            window,
            cx,
            |this, window, cx| {
                this.run_task(
                    "udev ルールをインストールしています",
                    window,
                    cx,
                    |_| Ok(usb::install_udev_rule()?),
                    |this, result, window, cx| match result {
                        Ok(()) => {
                            this.notify_success("udev ルールをインストールしました", window, cx);
                            this.connect_usb(window, cx);
                        }
                        Err(e) => this.notify_error("udev ルールをインストールできませんでした", &e, window, cx),
                    },
                );
            },
        );
    }

    pub fn connect_to(&mut self, addr: DeviceAddress, window: &mut Window, cx: &mut Context<Self>) {
        let Some(credentials) = self.credentials.clone() else {
            self.confirm(
                "ペアリング",
                "このアプリはまだ端末に登録されていません。ペアリングを開始しますか？\n端末の画面に表示される PIN を入力します。",
                "ペアリング",
                false,
                window,
                cx,
                move |this, window, cx| this.start_pairing(addr, window, cx),
            );
            return;
        };
        self.run_task(
            format!("{addr} に接続しています"),
            window,
            cx,
            move |_| {
                let info = quaderno_core::device_information(&addr, Duration::from_secs(5)).unwrap_or_default();
                let client = DeviceClient::connect(addr, credentials)?;
                let name = info.display_name();
                let status = device_status(&client);
                let entries = client.list_root()?;
                Ok((Arc::new(client), name, status, entries))
            },
            move |this, result, window, cx| match result {
                Ok((client, name, status, entries)) => {
                    this.push_log(format!("{name} に接続しました ({addr})"));
                    this.watch_connection(client.clone(), window, cx);
                    this.connection = Connection::Connected { client, name, status };
                    this.path.truncate(1);
                    this.set_entries(entries);
                    window.focus(&this.focus_handle, cx);
                }
                Err(e) => {
                    if matches!(e.downcast_ref::<Error>(), Some(Error::Auth(_))) {
                        let msg = format!("{e}\n端末とペアリングし直しますか？");
                        this.confirm(
                            "認証に失敗しました",
                            msg,
                            "ペアリング",
                            false,
                            window,
                            cx,
                            move |this, window, cx| this.start_pairing(addr, window, cx),
                        );
                    } else {
                        this.notify_error("接続に失敗しました", &e, window, cx);
                    }
                }
            },
        );
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(dev) = self.bt_link.take() {
            cx.background_executor().spawn(async move { bluetooth::disconnect_pan(&dev) }).detach();
        }
        self.connection = Connection::Disconnected;
        self.entries.clear();
        self.selection.clear();
        self.path.truncate(1);
        self.push_log("切断しました");
        cx.notify();
    }

    /// `client` が有効な接続である間は端末をポーリングし、端末がいなくなったら (ケーブルを抜いた、
    /// Wi-Fi や Bluetooth を切ったなど) 接続を切る。
    fn watch_connection(&mut self, client: Arc<DeviceClient>, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let mut failures = 0;
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let Ok((active, busy)) = this.update(cx, |this, _| {
                    (this.client().is_some_and(|c| Arc::ptr_eq(&c, &client)), this.busy.is_some())
                }) else {
                    return;
                };
                if !active {
                    return;
                }
                // 実行中のタスクは自分で端末と通信し、エラーも自分で報告する。
                if busy {
                    continue;
                }
                let c = client.clone();
                let alive = cx.background_executor().spawn(async move { device_alive(&c) }).await;
                failures = if alive { 0 } else { failures + 1 };
                if failures >= 2 {
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.client().is_some_and(|c| Arc::ptr_eq(&c, &client)) {
                            this.disconnect(cx);
                            this.usb_state = usb::scan();
                            this.notify_warning("端末との接続が切れました", window, cx);
                        }
                    });
                    return;
                }
            }
        })
        .detach();
    }

    /// ペアリング済みの端末との Bluetooth PAN リンクを確立し、端末を見つけて接続する。
    pub fn connect_bluetooth(&mut self, dev: BluetoothDevice, window: &mut Window, cx: &mut Context<Self>) {
        self.run_task(
            format!("Bluetooth ({}) で接続しています", dev.name),
            window,
            cx,
            {
                let dev = dev.clone();
                move |progress| {
                    progress.set_detail("PAN 接続を確立しています (数十秒かかることがあります)");
                    let iface = bluetooth::connect_pan(&dev)?;
                    progress.set_detail(format!("{iface} 上で端末を探しています"));
                    match bluetooth::locate_device(&iface) {
                        Ok(addr) => Ok(addr),
                        Err(e) => {
                            bluetooth::disconnect_pan(&dev);
                            Err(e.into())
                        }
                    }
                }
            },
            move |this, result, window, cx| match result {
                Ok(addr) => {
                    this.bt_link = Some(dev);
                    this.connect_to(addr, window, cx);
                }
                Err(e) => this.notify_error("Bluetooth 接続に失敗しました", &e, window, cx),
            },
        );
    }

    /// ペアリングの手順 1: 端末に PIN が表示されたら、ユーザーに PIN を尋ねる。
    pub fn start_pairing(&mut self, addr: DeviceAddress, window: &mut Window, cx: &mut Context<Self>) {
        self.run_task(
            "ペアリングを開始しています",
            window,
            cx,
            move |_| Ok(Registration::start(&addr)?),
            move |this, result, window, cx| match result {
                Ok(registration) => {
                    let registration = Arc::new(std::sync::Mutex::new(Some(registration)));
                    this.prompt_text(
                        "ペアリング",
                        "端末の画面に表示されている 8 桁の PIN を入力してください",
                        "",
                        "登録",
                        window,
                        cx,
                        move |this, pin, window, cx| {
                            if let Some(reg) = registration.lock().unwrap().take() {
                                this.finish_pairing(addr, reg, pin, window, cx);
                            }
                        },
                    );
                }
                Err(e) => this.notify_error("ペアリングを開始できません", &e, window, cx),
            },
        );
    }

    fn finish_pairing(
        &mut self,
        addr: DeviceAddress,
        registration: Registration,
        pin: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        self.run_task(
            "端末に登録しています",
            window,
            cx,
            move |progress| {
                progress.set_detail("鍵を生成しています");
                let creds = registration.finish(&pin)?;
                store.save(&creds).context("認証情報の保存に失敗しました")?;
                Ok(creds)
            },
            move |this, result, window, cx| match result {
                Ok(creds) => {
                    this.credentials = Some(creds);
                    this.notify_success("ペアリングが完了しました", window, cx);
                    this.connect_to(addr, window, cx);
                }
                Err(e) => this.notify_error("ペアリングに失敗しました", &e, window, cx),
            },
        );
    }

    // ----- 描画 ---------------------------------------------------------

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let busy = self.busy.is_some();
        let connected = matches!(self.connection, Connection::Connected { .. });
        let bluetooth_section = self.render_bluetooth(busy || connected, cx);
        let theme = cx.theme();

        let devices = self.devices.iter().enumerate().map(|(i, d)| {
            let addr = d.address;
            let via = d.transport.label();
            ListItem::new(("device", i))
                .px_2()
                .rounded(theme.radius)
                .child(div().text_sm().child(d.name()))
                .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{via} · {addr}")))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if is_double_click(ev) && this.busy.is_none() && this.client().is_none() {
                        this.connect_to(addr, window, cx);
                    }
                }))
        });

        let connection_box = match &self.connection {
            Connection::Connected { name, status, .. } => v_flex()
                .gap_1()
                .child(div().text_sm().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(name.clone()))
                .child(div().text_xs().text_color(theme.muted_foreground).child(status.clone()))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("disconnect")
                                .label("切断")
                                .small()
                                .outline()
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
                        )
                        .child(
                            Button::new("device-settings")
                                .label("端末の設定")
                                .small()
                                .outline()
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| this.open_device_settings(window, cx))),
                        ),
                ),
            Connection::Disconnected => {
                v_flex().child(div().text_sm().text_color(theme.muted_foreground).child("未接続"))
            }
        };

        let usb_buttons = h_flex()
            .gap_2()
            .child(
                Button::new("connect-usb")
                    .label("USB で接続")
                    .icon(IconName::ArrowRight)
                    .primary()
                    .small()
                    .disabled(busy || connected)
                    .on_click(cx.listener(|this, _, window, cx| this.connect_usb(window, cx))),
            )
            .child(
                Button::new("usb-mode")
                    .label(self.usb_mode.label())
                    .tooltip("USB Ethernet の方式 (通常は RNDIS)")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.usb_mode = match this.usb_mode {
                            UsbNetMode::Rndis => UsbNetMode::CdcEcm,
                            UsbNetMode::CdcEcm => UsbNetMode::Rndis,
                        };
                        cx.notify();
                    })),
            );

        let device_list = v_flex()
            .id("devices")
            .gap_1()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .when(self.devices.is_empty(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("端末が見つかりません。Wi-Fi の場合は端末と同じネットワークに接続してください"),
                )
            })
            .children(devices);

        let pairing_status = div().text_xs().text_color(theme.muted_foreground).child(match &self.credentials {
            Some(c) => format!("登録済みクライアント: {}", short_id(&c.client_id)),
            None => "未登録".into(),
        });
        let pair_button = Button::new("pair").label("ペアリングし直す").small().ghost().disabled(busy).on_click(
            cx.listener(|this, _, window, cx| {
                let addr = match &this.connection {
                    Connection::Connected { client, .. } => Some(*client.address()),
                    Connection::Disconnected => this.devices.first().map(|d| d.address),
                };
                match addr {
                    Some(addr) => this.start_pairing(addr, window, cx),
                    None => this.notify_warning("先に「端末を検索」で端末を見つけてください", window, cx),
                }
            }),
        );

        v_flex()
            .w(px(280.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .p_3()
            .gap_4()
            .child(
                section("connection", "接続")
                    .child(connection_box)
                    .child(usb_buttons)
                    .child(div().text_xs().text_color(theme.muted_foreground).child(usb_summary(&self.usb_state))),
            )
            .child(section("bluetooth", "Bluetooth").child(bluetooth_section))
            .child(
                section("wifi", "Wi-Fi").child(
                    h_flex().child(
                        Button::new("discover")
                            .label("端末を検索")
                            .icon(IconName::Search)
                            .small()
                            .outline()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.discover(window, cx))),
                    ),
                ),
            )
            .child(section("devices-section", "見つかった端末").flex_1().min_h_0().child(device_list))
            .child(section("pairing", "ペアリング").child(pairing_status).child(h_flex().child(pair_button)))
    }

    fn render_bluetooth(&self, disabled: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        if self.bt_devices.is_empty() {
            return v_flex().child(div().text_xs().text_color(theme.muted_foreground).child(
                "ペアリング済みの端末がありません。端末の Bluetooth ペアリング画面を開いたまま、\
                 OS の Bluetooth 設定でペアリングしてから「端末を検索」を押してください",
            ));
        }
        v_flex().gap_1().children(self.bt_devices.iter().enumerate().map(|(i, dev)| {
            let d = dev.clone();
            ListItem::new(("bt-device", i))
                .px_2()
                .rounded(theme.radius)
                .child(div().text_sm().truncate().child(dev.name.clone()))
                .child(div().text_xs().text_color(theme.muted_foreground).child(dev.address.clone()))
                .suffix({
                    let view = cx.entity();
                    move |_, _| {
                        let view = view.clone();
                        let d = d.clone();
                        Button::new(("bt-connect", i)).label("接続").small().outline().disabled(disabled).on_click(
                            move |_, window, cx| {
                                view.update(cx, |this, cx| this.connect_bluetooth(d.clone(), window, cx))
                            },
                        )
                    }
                })
        }))
    }

    fn render_status_bar(&self) -> impl IntoElement + use<> {
        let mut bar = StatusBar::new().h(px(30.)).px_3().flex_shrink_0();
        if let Some(busy) = &self.busy {
            let s = busy.snapshot();
            let mut text = s.label.clone();
            if !s.detail.is_empty() {
                text = format!("{text}: {}", s.detail);
            }
            if s.total > 0 {
                text = format!("{text} ({} / {})", format_size(s.done), format_size(s.total));
            }
            bar = bar.left(Spinner::new().small()).child(div().flex_1().truncate().child(text));
            if let Some(p) = busy.percent() {
                bar = bar.right(div().w(px(160.)).child(Progress::new("task-progress").value(p)));
            }
        } else {
            let last = self.log.last().cloned().unwrap_or_default();
            bar = bar.child(div().flex_1().truncate().child(last));
            if let Some(clip) = &self.clipboard {
                bar = bar.right(format!("クリップボード: {} 件 ({})", clip.entries.len(), clip.mode.label()));
            }
        }
        if self.client().is_some() {
            bar = bar.right(format!("{} 項目 · {} 件選択", self.entries.len(), self.selection.len()));
        }
        bar
    }
}

fn section(id: &'static str, title: &'static str) -> GroupBox {
    GroupBox::new()
        .id(id)
        .gap_2()
        .title(div().text_xs().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(title))
        .content_style(StyleRefinement::default().gap_2())
}

pub fn is_double_click(ev: &ClickEvent) -> bool {
    matches!(ev, ClickEvent::Mouse(m) if m.up.click_count >= 2)
}

/// サイドバーに表示する USB 接続の状態。
fn usb_summary(state: &usb::UsbState) -> String {
    if !state.interfaces.is_empty() {
        let names: Vec<String> =
            state.interfaces.iter().map(|i| format!("{}{}", i.name, if i.up { "" } else { " (無効)" })).collect();
        format!("USB Ethernet: {}", names.join(", "))
    } else if let Some(port) = state.serial_ports.first() {
        format!("USB シリアル: {}", port.display())
    } else {
        "USB: 未検出".into()
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect::<String>() + "…"
}

/// 端末に到達できなくなったら `false`。
fn device_alive(client: &DeviceClient) -> bool {
    let addr = client.address();
    // USB と Bluetooth はリンクローカルアドレスを使う。端末を抜くか PAN リンクが切れると、すぐにイ
    // ンターフェースが消える。
    if addr.scope_id != 0 && addr.interface_name().is_none() {
        return false;
    }
    // 端末から何らかの応答 (エラー文書でも) があれば、端末はまだ存在する。
    !matches!(client.ping(), Err(Error::Http(_) | Error::Io(_)))
}

/// サイドバーに表示するバッテリーと容量の概要。
pub fn device_status(client: &DeviceClient) -> String {
    let mut parts = Vec::new();
    if let Ok(b) = client.battery()
        && let Some(level) = b.level
    {
        let plugged = b.plugged.as_deref().is_some_and(|p| p != "not_plugged" && p != "false");
        parts.push(format!("バッテリー {level}%{}", if plugged { " (充電中)" } else { "" }));
    }
    if let Ok(s) = client.storage()
        && let (Some(avail), Some(cap)) = (s.available, s.capacity)
    {
        parts.push(format!("空き {} / {}", format_size(avail), format_size(cap)));
    }
    if let Ok(v) = client.firmware_version() {
        parts.push(format!("FW {v}"));
    }
    parts.join(" · ")
}

impl Render for QuadernoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.render_sidebar(cx);
        let explorer = self.render_explorer(window, cx);
        let status = self.render_status_bar();

        v_flex()
            .id("quaderno-app")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_go_up))
            .on_action(cx.listener(Self::on_open_selected))
            .on_action(cx.listener(Self::on_delete_selected))
            .on_action(cx.listener(Self::on_rename_selected))
            .on_action(cx.listener(Self::on_copy_selected))
            .on_action(cx.listener(Self::on_cut_selected))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_clear_selection))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_upload))
            .on_action(cx.listener(Self::on_download))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .flex_1()
                        .pr_2()
                        .justify_between()
                        .child(div().text_sm().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("Quaderno4Linux"))
                        .child(
                            Button::new("licenses")
                                .label("ライセンス")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| this.show_licenses(window, cx))),
                        ),
                ),
            )
            .child(h_flex().flex_1().min_h_0().child(sidebar).child(explorer))
            .child(status)
    }
}
