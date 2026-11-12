//! Opt-in CE system-image metadata. Guest system DLL code remains HLE.
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// System CPL metadata is mapped read-only; only these HLE exports execute.
pub const CPL_MODULES: &[(&str, u32)] = &[("cplmain.cpl", 0x1000_0020)];

#[derive(Debug)]
pub struct Settings {
    pub key_repeat_delay_ms: u64,
    pub key_repeat_interval_ms: u64,
    pub(crate) audio: Option<(u32, bool)>,
    pub(crate) sip_visible: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            key_repeat_delay_ms: 400,
            key_repeat_interval_ms: 33,
            audio: None,
            sip_visible: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlPanel {
    pub hwnd: u32,
    pub thread: usize,
    pub thunk: u32,
    pub frame: crate::GuestCallFrame,
    pub applet: usize,
    pub previous_active: u32,
    pub previous_focus: u32,
    pub fields: Vec<u32>,
    pub status: u32,
}

#[derive(Debug)]
pub struct PendingSend {
    pub id: u64,
    pub sender: usize,
    pub thunk: u32,
    pub sp: u32,
    pub output: u32,
    pub recipient: usize,
    pub args: [u32; 4],
    pub deadline: Instant,
    pub result: Option<Result<u32, u32>>,
    pub delivered: bool,
}

#[derive(Debug)]
pub struct SendCallback {
    pub thread: usize,
    pub thunk: u32,
    pub frame: crate::GuestCallFrame,
    pub pending: Option<u64>,
    pub output: u32,
    pub builtin: bool,
}

#[derive(Debug, Default)]
pub struct SendMessages {
    pub next_id: u64,
    pub pending: Vec<PendingSend>,
    pub callbacks: Vec<SendCallback>,
}

#[derive(Debug)]
pub struct SipEnumeration {
    pub thread: usize,
    pub thunk: u32,
    pub frame: crate::GuestCallFrame,
    pub scratch: u32,
}

#[derive(Debug, Default)]
pub struct ShellWindows {
    pub desktop: Option<u32>,
    pub taskbar: Option<(u32, bool)>,
    pub started: HashSet<u32>,
}

#[derive(Debug, Default)]
pub struct State {
    pub version: Option<(u32, u32, u32)>,
    pub ordinals: HashMap<String, HashMap<u16, String>>,
    pub system_modules: HashSet<String>,
    pub windows: crate::windowing::ClassicWindows,
    pub window_icons: HashMap<u32, [u32; 2]>,
    pub send_messages: SendMessages,
    pub control_panel: Option<ControlPanel>,
    pub settings: Settings,
    pub shell: ShellWindows,
    pub clipboard: crate::clipboard::Client,
    pub sip_enumerations: Vec<SipEnumeration>,
    pub api_waits: HashMap<(usize, u32, u32), (u32, Instant)>,
    pub(crate) inline_ids: Option<(u32, u32)>,
}
