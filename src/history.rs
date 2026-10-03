//! 挿入履歴（Everything の「実行回数」「最終実行日時」に当たる）。`Plugin/FileFinder_H/history.json` に保存する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// 読み込む。無ければ空。壊れていたら退避してから空で始める（黙って上書きしない）。
pub fn load(path: &Path) -> (History, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (History::default(), None),
        Err(e) => return (History::default(), Some(format!("挿入履歴を読めませんでした: {e}"))),
    };
    match serde_json::from_str(&text) {
        Ok(h) => (h, None),
        Err(e) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let broken = path.with_extension(format!("json.broken-{stamp}"));
            let note = if std::fs::rename(path, &broken).is_ok() {
                format!("挿入履歴が壊れていたので {} へ退避しました: {e}", broken.display())
            } else {
                format!("挿入履歴が壊れています（退避にも失敗）: {e}")
            };
            (History::default(), Some(note))
        }
    }
}

pub fn save(path: &Path, history: &History) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(history)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
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
}
