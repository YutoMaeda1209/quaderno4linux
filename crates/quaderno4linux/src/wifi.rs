//! 端末の Wi-Fi の設定ダイアログ。

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, IndexPath, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::{Context, Entity, SharedString, Window, div, prelude::*, px};
use quaderno_core::{Error, WifiNetwork, WifiStatus};

use crate::app::QuadernoApp;

/// 検索で見つかったネットワークの選択肢。値は検索結果での位置。
#[derive(Clone)]
struct NetworkChoice {
    index: usize,
    label: SharedString,
}

impl SearchableListItem for NetworkChoice {
    type Value = usize;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &usize {
        &self.index
    }
}

impl QuadernoApp {
    /// 接続中の端末の Wi-Fi の状態を読み込み、Wi-Fi の設定ダイアログを表示する。
    pub fn open_wifi_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        self.run_task(
            "Wi-Fi の設定を読み込んでいます",
            window,
            cx,
            move |_| Ok(client.wifi_status()?),
            |this, result, window, cx| match result {
                Ok(status) => this.show_wifi_settings(status, window, cx),
                Err(e) => this.notify_error("Wi-Fi の設定を読み込めませんでした", &e, window, cx),
            },
        );
    }

    fn show_wifi_settings(&mut self, status: WifiStatus, window: &mut Window, cx: &mut Context<Self>) {
        // ダイアログを開いたまま、操作のたびに読み込み直した状態で表示を更新する。
        let panel = cx.new(|_| status);
        let view = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let status = panel.read(cx).clone();
            let muted = cx.theme().muted_foreground;

            let toggle = {
                let (view, panel) = (view.clone(), panel.clone());
                Switch::new("wifi-enabled").checked(status.enabled).on_click(move |checked, window, cx| {
                    let enabled = *checked;
                    view.update(cx, |this, cx| this.set_wifi_enabled(enabled, panel.clone(), window, cx));
                })
            };
            let current = match (status.enabled, &status.current) {
                (false, _) => "オフ".to_string(),
                (true, Some(n)) if n.state.as_deref() == Some("connecting") => format!("{} に接続しています", n.ssid),
                (true, Some(n)) => format!("{} に接続中", n.ssid),
                (true, None) => "どのネットワークにも接続していません".to_string(),
            };

            let rows = status.saved.iter().enumerate().map(|(i, network)| {
                let (view, panel, network) = (view.clone(), panel.clone(), network.clone());
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(div().text_sm().child(network.ssid.clone()))
                            .child(div().text_xs().text_color(muted).child(security_label(&network.security))),
                    )
                    .child(Button::new(("wifi-delete", i)).label("削除").small().ghost().on_click(
                        move |_, window, cx| {
                            view.update(cx, |this, cx| this.delete_wifi(network.clone(), panel.clone(), window, cx))
                        },
                    ))
            });
            let saved = if !status.enabled {
                div().text_sm().text_color(muted).child("Wi-Fi をオンにすると表示されます").into_any_element()
            } else if status.saved.is_empty() {
                div().text_sm().text_color(muted).child("保存済みのネットワークはありません").into_any_element()
            } else {
                v_flex().id("wifi-saved").gap_2().max_h(px(200.)).overflow_y_scroll().children(rows).into_any_element()
            };
            let add = {
                let (view, panel) = (view.clone(), panel.clone());
                Button::new("wifi-add")
                    .label("ネットワークを追加")
                    .small()
                    .outline()
                    .disabled(!status.enabled)
                    .on_click(move |_, window, cx| {
                        view.update(cx, |this, cx| this.scan_wifi(panel.clone(), window, cx))
                    })
            };

            let form = v_form()
                .child(
                    field().label("Wi-Fi").child(h_flex().gap_2().child(toggle).child(div().text_sm().child(current))),
                )
                .child(
                    field()
                        .label("保存済みのネットワーク")
                        .child(v_flex().gap_3().child(saved).child(h_flex().child(add))),
                );
            dialog
                .title("Wi-Fi")
                .width(px(480.))
                .child(form)
                .button_props(DialogButtonProps::default().ok_text("閉じる"))
        });
    }

    fn set_wifi_enabled(
        &mut self,
        enabled: bool,
        panel: Entity<WifiStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if enabled {
            self.apply_wifi_enabled(true, panel, window, cx);
            return;
        }
        self.confirm(
            "Wi-Fi をオフにしますか？",
            "端末と Wi-Fi で接続している場合は、接続が切れます。",
            "オフにする",
            true,
            window,
            cx,
            move |this, window, cx| this.apply_wifi_enabled(false, panel.clone(), window, cx),
        );
    }

    fn apply_wifi_enabled(
        &mut self,
        enabled: bool,
        panel: Entity<WifiStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client() else { return };
        let label = if enabled { "Wi-Fi をオンにしています" } else { "Wi-Fi をオフにしています" };
        self.run_task(
            label,
            window,
            cx,
            move |_| {
                client.set_wifi_enabled(enabled)?;
                Ok(client.wifi_status()?)
            },
            move |this, result, window, cx| match result {
                Ok(status) => update_panel(&panel, status, cx),
                Err(e) => this.notify_error("Wi-Fi を切り替えられませんでした", &e, window, cx),
            },
        );
    }

    fn delete_wifi(
        &mut self,
        network: WifiNetwork,
        panel: Entity<WifiStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm(
            format!("{} を削除しますか？", network.ssid),
            "このネットワークで端末と Wi-Fi 接続している場合は、接続が切れます。",
            "削除",
            true,
            window,
            cx,
            move |this, window, cx| {
                let Some(client) = this.client() else { return };
                let (network, panel) = (network.clone(), panel.clone());
                this.run_task(
                    "ネットワークを削除しています",
                    window,
                    cx,
                    move |_| {
                        client.delete_wifi(&network)?;
                        Ok((network.ssid, client.wifi_status()?))
                    },
                    move |this, result, window, cx| match result {
                        Ok((ssid, status)) => {
                            update_panel(&panel, status, cx);
                            this.notify_success(format!("{ssid} を削除しました"), window, cx);
                        }
                        Err(e) => this.notify_error("ネットワークを削除できませんでした", &e, window, cx),
                    },
                );
            },
        );
    }

    fn scan_wifi(&mut self, panel: Entity<WifiStatus>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        self.run_task(
            "ネットワークを検索しています",
            window,
            cx,
            move |_| Ok(client.scan_wifi()?),
            move |this, result, window, cx| match result {
                Ok(networks) => this.show_add_wifi(networks, panel, window, cx),
                Err(e) => this.notify_error("ネットワークを検索できませんでした", &e, window, cx),
            },
        );
    }

    fn show_add_wifi(
        &mut self,
        mut networks: Vec<WifiNetwork>,
        panel: Entity<WifiStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // SSID を公開していないネットワークは選べないので除き、同じネットワークは 1 つにまとめる。
        networks.retain(|n| !n.ssid.is_empty());
        let mut seen = std::collections::HashSet::new();
        networks.retain(|n| seen.insert((n.ssid.clone(), n.security.clone())));
        if networks.is_empty() {
            self.notify_warning("ネットワークが見つかりませんでした", window, cx);
            return;
        }

        let choices: Vec<NetworkChoice> = networks
            .iter()
            .enumerate()
            .map(|(index, n)| {
                let mut detail = vec![security_label(&n.security)];
                if !is_supported(&n.security) {
                    detail.push("未対応".into());
                }
                if let Some(band) = n.frequency_band.as_deref() {
                    detail.push(band_label(band));
                }
                NetworkChoice { index, label: format!("{} ({})", n.ssid, detail.join("、")).into() }
            })
            .collect();
        let first = networks.iter().position(|n| is_supported(&n.security)).unwrap_or(0);
        let select = cx.new(|cx| {
            SelectState::new(SearchableVec::new(choices), Some(IndexPath::new(first)), window, cx).searchable(true)
        });
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));

        let view = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let (view, panel, networks, password) = (view.clone(), panel.clone(), networks.clone(), password.clone());
            let selected: Option<WifiNetwork> =
                select.read(cx).selected_value().and_then(|i| networks.get(*i)).cloned();
            let needs_password = selected.as_ref().is_some_and(|n| n.security == "psk");
            let form =
                v_form().child(field().label("ネットワーク").child(Select::new(&select).menu_max_h(px(240.)))).child(
                    field()
                        .label("パスワード")
                        .description(
                            "IP アドレスは DHCP で自動的に取得します。端末と Wi-Fi で接続している場合は、端末が追加したネットワークに切り替えるため、接続が切れることがあります。",
                        )
                        .child(Input::new(&password).mask_toggle().disabled(!needs_password)),
                );
            dialog
                .title("ネットワークを追加")
                .width(px(440.))
                .child(form)
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("追加").cancel_text("キャンセル"))
                .on_ok(move |_, window, cx| {
                    let Some(network) = selected.clone() else { return false };
                    let password = if needs_password { password.read(cx).value().to_string() } else { String::new() };
                    let problem = if !is_supported(&network.security) {
                        Some("このネットワークのセキュリティの方式には対応していません")
                    } else if needs_password && !is_valid_psk(&password) {
                        Some("パスワードは 8〜63 文字で入力してください")
                    } else {
                        None
                    };
                    view.update(cx, |this, cx| match problem {
                        Some(message) => {
                            this.notify_warning(message, window, cx);
                            false
                        }
                        None => {
                            this.add_wifi(network, password, panel.clone(), window, cx);
                            true
                        }
                    })
                })
        });
    }

    fn add_wifi(
        &mut self,
        network: WifiNetwork,
        password: String,
        panel: Entity<WifiStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client() else { return };
        self.run_task(
            "ネットワークを追加しています",
            window,
            cx,
            move |_| {
                match client.add_wifi(&network.ssid, &network.security, &password) {
                    // 応答がないまま通信が途切れた。端末がネットワークを切り替えた可能性がある。
                    Err(Error::Http(e)) if e.status().is_none() => return Ok((network.ssid, None)),
                    result => result?,
                }
                Ok((network.ssid, Some(client.wifi_status()?)))
            },
            move |this, result, window, cx| match result {
                Ok((ssid, Some(status))) => {
                    update_panel(&panel, status, cx);
                    this.notify_success(format!("{ssid} を追加しました"), window, cx);
                }
                Ok((ssid, None)) => this.notify_warning(
                    format!(
                        "{ssid} の追加中に端末との通信が途切れました。端末がネットワークを切り替えた可能性があります"
                    ),
                    window,
                    cx,
                ),
                Err(e) => this.notify_error("ネットワークを追加できませんでした", &e, window, cx),
            },
        );
    }
}

fn update_panel(panel: &Entity<WifiStatus>, status: WifiStatus, cx: &mut Context<QuadernoApp>) {
    panel.update(cx, |s, cx| {
        *s = status;
        cx.notify();
    });
}

/// 対応しているセキュリティの方式。802.1X (EAP) は証明書などの設定が必要なので扱わない。
fn is_supported(security: &str) -> bool {
    matches!(security, "nonsec" | "psk")
}

/// WPA/WPA2 パーソナルのパスフレーズ (8〜63 文字の ASCII) か、64 桁の 16 進数の鍵。
fn is_valid_psk(password: &str) -> bool {
    let len = password.len();
    password.is_ascii() && ((8..=63).contains(&len) || (len == 64 && password.chars().all(|c| c.is_ascii_hexdigit())))
}

fn security_label(security: &str) -> String {
    match security {
        "nonsec" => "セキュリティなし".into(),
        "psk" => "WPA/WPA2 パーソナル".into(),
        "eap" => "802.1X (EAP)".into(),
        other => other.into(),
    }
}

fn band_label(band: &str) -> String {
    match band {
        "2.4ghz" => "2.4 GHz".into(),
        "5ghz" => "5 GHz".into(),
        other => other.into(),
    }
}
