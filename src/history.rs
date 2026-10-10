//! 挿入履歴（Everything の「実行回数」「最終実行日時」に当たる）。`Plugin/FileFinder_H/history.json` に保存する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{self, Loaded};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub count: u32,
    /// 最後に挿入した時刻（UNIX 秒）
    pub last: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    /// キーはフルパスを小文字にしたもの（Windows のパスは大文字小文字を区別しない）
    records: HashMap<String, Record>,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

impl History {
    pub fn get(&self, path: &Path) -> Option<Record> {
        self.records.get(&key(path)).copied()
    }

    pub fn bump(&mut self, path: &Path, now: i64) {
        let r = self.records.entry(key(path)).or_default();
        r.count = r.count.saturating_add(1);
        r.last = now;
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
}

pub fn default_path() -> PathBuf {
    aviutl2::config::app_data_path()
        .join("Plugin")
        .join("FileFinder_H")
        .join("history.json")
}

/// 読み込む。無ければ空。読めなければ退避してから空で始め、退避もできなければ保存しない（`store.rs`）
pub fn load(path: &Path) -> Loaded<History> {
    store::load_json(path, "挿入履歴")
}

/// 一時ファイルに書いてから置き換える
pub fn save(path: &Path, history: &History) -> anyhow::Result<()> {
    store::save_text(path, &serde_json::to_string(history)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_counts_and_ignores_case() {
        let mut h = History::default();
        h.bump(Path::new("C:/素材/Rain.wav"), 100);
        h.bump(Path::new("c:/素材/rain.WAV"), 200);
        assert_eq!(h.get(Path::new("C:/素材/RAIN.wav")), Some(Record { count: 2, last: 200 }));
        assert_eq!(h.get(Path::new("C:/other.wav")), None);
    }

    #[test]
    fn roundtrips_through_json() {
        let mut h = History::default();
        h.bump(Path::new("C:/a.png"), 5);
        let back: History = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back.get(Path::new("C:/a.png")), Some(Record { count: 1, last: 5 }));
    }

    /// 新しい版が足した項目は無視し、欠けた項目は 0 で読む
    #[test]
    fn tolerates_unknown_and_missing_fields() {
        let text = r#"{"records":{"c:/a.png":{"count":3,"last":9,"pinned":true},"c:/b.wav":{"count":2}},"version":2}"#;
        let h: History = serde_json::from_str(text).unwrap();
        assert_eq!(h.get(Path::new("C:/a.png")), Some(Record { count: 3, last: 9 }));
        assert_eq!(h.get(Path::new("C:/b.wav")), Some(Record { count: 2, last: 0 }));
    }

    #[test]
    fn load_moves_non_utf8_file_aside() {
        let dir = crate::store::tests::temp_dir("history");
        let p = dir.join("history.json");
        std::fs::write(&p, [0xFFu8, 0xFE, b'{', b'}']).unwrap();
        let r = load(&p);
        assert_eq!(r.value.len(), 0);
        assert!(r.writable);
        assert!(!p.exists());
        assert_eq!(crate::store::tests::names_starting_with(&dir, "history.json.broken-").len(), 1);

        let mut h = History::default();
        h.bump(Path::new("C:/a.png"), 1);
        save(&p, &h).unwrap();
        assert_eq!(load(&p).value.get(Path::new("C:/a.png")), Some(Record { count: 1, last: 1 }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
