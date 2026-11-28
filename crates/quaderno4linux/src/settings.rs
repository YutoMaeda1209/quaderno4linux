//! 端末の設定ダイアログ。

use std::time::SystemTime;

use gpui_kit::component::button::Button;
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{IndexPath, Sizable, WindowExt, h_flex};
use gpui_kit::{App, Context, Entity, SharedString, Window, prelude::*, px};
use quaderno_core::DeviceSettings;

use crate::app::QuadernoApp;

/// 選択肢。表示名と、端末に送る値を分けて持つ。
#[derive(Clone)]
struct Choice {
    value: String,
    label: SharedString,
}

impl SearchableListItem for Choice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }

    // タイムゾーンは表示名 (`UTC+9`) でも ID (`Asia/Tokyo`) でも探せるようにする。
    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.label.to_lowercase().contains(&query) || self.value.to_lowercase().contains(&query)
    }
}

type ChoiceSelect = Entity<SelectState<SearchableVec<Choice>>>;

/// 公式アプリ (Digital Paper App) が選択肢にしている値。
const STANDBY_MINUTES: [&str; 4] = ["5", "10", "30", "60"];
const DATE_FORMATS: [&str; 2] = ["yyyy/M/d", "M/d/yyyy"];
const TIMEZONES: [&str; 24] = [
    "UTC",
    "Asia/Tokyo",
    "Pacific/Honolulu",
    "America/Anchorage",
    "America/Los_Angeles",
    "America/Denver",
    "America/Chicago",
    "America/New_York",
    "America/Halifax",
    "Europe/Amsterdam",
    "Europe/Helsinki",
    "Etc/GMT+12",
    "Etc/GMT+11",
    "Etc/GMT+3",
    "Etc/GMT+2",
    "Etc/GMT+1",
    "Etc/GMT-3",
    "Etc/GMT-4",
    "Etc/GMT-5",
    "Etc/GMT-6",
    "Etc/GMT-7",
    "Etc/GMT-8",
    "Etc/GMT-10",
    "Etc/GMT-11",
];

impl QuadernoApp {
    /// 接続中の端末の設定を読み込み、設定ダイアログを表示する。
    pub fn open_device_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        self.run_task(
            "端末の設定を読み込んでいます",
            window,
            cx,
            move |_| Ok(client.settings()?),
            |this, result, window, cx| match result {
                Ok(settings) => this.show_device_settings(settings, window, cx),
                Err(e) => this.notify_error("端末の設定を読み込めませんでした", &e, window, cx),
            },
        );
    }

    fn show_device_settings(&mut self, current: DeviceSettings, window: &mut Window, cx: &mut Context<Self>) {
        let standby = choice_select(
            STANDBY_MINUTES
                .iter()
                .map(|m| choice(m, format!("{m} 分")))
                .chain([choice("never", "スタンバイにしない")])
                .collect(),
            &current.timeout_to_standby,
            false,
            window,
            cx,
        );
        let date_format = choice_select(
            DATE_FORMATS.iter().map(|f| choice(f, *f)).collect(),
            &current.date_format,
            false,
            window,
            cx,
        );
        let time_format = choice_select(
            vec![choice("24hour", "24 時間表示"), choice("12hour", "12 時間表示 (午前 / 午後)")],
            &current.time_format,
            false,
            window,
            cx,
        );
        let timezone = choice_select(
            TIMEZONES.iter().map(|tz| choice(tz, timezone_label(tz))).collect(),
            &current.timezone,
            true,
            window,
            cx,
        );

        let view = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (view, current) = (view.clone(), current.clone());
            let (standby, date_format, time_format, timezone) =
                (standby.clone(), date_format.clone(), time_format.clone(), timezone.clone());
            let sync_clock = {
                let view = view.clone();
                Button::new("sync-clock")
                    .label("端末の日時をこの PC に合わせる")
                    .small()
                    .outline()
                    .on_click(move |_, window, cx| view.update(cx, |this, cx| this.sync_device_clock(window, cx)))
            };
            let form = v_form()
                .child(field().label("スタンバイまでの時間").child(Select::new(&standby)))
                .child(field().label("日付の形式").child(Select::new(&date_format)))
                .child(field().label("時刻の形式").child(Select::new(&time_format)))
                .child(field().label("タイムゾーン").child(Select::new(&timezone).menu_max_h(px(240.))))
                .child(field().label("日時").child(h_flex().child(sync_clock)));
            dialog
                .title("端末の設定")
                .width(px(480.))
                .child(form)
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("保存").cancel_text("キャンセル"))
                .on_ok(move |_, window, cx| {
                    let selected = |select: &ChoiceSelect, cx: &App, fallback: &str| {
                        select.read(cx).selected_value().cloned().unwrap_or_else(|| fallback.to_string())
                    };
                    let new = DeviceSettings {
                        timeout_to_standby: selected(&standby, cx, &current.timeout_to_standby),
                        date_format: selected(&date_format, cx, &current.date_format),
                        time_format: selected(&time_format, cx, &current.time_format),
                        timezone: selected(&timezone, cx, &current.timezone),
                    };
                    let current = current.clone();
                    view.update(cx, |this, cx| this.save_device_settings(current, new, window, cx));
                    true
                })
        });
    }

    fn save_device_settings(
        &mut self,
        current: DeviceSettings,
        new: DeviceSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if current == new {
            return;
        }
        let Some(client) = self.client() else { return };
        self.run_task(
            "端末の設定を保存しています",
            window,
            cx,
            move |_| Ok(client.update_settings(&current, &new)?),
            |this, result, window, cx| match result {
                Ok(()) => this.notify_success("端末の設定を保存しました", window, cx),
                Err(e) => this.notify_error("端末の設定を保存できませんでした", &e, window, cx),
            },
        );
    }

    fn sync_device_clock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        self.run_task(
            "端末の日時を合わせています",
            window,
            cx,
            move |_| Ok(client.set_datetime(SystemTime::now())?),
            |this, result, window, cx| match result {
                Ok(()) => this.notify_success("端末の日時をこの PC に合わせました", window, cx),
                Err(e) => this.notify_error("端末の日時を合わせられませんでした", &e, window, cx),
            },
        );
    }
}

fn choice(value: &str, label: impl Into<SharedString>) -> Choice {
    Choice { value: value.to_string(), label: label.into() }
}

/// `current` を選んだ状態の選択欄。端末の値が選択肢になければ、その値も選択肢に加える。
fn choice_select(
    mut choices: Vec<Choice>,
    current: &str,
    searchable: bool,
    window: &mut Window,
    cx: &mut App,
) -> ChoiceSelect {
    if !current.is_empty() && !choices.iter().any(|c| c.value == current) {
        choices.push(choice(current, current.to_string()));
    }
    let selected = choices.iter().position(|c| c.value == current).map(IndexPath::new);
    cx.new(|cx| SelectState::new(SearchableVec::new(choices), selected, window, cx).searchable(searchable))
}

/// `Etc/GMT-9` は UTC+9 を表す (符号が逆) ので、UTC からのずれで表示する。
fn timezone_label(tz: &str) -> String {
    match tz.strip_prefix("Etc/GMT") {
        Some(offset) => {
            let flipped: String = offset
                .chars()
                .map(|c| {
                    if c == '-' {
                        '+'
                    } else if c == '+' {
                        '-'
                    } else {
                        c
                    }
                })
                .collect();
            format!("UTC{flipped} ({tz})")
        }
        None => tz.to_string(),
    }
}
