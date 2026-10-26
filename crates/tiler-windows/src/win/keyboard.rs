//! The global keyboard hook that turns bound key chords into tiler commands.

use std::collections::HashSet;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::thread;

use crossbeam_channel::Sender;
use tiler_core::{KeyChord, Mods};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_LCONTROL, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RWIN,
    VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, HC_ACTION,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::vk::{is_modifier, key_name};

/// The chords bound in the current mode, shared with the server.
pub type Bindings = Arc<RwLock<HashSet<KeyChord>>>;

struct Shared {
    bindings: Bindings,
    tx: Sender<KeyChord>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// An unassigned virtual key, pressed while Alt or Win is held so that releasing the
/// modifier later doesn't open the app's menu bar or the Start menu.
const VK_MASK: u16 = 0xE8;

fn held(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: plain query.
    (unsafe { GetAsyncKeyState(i32::from(vk.0)) }) < 0
}

/// AltGr on non-US layouts arrives as left Ctrl + right Alt. Those characters
/// (e.g. `@` on German keyboards) must never trigger a ctrl-alt binding.
fn altgr_held() -> bool {
    held(VK_RMENU) && held(VK_LCONTROL) && !held(VK_RCONTROL)
}

fn current_mods() -> Mods {
    let mut mods = Mods::NONE;
    for (vk, m) in [
        (VK_MENU, Mods::ALT),
        (VK_CONTROL, Mods::CTRL),
        (VK_SHIFT, Mods::SHIFT),
    ] {
        if held(vk) {
            mods = mods.union(m);
        }
    }
    if held(VK_LWIN) || held(VK_RWIN) {
        mods = mods.union(Mods::WIN);
    }
    mods
}

fn press_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_MASK),
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let inputs = [key(Default::default()), key(KEYEVENTF_KEYUP)];
    // SAFETY: two well-formed INPUTs.
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

/// Decides whether to swallow a key event. Must be fast: Windows silently removes
/// low-level hooks that take too long.
fn handle(msg: u32, vk: u32) -> bool {
    let Some(shared) = SHARED.get() else {
        return false;
    };
    if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
        if is_modifier(vk) || altgr_held() {
            return false;
        }
        let Some(key) = key_name(vk).and_then(|k| k.parse().ok()) else {
            return false;
        };
        let chord = KeyChord {
            mods: current_mods(),
            key,
        };
        let bound = shared
            .bindings
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&chord);
        if bound {
            // Inject the mask key now, while the modifier is still down, so the shell
            // sees "Win, E8, ..." whatever order the keys are released in.
            if chord.mods.contains(Mods::ALT) || chord.mods.contains(Mods::WIN) {
                press_mask_key();
            }
            let _ = shared.tx.send(chord);
            return true;
        }
    }
    false
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, lparam points to a KBDLLHOOKSTRUCT.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        // Our own injected keys (and other programs') pass through untouched.
        if kb.flags.0 & LLKHF_INJECTED.0 == 0 && handle(wparam.0 as u32, kb.vkCode) {
            return LRESULT(1);
        }
    }
    // SAFETY: forwarding to the next hook as required.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Starts the keyboard hook thread. Bound chords are sent to `tx` and swallowed;
/// everything else reaches applications as usual. Can only be called once.
pub fn spawn_keyboard_hook(bindings: Bindings, tx: Sender<KeyChord>) {
    if SHARED.set(Shared { bindings, tx }).is_err() {
        panic!("spawn_keyboard_hook may only be called once");
    }
    thread::Builder::new()
        .name("keyboard-hook".into())
        .spawn(|| {
            // SAFETY: the module handle of our own executable; the hook procedure is a
            // plain function that lives for the whole program.
            let hook = unsafe {
                GetModuleHandleW(None).and_then(|module| {
                    SetWindowsHookExW(
                        WH_KEYBOARD_LL,
                        Some(hook_proc),
                        Some(HINSTANCE(module.0)),
                        0,
                    )
                })
            };
            if let Err(e) = hook {
                tracing::error!(%e, "keyboard hook failed; hotkeys will not work");
                return;
            }
            // Low-level hooks are called through this thread's message loop.
            let mut msg = MSG::default();
            // SAFETY: standard message loop on the thread that installed the hook.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .expect("failed to spawn the keyboard hook thread");
}
