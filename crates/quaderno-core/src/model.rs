//! 端末の API が返すデータ型。

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

#[cfg(test)]
mod tests {
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
    }
}
