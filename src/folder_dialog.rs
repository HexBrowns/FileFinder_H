//! フォルダ選択ダイアログ（IFileOpenDialog + FOS_PICKFOLDERS）
//!
//! egui のスレッドの COM 状態に依存しないよう、専用のスレッドを STA で立てて開く。
//! 結果はチャンネルで返し、UI 側が毎フレーム `try_recv` する。

use std::path::PathBuf;
use std::sync::mpsc;

use aviutl2::tracing;
use aviutl2_eframe::egui;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
    SIGDN_FILESYSPATH,
};
use windows::core::w;

/// 選ばれたフォルダ（キャンセルなら None）を 1 回だけ送るチャンネルを返す
pub fn pick_folder_async(ctx: egui::Context) -> mpsc::Receiver<Option<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("FileFinder_H folder dialog".into())
        .spawn(move || {
            let result = pick_folder_blocking();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    if let Err(e) = spawned {
        tracing::error!("FileFinder_H: ダイアログのスレッドを起動できませんでした: {e}");
    }
    rx
}

fn pick_folder_blocking() -> Option<PathBuf> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let result = (|| -> windows::core::Result<Option<PathBuf>> {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let options = dialog.GetOptions()?;
            dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
            dialog.SetTitle(w!("検索するフォルダを選択"))?;
            // キャンセルは HRESULT_FROM_WIN32(ERROR_CANCELLED) のエラーとして返る
            if dialog.Show(None).is_err() {
                return Ok(None);
            }
            let item = dialog.GetResult()?;
            let raw = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = raw.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(raw.0 as *const _));
            Ok(path)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        match result {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("FileFinder_H: フォルダ選択に失敗しました: {e}");
                None
            }
        }
    }
}
