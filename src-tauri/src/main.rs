#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        if !is_running_as_admin() {
            if self_elevate() {
                return;
            }
        }
    }
    next_os_installer_lib::run()
}

/// Windows'ta yönetici yetkisi kontrolü (PowerShell'siz, hızlı).
#[cfg(windows)]
fn is_running_as_admin() -> bool {
    use std::process::Command;
    // "net session" sadece admin olarak çalışır. Hata verirse admin değiliz.
    Command::new("cmd")
        .args(["/C", "net", "session"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Uygulamayı yönetici olarak yeniden başlatır.
/// Başarılıysa true döner (çağıran instance kapanmalı).
#[cfg(windows)]
fn self_elevate() -> bool {
    use std::os::windows::ffi::OsStrExt;

    // Windows API tipleri
    type HINSTANCE = isize;
    type HWND = isize;
    const SW_SHOWNORMAL: i32 = 1;

    extern "system" {
        fn ShellExecuteW(
            hwnd: HWND,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> HINSTANCE;
    }

    fn to_wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };

    let exe_str = exe_path.to_string_lossy().to_string();
    let verb = to_wide("runas");
    let file = to_wide(&exe_str);

    // Komut satırı argümanlarını aktar (varsa)
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args_str = args.join(" ");
    let params = to_wide(&args_str);

    let result = unsafe {
        ShellExecuteW(
            0,
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    // ShellExecuteW > 32 ise başarılı
    result as usize > 32
}
