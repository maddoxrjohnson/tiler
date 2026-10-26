//! Starting tiler when the user logs in (the `start-at-login` setting).

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "tiler";

fn open_run_key() -> Option<HKEY> {
    let mut key = HKEY::default();
    // SAFETY: valid strings and out pointer; the key is closed by the caller.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(RUN_KEY),
            Some(0),
            KEY_READ | KEY_SET_VALUE,
            &mut key,
        )
    };
    (status == ERROR_SUCCESS).then_some(key)
}

fn current_value(key: HKEY) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: the buffer and its byte length match.
    let status = unsafe {
        RegGetValueW(
            key,
            PCWSTR::null(),
            &HSTRING::from(VALUE),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let chars = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..chars.min(buf.len())]))
}

/// Registers or unregisters `"<this exe>" <args>` to run at login. Unregistering
/// only removes the entry if it runs this very executable, so tiler never deletes
/// someone else's value.
pub fn set_start_at_login(enable: bool, args: &str) {
    let Some(key) = open_run_key() else {
        tracing::warn!("could not open the Run registry key");
        return;
    };
    if enable {
        match std::env::current_exe() {
            Ok(exe) => {
                let command = format!("\"{}\" {args}", exe.display());
                let wide: Vec<u16> = command.encode_utf16().chain([0]).collect();
                // SAFETY: REG_SZ data is the NUL-terminated UTF-16 string as bytes.
                let bytes = unsafe {
                    std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2)
                };
                let status = unsafe {
                    RegSetValueExW(key, &HSTRING::from(VALUE), Some(0), REG_SZ, Some(bytes))
                };
                if status != ERROR_SUCCESS {
                    tracing::warn!(?status, "could not register tiler to start at login");
                }
            }
            Err(e) => tracing::warn!(%e, "could not find tiler's own path"),
        }
    } else if let (Some(value), Ok(exe)) = (current_value(key), std::env::current_exe()) {
        let ours = value
            .to_lowercase()
            .contains(&exe.display().to_string().to_lowercase());
        if ours {
            // SAFETY: valid key and value name.
            let _ = unsafe { RegDeleteValueW(key, &HSTRING::from(VALUE)) };
        }
    }
    // SAFETY: we opened this key above.
    let _ = unsafe { RegCloseKey(key) };
}
