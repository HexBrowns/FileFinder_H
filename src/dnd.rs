//! 一覧からのドラッグ＆ドロップ（Everything の「結果をドラッグして他のアプリへ渡す」に当たる）
//!
//! ファイルのシェルアイテムからシェルのデータオブジェクト（CF_HDROP を含む）を作り、`SHDoDragDrop` に渡す。
//! エクスプローラーからドラッグしたときと同じ形なので、本体のタイムラインにはそのままドロップできる。
//!
//! `SHDoDragDrop` はドロップされるまで戻らない（その間はこのスレッドでモーダルなメッセージループが回る）。
//! マウスボタンを離したメッセージはそのループが受け取ってしまい egui には届かないので、
//! 戻ったあとに自分のウィンドウへ WM_LBUTTONUP を送り直す（送らないと egui はボタンが押されたままだと思う）。

use std::path::Path;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE, OleInitialize, OleUninitialize,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, ILCreateFromPathW, ILFree, IShellItemArray, SHCreateShellItemArrayFromIDLists,
    SHDoDragDrop,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, PostMessageW, WM_LBUTTONUP, WindowFromPoint};
use windows::core::HSTRING;

/// ファイルをドラッグする。ドロップされたら true（キャンセル・拒否なら false）
pub fn drag_files(paths: &[&Path]) -> windows::core::Result<bool> {
    unsafe {
        // ドラッグを始めた時点でカーソルの下にあるのは、この一覧のウィンドウ
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor)?;
        let own_window = WindowFromPoint(cursor);

        // winit がウィンドウのスレッドを OLE 初期化している前提だが、念のため自分でも呼ぶ（成功した分だけ戻す）
        let ole = OleInitialize(None).is_ok();
        let result = do_drag(paths);
        if ole {
            OleUninitialize();
        }

        release_mouse(own_window);
        result.map(|effect| effect != DROPEFFECT_NONE)
    }
}

unsafe fn do_drag(paths: &[&Path]) -> windows::core::Result<DROPEFFECT> {
    unsafe {
        let pidls: Vec<*mut ITEMIDLIST> = paths
            .iter()
            .map(|p| ILCreateFromPathW(&HSTRING::from(p.as_os_str())))
            .filter(|p| !p.is_null())
            .collect();
        let result = (|| {
            if pidls.is_empty() {
                // どのパスからもシェルアイテムを作れなかった（ファイルが消えた等）
                return Err(windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG));
            }
            let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
            let array: IShellItemArray = SHCreateShellItemArrayFromIDLists(&consts)?;
            let data: IDataObject = array.BindToHandler(None, &BHID_DataObject)?;
            // 移動は許さない（エクスプローラーへ落としても元のファイルは動かない）
            SHDoDragDrop(None, &data, None, DROPEFFECT_COPY | DROPEFFECT_LINK)
        })();
        for pidl in pidls {
            ILFree(Some(pidl as *const _));
        }
        result
    }
}

/// モーダルループが飲み込んだボタンの解放を egui に伝える
unsafe fn release_mouse(hwnd: HWND) {
    unsafe {
        if hwnd.is_invalid() {
            return;
        }
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() || !ScreenToClient(hwnd, &mut pt).as_bool() {
            return;
        }
        let lparam = ((pt.y as u16 as isize) << 16) | (pt.x as u16 as isize);
        let _ = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), LPARAM(lparam));
    }
}
