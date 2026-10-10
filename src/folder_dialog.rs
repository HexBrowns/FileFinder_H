//! フォルダ選択ダイアログ（IFileOpenDialog + FOS_PICKFOLDERS）
//!
//! egui のスレッドの COM 状態に依存しないよう、専用のスレッドを STA で立てて開く。
//! 結果はチャンネルで返し、UI 側が毎フレーム `try_recv` する。
//!
//! オーナーは自分のウィンドウの最上位（`GetAncestor(GA_ROOT)`）。本体にはめ込まれていれば本体のメインウィンドウ、
//! 「ウィンドウを分離」していればその枠になる。開く直前に求めるので、分離・移動の後でも合う。
//! オーナーがあると、ダイアログは本体の手前に出てタスクバーに別に並ばず、開いている間はオーナーが無効になる（モーダル）。
//!
//! オーナーは別のスレッド（本体の UI スレッド）のウィンドウだが、固まらない:
//! - `Show` はオーナーを `EnableWindow(FALSE)` し、ダイアログを閉じると戻す。どちらもオーナーのスレッドへの送信で、
//!   本体の UI スレッドがメッセージを回していれば返る。本体の UI スレッドも egui のスレッドもこのスレッドを待たない
//!   （結果は `try_recv` で受け、`join` しない）ので、待ち合いの輪ができない
//! - オーナーが別スレッドだと入力キューがつながる（`AttachThreadInput` 相当）。自分のウィンドウは本体の子なので
//!   egui のスレッドと本体の UI スレッドは元からつながっている。このスレッドは `Show` の中でメッセージを回し続ける
//! - 終了処理はこのスレッドを待たない。開いたまま本体が DLL を外しても、DLL は pin してある（`shutdown.rs`）

use std::path::PathBuf;
use std::sync::mpsc;

use aviutl2::tracing;
use aviutl2_eframe::egui;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, IsWindow};
use windows::core::w;

/// 選ばれたフォルダ（キャンセルなら None）を 1 回だけ送るチャンネルを返す。
/// `own_hwnd` は自分のウィンドウ（egui のウィンドウ）。オーナーはその最上位にする。取れなければオーナー無しで開く
pub fn pick_folder_async(ctx: egui::Context, own_hwnd: Option<isize>) -> mpsc::Receiver<Option<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("FileFinder_H folder dialog".into())
        .spawn(move || {
            let result = pick_folder_blocking(own_hwnd);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    if let Err(e) = spawned {
        tracing::error!("FileFinder_H: ダイアログのスレッドを起動できませんでした: {e}");
    }
    rx
}

/// 自分のウィンドウの最上位。ウィンドウが無くなっていれば None
fn owner_of(own_hwnd: Option<isize>) -> Option<HWND> {
    let hwnd = HWND(own_hwnd? as *mut std::ffi::c_void);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        (!root.is_invalid()).then_some(root)
    }
}

fn pick_folder_blocking(own_hwnd: Option<isize>) -> Option<PathBuf> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let result = (|| -> windows::core::Result<Option<PathBuf>> {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let options = dialog.GetOptions()?;
            dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
            dialog.SetTitle(w!("検索するフォルダを選択"))?;
            // キャンセルは HRESULT_FROM_WIN32(ERROR_CANCELLED) のエラーとして返る
            if dialog.Show(owner_of(own_hwnd)).is_err() {
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
