//! 端末の設定ダイアログの「一般」のタブ。

use std::time::SystemTime;

use gpui_kit::component::button::Button;
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Sizable, h_flex};
use gpui_kit::{AnyElement, App, Context, Entity, Window, prelude::*, px};
use quaderno_core::DeviceSettings;

use super::{ChoiceSelect, choice, choice_select};
use crate::app::QuadernoApp;

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

/// 「一般」のタブの入力欄。
#[derive(Clone)]
pub(super) struct GeneralForm {
    pub(super) current: DeviceSettings,
    standby: ChoiceSelect,
    date_format: ChoiceSelect,
    time_format: ChoiceSelect,
    timezone: ChoiceSelect,
}

impl GeneralForm {
    pub(super) fn new(current: DeviceSettings, window: &mut Window, cx: &mut App) -> Self {
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
        Self { current, standby, date_format, time_format, timezone }
    }

    pub(super) fn render(&self, view: &Entity<QuadernoApp>) -> AnyElement {
        let sync_clock = {
            let view = view.clone();
            Button::new("sync-clock")
                .label("端末の日時をこの PC に合わせる")
                .small()
                .outline()
                .on_click(move |_, window, cx| view.update(cx, |this, cx| this.sync_device_clock(window, cx)))
        };
        v_form()
            .child(field().label("スタンバイまでの時間").child(Select::new(&self.standby)))
            .child(field().label("日付の形式").child(Select::new(&self.date_format)))
            .child(field().label("時刻の形式").child(Select::new(&self.time_format)))
            .child(field().label("タイムゾーン").child(Select::new(&self.timezone).menu_max_h(px(240.))))
            .child(field().label("日時").child(h_flex().child(sync_clock)))
            .into_any_element()
    }

    pub(super) fn values(&self, cx: &App) -> DeviceSettings {
        let selected = |select: &ChoiceSelect, fallback: &str| {
            select.read(cx).selected_value().cloned().unwrap_or_else(|| fallback.to_string())
        };
        DeviceSettings {
            timeout_to_standby: selected(&self.standby, &self.current.timeout_to_standby),
            date_format: selected(&self.date_format, &self.current.date_format),
            time_format: selected(&self.time_format, &self.current.time_format),
            timezone: selected(&self.timezone, &self.current.timezone),
        }
    }
}

impl QuadernoApp {
    pub(super) fn save_device_settings(
        &mut self,
        current: DeviceSettings,
        new: DeviceSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if current == new {
            return true;
        }
        let Some(client) = self.client() else { return true };
        self.run_task(
            "端末の設定を保存しています",
            window,
            cx,
            move |_| Ok(client.update_settings(&current, &new)?),
            |this, result, window, cx| match result {
                Ok(()) => this.notify_success("端末の設定を保存しました", window, cx),
                Err(e) => this.notify_error("端末の設定を保存できませんでした", &e, window, cx),
            },
        )
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
