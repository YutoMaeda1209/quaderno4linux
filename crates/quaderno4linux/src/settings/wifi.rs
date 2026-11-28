//! 端末の設定ダイアログの「Wi-Fi」のタブと、ネットワークを追加するダイアログ。

use std::collections::HashSet;
use std::net::Ipv4Addr;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::{AnyElement, App, Context, Entity, SharedString, Window, div, prelude::*, px};
use quaderno_core::{EapConfig, Error, Proxy, StaticIp, WifiConfig, WifiNetwork, WifiStatus};

use super::{ChoiceSelect, Loading, choice, choice_select, load_failed, loading_indicator, set_loaded};
use crate::app::QuadernoApp;

/// Wi-Fi のタブに表示する状態。
pub(super) type WifiPanel = Entity<Loading<WifiStatus>>;

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

type NetworkSelect = Entity<SelectState<SearchableVec<NetworkChoice>>>;

/// 802.1X の内部認証の選択肢。実機で確かめたのは `mschapv2` だけ。
const EAP_PHASE2: [(&str, &str); 4] = [("mschapv2", "MSCHAPv2"), ("gtc", "GTC"), ("mschap", "MSCHAP"), ("pap", "PAP")];

/// Wi-Fi のタブ。操作はその場で端末に反映し、ダイアログの「保存」とは関係しない。
pub(super) fn wifi_tab(view: &Entity<QuadernoApp>, panel: &WifiPanel, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let note = div().text_xs().text_color(muted).child("Wi-Fi の設定は、操作するとすぐに端末に反映されます。");
    let status = match panel.read(cx) {
        Loading::Ready(status) => status.clone(),
        Loading::Pending => return loading_indicator("Wi-Fi の設定を読み込んでいます", cx),
        Loading::Failed => return load_failed("Wi-Fi の設定を読み込めませんでした", cx),
    };

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
            .child(Button::new(("wifi-delete", i)).label("削除").small().ghost().on_click(move |_, window, cx| {
                view.update(cx, |this, cx| this.delete_wifi(network.clone(), panel.clone(), window, cx))
            }))
    });
    let saved = if !status.enabled {
        div().text_sm().text_color(muted).child("Wi-Fi をオンにすると表示されます").into_any_element()
    } else if status.saved.is_empty() {
        div().text_sm().text_color(muted).child("保存済みのネットワークはありません").into_any_element()
    } else {
        v_flex().id("wifi-saved").gap_2().max_h(px(200.)).overflow_y_scroll().children(rows).into_any_element()
    };
    let add =
        {
            let (view, panel) = (view.clone(), panel.clone());
            Button::new("wifi-add").label("ネットワークを追加").small().outline().disabled(!status.enabled).on_click(
                move |_, window, cx| view.update(cx, |this, cx| this.open_add_wifi(panel.clone(), window, cx)),
            )
        };

    v_flex()
        .gap_3()
        .child(note)
        .child(
            v_form()
                .child(
                    field().label("Wi-Fi").child(h_flex().gap_2().child(toggle).child(div().text_sm().child(current))),
                )
                .child(
                    field()
                        .label("保存済みのネットワーク")
                        .child(v_flex().gap_3().child(saved).child(h_flex().child(add))),
                ),
        )
        .into_any_element()
}

/// ネットワークを追加するダイアログの入力欄。
#[derive(Clone)]
struct AddWifiForm {
    ssid: Entity<InputState>,
    security: ChoiceSelect,
    password: Entity<InputState>,
    phase2: ChoiceSelect,
    identity: Entity<InputState>,
    anonymous_identity: Entity<InputState>,
    ip_mode: ChoiceSelect,
    address: Entity<InputState>,
    subnet: Entity<InputState>,
    gateway: Entity<InputState>,
    dns1: Entity<InputState>,
    dns2: Entity<InputState>,
    proxy: ChoiceSelect,
    proxy_host: Entity<InputState>,
    proxy_port: Entity<InputState>,
}

impl AddWifiForm {
    fn new(window: &mut Window, cx: &mut Context<QuadernoApp>) -> Self {
        let mut input = |placeholder: &'static str| cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let ssid = input("");
        let identity = input("");
        let anonymous_identity = input("anonymous");
        let address = input("192.168.0.10");
        let subnet = input("24 または 255.255.255.0");
        let gateway = input("192.168.0.1");
        let dns1 = input("");
        let dns2 = input("");
        let proxy_host = input("proxy.example.com");
        let proxy_port = input("8080");
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let security = choice_select(
            vec![
                choice("psk", security_label("psk")),
                choice("eap", security_label("eap")),
                choice("nonsec", security_label("nonsec")),
            ],
            "psk",
            false,
            window,
            cx,
        );
        let phase2 = choice_select(
            EAP_PHASE2.iter().map(|(value, label)| choice(value, *label)).collect(),
            "mschapv2",
            false,
            window,
            cx,
        );
        let ip_mode = choice_select(
            vec![choice("dhcp", "自動 (DHCP)"), choice("static", "手動 (固定 IP アドレス)")],
            "dhcp",
            false,
            window,
            cx,
        );
        let proxy = choice_select(vec![choice("off", "使わない"), choice("on", "使う")], "off", false, window, cx);
        Self {
            ssid,
            security,
            password,
            phase2,
            identity,
            anonymous_identity,
            ip_mode,
            address,
            subnet,
            gateway,
            dns1,
            dns2,
            proxy,
            proxy_host,
            proxy_port,
        }
    }

    fn values(&self, cx: &App) -> FormValues {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let trimmed = |input: &Entity<InputState>| text(input).trim().to_string();
        let select = |select: &ChoiceSelect| select.read(cx).selected_value().cloned().unwrap_or_default();
        FormValues {
            ssid: text(&self.ssid),
            security: select(&self.security),
            password: text(&self.password),
            phase2: select(&self.phase2),
            identity: trimmed(&self.identity),
            anonymous_identity: trimmed(&self.anonymous_identity),
            static_ip: select(&self.ip_mode) == "static",
            address: trimmed(&self.address),
            subnet: trimmed(&self.subnet),
            gateway: trimmed(&self.gateway),
            dns1: trimmed(&self.dns1),
            dns2: trimmed(&self.dns2),
            proxy: select(&self.proxy) == "on",
            proxy_host: trimmed(&self.proxy_host),
            proxy_port: trimmed(&self.proxy_port),
        }
    }
}

/// 入力欄の値。
#[derive(Default)]
struct FormValues {
    ssid: String,
    security: String,
    password: String,
    phase2: String,
    identity: String,
    anonymous_identity: String,
    static_ip: bool,
    address: String,
    subnet: String,
    gateway: String,
    dns1: String,
    dns2: String,
    proxy: bool,
    proxy_host: String,
    proxy_port: String,
}

impl QuadernoApp {
    fn set_wifi_enabled(&mut self, enabled: bool, panel: WifiPanel, window: &mut Window, cx: &mut Context<Self>) {
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

    fn apply_wifi_enabled(&mut self, enabled: bool, panel: WifiPanel, window: &mut Window, cx: &mut Context<Self>) {
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

    fn delete_wifi(&mut self, network: WifiNetwork, panel: WifiPanel, window: &mut Window, cx: &mut Context<Self>) {
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

    /// ネットワークを追加するダイアログを開き、周囲のネットワークを検索する。検索している間も、
    /// ネットワークは手動で入力できる。
    fn open_add_wifi(&mut self, panel: WifiPanel, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        let form = AddWifiForm::new(window, cx);
        // 見つかったネットワークの選択欄。見つからなければ `None`。
        let scan: Entity<Loading<Option<NetworkSelect>>> = cx.new(|_| Loading::Pending);
        let started =
            self.run_task("ネットワークを検索しています", window, cx, move |_| Ok(client.scan_wifi()?), {
                let (form, scan) = (form.clone(), scan.clone());
                move |_, result, window, cx| {
                    let value = match result {
                        Ok(networks) => Loading::Ready(network_select(networks, &form, window, cx)),
                        Err(e) => {
                            log::warn!("ネットワークを検索できませんでした: {e:#}");
                            Loading::Failed
                        }
                    };
                    set_loaded(&scan, value, cx);
                }
            });
        if started {
            self.show_add_wifi(form, scan, panel, window, cx);
        }
    }

    fn show_add_wifi(
        &mut self,
        form: AddWifiForm,
        scan: Entity<Loading<Option<NetworkSelect>>>,
        panel: WifiPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let (view, panel, form) = (view.clone(), panel.clone(), form.clone());
            let values = form.values(cx);
            let muted = cx.theme().muted_foreground;
            let nearby = match scan.read(cx) {
                Loading::Pending => h_flex()
                    .gap_2()
                    .child(Spinner::new().small())
                    .child(div().text_sm().text_color(muted).child("検索しています"))
                    .into_any_element(),
                Loading::Ready(Some(select)) => {
                    Select::new(select).placeholder("選択してください").menu_max_h(px(240.)).into_any_element()
                }
                Loading::Ready(None) => {
                    div().text_sm().text_color(muted).child("見つかりませんでした").into_any_element()
                }
                Loading::Failed => div().text_sm().text_color(muted).child("検索できませんでした").into_any_element(),
            };
            let mut fields = v_form()
                .child(
                    field()
                        .label("周辺のネットワーク")
                        .description("選ぶと、ネットワーク名とセキュリティを入力します。")
                        .child(nearby),
                )
                .child(field().label("ネットワーク名 (SSID)").child(Input::new(&form.ssid)))
                .child(field().label("セキュリティ").child(Select::new(&form.security)));
            if values.security == "eap" {
                fields = fields
                    .child(field().label("EAP 方式").child(div().text_sm().child("PEAP")))
                    .child(field().label("内部認証 (Phase 2)").child(Select::new(&form.phase2)))
                    .child(field().label("ID").child(Input::new(&form.identity)))
                    .child(field().label("匿名 ID").child(Input::new(&form.anonymous_identity)));
            }
            if values.security != "nonsec" {
                fields = fields.child(field().label("パスワード").child(Input::new(&form.password).mask_toggle()));
            }
            fields = fields.child(field().label("IP アドレスの取得").child(Select::new(&form.ip_mode)));
            if values.static_ip {
                fields = fields
                    .child(field().label("IP アドレス").child(Input::new(&form.address)))
                    .child(field().label("サブネットマスク").child(Input::new(&form.subnet)))
                    .child(field().label("デフォルトゲートウェイ").child(Input::new(&form.gateway)))
                    .child(field().label("DNS サーバー 1").child(Input::new(&form.dns1)))
                    .child(field().label("DNS サーバー 2").child(Input::new(&form.dns2)));
            }
            fields = fields.child(field().label("プロキシ").child(Select::new(&form.proxy)));
            if values.proxy {
                fields = fields
                    .child(field().label("プロキシのホスト").child(Input::new(&form.proxy_host)))
                    .child(field().label("プロキシのポート").child(Input::new(&form.proxy_port)));
            }
            let note = div().text_xs().text_color(cx.theme().muted_foreground).child(
                "端末と Wi-Fi で接続している場合は、端末が追加したネットワークに切り替えるため、接続が切れることがあります。",
            );
            dialog
                .title("ネットワークを追加")
                .width(px(480.))
                .child(
                    v_flex()
                        .id("add-wifi-form")
                        .gap_3()
                        .max_h(px(440.))
                        .overflow_y_scroll()
                        .pr_2()
                        .child(fields)
                        .child(note),
                )
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("追加").cancel_text("キャンセル"))
                .on_ok(move |_, window, cx| match build_config(&form.values(cx)) {
                    // 別の処理を実行中で追加できなければ、入力を残すためにダイアログを閉じない。
                    Ok(config) => view.update(cx, |this, cx| this.add_wifi(config, panel.clone(), window, cx)),
                    Err(message) => {
                        view.update(cx, |this, cx| this.notify_warning(message, window, cx));
                        false
                    }
                })
        });
    }

    fn add_wifi(&mut self, config: WifiConfig, panel: WifiPanel, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(client) = self.client() else { return true };
        self.run_task(
            "ネットワークを追加しています",
            window,
            cx,
            move |_| {
                match client.add_wifi(&config) {
                    // 応答がないまま通信が途切れた。端末がネットワークを切り替えた可能性がある。
                    Err(Error::Http(e)) if e.status().is_none() => return Ok((config.ssid, None)),
                    result => result?,
                }
                Ok((config.ssid, Some(client.wifi_status()?)))
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
        )
    }
}

/// 見つかったネットワークの選択欄。選んだネットワークの名前とセキュリティを入力欄に写す。
fn network_select(
    mut networks: Vec<WifiNetwork>,
    form: &AddWifiForm,
    window: &mut Window,
    cx: &mut Context<QuadernoApp>,
) -> Option<NetworkSelect> {
    // SSID を公開していないネットワークは名前を入力して追加する。同じネットワークは 1 つにまとめる。
    networks.retain(|n| !n.ssid.is_empty());
    let mut seen = HashSet::new();
    networks.retain(|n| seen.insert((n.ssid.clone(), n.security.clone())));
    if networks.is_empty() {
        return None;
    }
    let choices: Vec<NetworkChoice> = networks
        .iter()
        .enumerate()
        .map(|(index, n)| {
            let mut detail = vec![security_label(&n.security)];
            if let Some(band) = n.frequency_band.as_deref() {
                detail.push(band_label(band));
            }
            NetworkChoice { index, label: format!("{} ({})", n.ssid, detail.join("、")).into() }
        })
        .collect();
    let select = cx.new(|cx| SelectState::new(SearchableVec::new(choices), None, window, cx).searchable(true));
    let form = form.clone();
    cx.subscribe_in(&select, window, move |_, _, event: &SelectEvent<SearchableVec<NetworkChoice>>, window, cx| {
        let SelectEvent::Confirm(Some(index)) = event else { return };
        let Some(network) = networks.get(*index) else { return };
        form.ssid.update(cx, |s, cx| s.set_value(network.ssid.clone(), window, cx));
        form.security.update(cx, |s, cx| s.set_selected_value(&network.security, window, cx));
    })
    .detach();
    Some(select)
}

fn update_panel(panel: &WifiPanel, status: WifiStatus, cx: &mut Context<QuadernoApp>) {
    set_loaded(panel, Loading::Ready(status), cx);
}

/// 入力欄の値を確かめて、登録する設定にする。問題があればその説明を返す。
fn build_config(v: &FormValues) -> Result<WifiConfig, &'static str> {
    if v.ssid.is_empty() || v.ssid.len() > 32 {
        return Err("ネットワーク名は 1〜32 バイトで入力してください");
    }
    let eap = match v.security.as_str() {
        "nonsec" => None,
        "psk" if is_valid_psk(&v.password) => None,
        "psk" => return Err("パスワードは 8〜63 文字の英数字記号で入力してください"),
        "eap" if v.identity.is_empty() => return Err("ID を入力してください"),
        "eap" if v.password.is_empty() => return Err("パスワードを入力してください"),
        "eap" => Some(EapConfig {
            method: "peap".into(),
            phase2: v.phase2.clone(),
            identity: v.identity.clone(),
            anonymous_identity: v.anonymous_identity.clone(),
        }),
        _ => return Err("セキュリティを選んでください"),
    };
    let static_ip = if v.static_ip {
        let optional = |addr: &str, message| match addr {
            "" => Ok(String::new()),
            addr => addr.parse::<Ipv4Addr>().map(|a| a.to_string()).map_err(|_| message),
        };
        Some(StaticIp {
            address: v.address.parse::<Ipv4Addr>().map_err(|_| "IP アドレスの形式が正しくありません")?.to_string(),
            prefix_len: parse_prefix_len(&v.subnet).ok_or("サブネットマスクの形式が正しくありません")?,
            gateway: optional(&v.gateway, "デフォルトゲートウェイの形式が正しくありません")?,
            dns1: optional(&v.dns1, "DNS サーバー 1 の形式が正しくありません")?,
            dns2: optional(&v.dns2, "DNS サーバー 2 の形式が正しくありません")?,
        })
    } else {
        None
    };
    let proxy = if v.proxy {
        if v.proxy_host.is_empty() {
            return Err("プロキシのホストを入力してください");
        }
        let port = v
            .proxy_port
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or("プロキシのポートは 1〜65535 で入力してください")?;
        Some(Proxy { host: v.proxy_host.clone(), port })
    } else {
        None
    };
    let password = if v.security == "nonsec" { String::new() } else { v.password.clone() };
    Ok(WifiConfig { ssid: v.ssid.clone(), security: v.security.clone(), password, static_ip, proxy, eap })
}

/// `24` のようなプレフィックス長か、`255.255.255.0` のようなサブネットマスクを、プレフィックス長にする。
fn parse_prefix_len(subnet: &str) -> Option<u8> {
    if let Ok(len) = subnet.parse::<u8>() {
        return (len <= 32).then_some(len);
    }
    let mask = u32::from(subnet.parse::<Ipv4Addr>().ok()?);
    // 上位のビットから 1 が連続している場合だけサブネットマスクとして扱う。
    (mask.leading_ones() + mask.trailing_zeros() == 32).then(|| mask.leading_ones() as u8)
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
        "eap" => "WPA/WPA2 エンタープライズ (802.1X)".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn psk() -> FormValues {
        FormValues {
            ssid: "Home".into(),
            security: "psk".into(),
            password: "password1234".into(),
            phase2: "mschapv2".into(),
            ..Default::default()
        }
    }

    #[test]
    fn builds_config() {
        let config = build_config(&psk()).unwrap();
        assert_eq!((config.static_ip, config.proxy, config.eap), (None, None, None));
        assert!(build_config(&FormValues { password: "short".into(), ..psk() }).is_err());
        assert!(build_config(&FormValues { ssid: String::new(), ..psk() }).is_err());

        // セキュリティなしではパスワードを送らない。
        let open = build_config(&FormValues { security: "nonsec".into(), ..psk() }).unwrap();
        assert_eq!(open.password, "");

        let eap = FormValues { security: "eap".into(), password: "pw".into(), ..psk() };
        assert!(build_config(&eap).is_err(), "ID が必要");
        let config = build_config(&FormValues { identity: "user".into(), ..eap }).unwrap();
        assert_eq!(config.eap.unwrap().method, "peap");

        let ip = FormValues {
            static_ip: true,
            address: "192.168.0.10".into(),
            subnet: "255.255.255.0".into(),
            gateway: "192.168.0.1".into(),
            ..psk()
        };
        let config = build_config(&ip).unwrap().static_ip.unwrap();
        assert_eq!((config.prefix_len, config.dns1.as_str()), (24, ""));
        assert!(build_config(&FormValues { subnet: "255.0.255.0".into(), ..ip }).is_err());

        let proxy = FormValues { proxy: true, proxy_host: "proxy".into(), proxy_port: "0".into(), ..psk() };
        assert!(build_config(&proxy).is_err());
        let config = build_config(&FormValues { proxy_port: "8080".into(), ..proxy }).unwrap();
        assert_eq!(config.proxy.unwrap().port, 8080);
    }

    #[test]
    fn prefix_len() {
        assert_eq!(parse_prefix_len("24"), Some(24));
        assert_eq!(parse_prefix_len("255.255.254.0"), Some(23));
        assert_eq!(parse_prefix_len("0.0.0.0"), Some(0));
        assert_eq!(parse_prefix_len("33"), None);
        assert_eq!(parse_prefix_len("255.0.255.0"), None);
    }
}
