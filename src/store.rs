//! 設定（`config.json`）と挿入履歴（`history.json`）の読み書きの共通部分
//!
//! - 読めないとき（解析の失敗・NotFound 以外の I/O エラー・UTF-8 でない）は `{名前}.broken-{秒}` へ退避して初期値で始める
//! - 退避もできないときは、その起動の間は保存しない（読めなかったファイルを初期値で上書きしない）
//! - 書くときは一時ファイルに書いてから置き換える
//!
//! ルール `.claude/rules/au2-plugin-checklist.md`「設定ファイルの下位互換」

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// 読み込んだ結果
#[derive(Debug)]
pub struct Loaded<T> {
    pub value: T,
    /// 利用者に知らせること（読めなかった・退避した・保存しない）
    pub warning: Option<String>,
    /// false なら、この起動の間は保存しない（読めず、退避もできなかった）
    pub writable: bool,
}

/// `path` の JSON を読む。無ければ初期値。`what` は知らせる文の主語（「設定」「挿入履歴」）
pub fn load_json<T: DeserializeOwned + Default>(path: &Path, what: &str) -> Loaded<T> {
    let error = match std::fs::read(path) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            // メモ帳などで保存し直すと BOM が付くことがある。それだけで捨てないように外して読む
            Ok(text) => match serde_json::from_str::<T>(text.strip_prefix('\u{feff}').unwrap_or(text)) {
                Ok(value) => return Loaded { value, warning: None, writable: true },
                Err(e) => format!("解析できません: {e}"),
            },
            Err(e) => format!("UTF-8 ではありません: {e}"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded { value: T::default(), warning: None, writable: true };
        }
        Err(e) => format!("読み込めません: {e}"),
    };
    match backup_broken(path) {
        Ok(dest) => Loaded {
            value: T::default(),
            warning: Some(format!("{what}を読めなかったので {} へ退避し、初期値で始めます（{error}）", dest.display())),
            writable: true,
        },
        Err(be) => Loaded {
            value: T::default(),
            warning: Some(format!(
                "{what}を読めず、退避もできないので、この起動の間は{what}を保存しません（{error} / 退避: {be}）: {}",
                path.display()
            )),
            writable: false,
        },
    }
}

/// 読めなかったファイルを `{名前}.broken-{秒}` へ移す
pub fn backup_broken(path: &Path) -> std::io::Result<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".broken-{secs}"));
    let dest = PathBuf::from(name);
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

/// 一時ファイル（`{名前}.tmp`）に書いてから置き換える。書きかけで落ちても元のファイルが残る
pub fn save_text(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    let tmp = PathBuf::from(name);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 読めない値（新しい版が足した列挙の値・型の違う値）を初期値に読み替える。
/// `#[serde(deserialize_with = "crate::store::lenient")]` で項目ごとに付ける。
/// 1 つの項目が読めないだけでファイル全体を捨てないため。
/// 外れると困る項目（検索するフォルダなど）には付けない（初期値で保存すると消える）
pub fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// テストごとの空のフォルダ
    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("file_finder_h_store_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `dir` の中で `prefix` で始まるファイル名
    pub(crate) fn names_starting_with(dir: &Path, prefix: &str) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix))
            .collect()
    }

    #[derive(Debug, Default, PartialEq, serde::Serialize, Deserialize)]
    #[serde(default)]
    struct Sample {
        name: String,
        n: u32,
    }

    #[test]
    fn missing_file_is_default_and_writable() {
        let dir = temp_dir("missing");
        let r: Loaded<Sample> = load_json(&dir.join("x.json"), "設定");
        assert_eq!(r.value, Sample::default());
        assert!(r.warning.is_none());
        assert!(r.writable);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_file_with_bom() {
        let dir = temp_dir("bom");
        let p = dir.join("x.json");
        std::fs::write(&p, "\u{feff}{\"name\":\"a\",\"n\":3}").unwrap();
        let r: Loaded<Sample> = load_json(&p, "設定");
        assert_eq!(r.value, Sample { name: "a".into(), n: 3 });
        assert!(r.warning.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unparsable_file_is_moved_aside() {
        let dir = temp_dir("broken");
        let p = dir.join("x.json");
        std::fs::write(&p, "{ broken").unwrap();
        let r: Loaded<Sample> = load_json(&p, "設定");
        assert_eq!(r.value, Sample::default());
        assert!(r.writable);
        assert!(r.warning.unwrap().contains("退避"));
        assert!(!p.exists());
        let moved = names_starting_with(&dir, "x.json.broken-");
        assert_eq!(moved.len(), 1);
        assert_eq!(std::fs::read_to_string(&moved[0]).unwrap(), "{ broken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_utf8_file_is_moved_aside() {
        let dir = temp_dir("sjis");
        let p = dir.join("x.json");
        // Shift_JIS の「設定」
        let bytes: &[u8] = &[b'{', b'"', 0x90, 0xDD, 0x92, 0xE8, b'"', b':', b'1', b'}'];
        std::fs::write(&p, bytes).unwrap();
        let r: Loaded<Sample> = load_json(&p, "設定");
        assert_eq!(r.value, Sample::default());
        assert!(r.writable);
        assert!(r.warning.unwrap().contains("UTF-8"));
        assert!(!p.exists());
        let moved = names_starting_with(&dir, "x.json.broken-");
        assert_eq!(moved.len(), 1);
        assert_eq!(std::fs::read(&moved[0]).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 別のプロセスが共有なしで開いている（読めず、移せもしない）。保存を止め、元のファイルは残す
    #[test]
    fn locked_file_blocks_saving_and_is_kept() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("locked");
        let p = dir.join("x.json");
        std::fs::write(&p, "{\"name\":\"keep\"}").unwrap();
        {
            let _lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&p).unwrap();
            let r: Loaded<Sample> = load_json(&p, "設定");
            assert_eq!(r.value, Sample::default());
            assert!(!r.writable, "退避できなかったら保存を止める");
            let warning = r.warning.unwrap();
            assert!(warning.contains("保存しません"), "{warning}");
            assert!(names_starting_with(&dir, "x.json.broken-").is_empty());
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"name\":\"keep\"}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_text_replaces_and_leaves_no_temp() {
        let dir = temp_dir("save");
        let p = dir.join("sub").join("x.json");
        save_text(&p, "1").unwrap();
        save_text(&p, "2").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "2");
        assert!(!dir.join("sub").join("x.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lenient_falls_back_per_field() {
        #[derive(Debug, Default, PartialEq, Deserialize)]
        enum Mode {
            #[default]
            A,
            B,
        }
        #[derive(Debug, Default, Deserialize)]
        #[serde(default)]
        struct S {
            #[serde(deserialize_with = "lenient")]
            mode: Mode,
            keep: u32,
        }
        let s: S = serde_json::from_str(r#"{"mode":"B","keep":1}"#).unwrap();
        assert_eq!((s.mode, s.keep), (Mode::B, 1));
        let s: S = serde_json::from_str(r#"{"mode":"Future","keep":2}"#).unwrap();
        assert_eq!((s.mode, s.keep), (Mode::A, 2));
        let s: S = serde_json::from_str(r#"{"mode":{"Custom":3},"keep":3}"#).unwrap();
        assert_eq!((s.mode, s.keep), (Mode::A, 3));
        let s: S = serde_json::from_str(r#"{"keep":4}"#).unwrap();
        assert_eq!((s.mode, s.keep), (Mode::A, 4));
    }
}
