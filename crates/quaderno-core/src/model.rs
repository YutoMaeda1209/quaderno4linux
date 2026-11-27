//! 端末の API が返すデータ型。

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer};

/// ルートフォルダ ("Document") の ID。
pub const ROOT_FOLDER_ID: &str = "root";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    Document,
    Folder,
    #[serde(other)]
    Unknown,
}

/// 端末上の文書またはフォルダ。
#[derive(Clone, Debug, Deserialize)]
pub struct Entry {
    pub entry_id: String,
    pub entry_name: String,
    pub entry_type: EntryType,
    #[serde(default)]
    pub entry_path: String,
    #[serde(default)]
    pub parent_folder_id: Option<String>,
    /// 端末は数値を文字列で表す。
    #[serde(default, deserialize_with = "lenient_u64")]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub created_date: Option<String>,
    #[serde(default)]
    pub modified_date: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub total_page: Option<u64>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

impl Entry {
    pub fn is_folder(&self) -> bool {
        self.entry_type == EntryType::Folder
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct EntryList {
    #[serde(default)]
    pub entry_list: Vec<Entry>,
}

/// `:8080/register/information` で認証なしに取得できる情報。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct DeviceInfo {
    #[serde(default)]
    pub serial_number: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl DeviceInfo {
    /// 人が読むための端末名。
    pub fn display_name(&self) -> String {
        let model = self
            .model_name
            .clone()
            .or_else(|| self.extra.get("model").and_then(|v| v.as_str()).map(str::to_owned))
            .unwrap_or_else(|| "Quaderno".into());
        match &self.serial_number {
            Some(sn) => format!("{model} ({sn})"),
            None => model,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BatteryStatus {
    #[serde(default, deserialize_with = "lenient_u64")]
    pub level: Option<u64>,
    #[serde(default)]
    pub plugged: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct StorageStatus {
    #[serde(default, deserialize_with = "lenient_u64")]
    pub capacity: Option<u64>,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub available: Option<u64>,
}

/// 端末の設定 (`/system/configs/{key}`)。値は端末が返す文字列のまま持つ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSettings {
    /// 所有者名。
    pub owner: String,
    /// スタンバイになるまでの分数。スタンバイにしない場合は `never`。
    pub timeout_to_standby: String,
    /// 日付の形式 (`yyyy/M/d` など)。
    pub date_format: String,
    /// 時刻の形式 (`24hour` または `12hour`)。
    pub time_format: String,
    /// タイムゾーン ID (`Asia/Tokyo` など)。
    pub timezone: String,
}

impl DeviceSettings {
    pub const KEYS: [&str; 5] = ["owner", "timeout_to_standby", "date_format", "time_format", "timezone"];

    /// `KEYS` の各キーの値への参照 (`KEYS` と同じ順)。
    pub fn values_mut(&mut self) -> [&mut String; 5] {
        [
            &mut self.owner,
            &mut self.timeout_to_standby,
            &mut self.date_format,
            &mut self.time_format,
            &mut self.timezone,
        ]
    }

    /// `KEYS` と同じ順の値。
    pub fn values(&self) -> [&str; 5] {
        [&self.owner, &self.timeout_to_standby, &self.date_format, &self.time_format, &self.timezone]
    }

    /// `self` から `new` で値が変わったキーと、その新しい値。
    pub fn changes<'a>(&self, new: &'a DeviceSettings) -> Vec<(&'static str, &'a str)> {
        Self::KEYS
            .into_iter()
            .zip(self.values().into_iter().zip(new.values()))
            .filter(|(_, (old, new))| old != new)
            .map(|(key, (_, new))| (key, new))
            .collect()
    }
}

fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
}

/// バイト数を表示用に整形する (`1.2 MB`)。
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// 端末の ISO-8601 タイムスタンプ (`2024-01-02T03:04:05Z`) を `2024-01-02 03:04` に整形する。
pub fn format_date(date: &str) -> String {
    let d = date.replace('T', " ");
    d.get(..16).map(str::to_owned).unwrap_or(d)
}

/// 時刻を ISO 8601 の UTC (`2024-01-02T03:04:05Z`) で表す。端末の日時の設定に使う。
pub fn format_utc(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // 1970-01-01 からの日数を暦日に変換する (Howard Hinnant の civil_from_days)。
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn parse_entries() {
        let json = r#"{"count":"2","entry_list":[
            {"entry_id":"a","entry_name":"x.pdf","entry_type":"document","entry_path":"Document/x.pdf",
             "file_size":"71028218","modified_date":"2024-01-02T03:04:05Z","total_page":"12"},
            {"entry_id":"b","entry_name":"Notes","entry_type":"folder","entry_path":"Document/Notes","is_new":"false"}
        ]}"#;
        let list: EntryList = serde_json::from_str(json).unwrap();
        assert_eq!(list.entry_list.len(), 2);
        assert_eq!(list.entry_list[0].file_size, Some(71028218));
        assert_eq!(list.entry_list[0].total_page, Some(12));
        assert!(list.entry_list[1].is_folder());
        assert_eq!(list.entry_list[1].file_size, None);
    }

    #[test]
    fn sizes_and_dates() {
        assert_eq!(format_size(10), "10 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_date("2024-01-02T03:04:05Z"), "2024-01-02 03:04");
        assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(UNIX_EPOCH + Duration::from_secs(1_709_208_245)), "2024-02-29T12:04:05Z");
    }

    #[test]
    fn settings_changes() {
        let old = DeviceSettings { owner: "a".into(), time_format: "24hour".into(), ..Default::default() };
        let new = DeviceSettings { owner: "b".into(), timezone: "Asia/Tokyo".into(), ..old.clone() };
        assert_eq!(old.changes(&new), vec![("owner", "b"), ("timezone", "Asia/Tokyo")]);
        assert!(new.changes(&new).is_empty());
    }
}
