//! CE6 control.exe discovers CPlApplet through the normal module/resource ABI.
//! Its ROM CPL cannot be relocated as native code. Map only read-only resources
//! and supply applets that operate on the shared, private CE device settings.
use super::*;
use pocket_kernel::{ce::ControlPanel, controls::DialogPanel, registry::RegistryValue};

#[derive(Clone, Copy)]
struct Applet {
    name: &'static str,
    description: &'static str,
    icon: u32,
    string: u32,
}
const APPLETS: &[Applet] = &[
    Applet {
        name: "Owner",
        description: "Change owner information.",
        icon: 5040,
        string: 0x8040,
    },
    Applet {
        name: "Display",
        description: "Change display brightness.",
        icon: 5070,
        string: 0x8100,
    },
    Applet {
        name: "Keyboard",
        description: "Change keyboard repeat settings.",
        icon: 5020,
        string: 0x8020,
    },
    Applet {
        name: "Mouse",
        description: "Change double-click speed.",
        icon: 5090,
        string: 0x8150,
    },
    Applet {
        name: "System",
        description: "View system information and identify the device.",
        icon: 5060,
        string: 0x8060,
    },
    Applet {
        name: "Volume & Sounds",
        description: "Change application audio volume.",
        icon: 5110,
        string: 0x8160,
    },
    Applet {
        name: "Input Panel",
        description: "Show or hide the on-screen keyboard.",
        icon: 5140,
        string: 0x8210,
    },
];

pub(super) fn register(d: &mut WinCeDispatcher) {
    for &(dll, _) in pocket_kernel::ce::CPL_MODULES {
        d.register_handler(dll, "CPlApplet", applet);
    }
    d.register_handler("coredll.dll", "GetDoubleClickTime", double_click_time);
    d.register_handler("coredll.dll", "SetDoubleClickTime", set_double_click_time);
}

pub(super) fn load(
    ctx: &mut CallCtx<'_>,
    request: &str,
) -> Result<Option<DispatchOutcome>, KernelError> {
    let name = module_file_name(request);
    let Some((_, hle)) = pocket_kernel::ce::CPL_MODULES
        .iter()
        .find(|(dll, _)| *dll == name)
    else {
        return Ok(None);
    };
    if !ctx.kernel.ce.system_modules.contains(&name) {
        return Ok(None);
    }
    let Some(exports) = ctx.kernel.dynamic_exports.get(hle).cloned() else {
        return Ok(None);
    };
    let Some(path) = ctx.kernel.find_module_file(request) else {
        set_thread_error(ctx, 126);
        return Ok(Some(DispatchOutcome::ReturnedR0(0)));
    };
    let Some(base) = map_runtime_module(ctx, &path, request, true)? else {
        return Ok(Some(DispatchOutcome::ReturnedR0(0)));
    };
    let module = ctx
        .kernel
        .modules
        .iter_mut()
        .find(|m| m.handle == base)
        .unwrap();
    // Keep the normal runtime reference counts and resource addresses, but no
    // ROM instruction, import or DllMain is made executable or called.
    module.data_file = false;
    module.attached = true;
    module.image_entry = 0;
    ctx.kernel.dynamic_exports.insert(base, exports);
    log::info!(
        "Control Panel: {} resources at 0x{base:08x}, {} HLE applets",
        name,
        APPLETS.len()
    );
    Ok(Some(DispatchOutcome::ReturnedR0(base)))
}

fn ret(value: u32) -> Result<DispatchOutcome, KernelError> {
    Ok(DispatchOutcome::ReturnedR0(value))
}
fn wide_field(bytes: &mut [u8], text: &str) {
    let room = bytes.len() / 2 - 1;
    for (slot, unit) in bytes
        .chunks_exact_mut(2)
        .zip(text.encode_utf16().take(room))
    {
        slot.copy_from_slice(&unit.to_le_bytes());
    }
}
fn module(ctx: &CallCtx<'_>) -> u32 {
    ctx.kernel
        .module_by_name(&ctx.thunk.dll)
        .map_or(0, |m| m.handle)
}
fn label(ctx: &mut CallCtx<'_>, instance: u32, id: u32, fallback: &str) -> String {
    let (entries, base) = ctx.kernel.resource_scope(instance);
    let block = id / 16 + 1;
    let entry = entries
        .iter()
        .find(|e| e.ty == ResourceKey::Id(6) && e.name == ResourceKey::Id(block))
        .cloned();
    let text = entry
        .filter(|e| e.size <= 65536)
        .and_then(|e| ctx.cpu.read_mem(base + e.data_rva, e.size).ok())
        .and_then(|bytes| {
            let mut p = 0;
            for index in 0..16 {
                let n = u16::from_le_bytes(bytes.get(p..p + 2)?.try_into().ok()?) as usize;
                p += 2;
                let raw = bytes.get(p..p + n * 2)?;
                if index == id % 16 {
                    return Some(String::from_utf16_lossy(
                        &raw.chunks_exact(2)
                            .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
                            .collect::<Vec<_>>(),
                    ));
                }
                p += n * 2;
            }
            None
        });
    text.filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.into())
}
fn applet(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    let message = ctx.arg_u32(1)?;
    if message == 1 {
        return ret(1);
    }
    if message == 2 {
        return ret(APPLETS.len() as u32);
    }
    if matches!(message, 6 | 7) {
        return ret(0);
    }
    let index = ctx.arg_u32(2)? as usize;
    let Some(info) = APPLETS.get(index).copied() else {
        return ret(0);
    };
    let pointer = ctx.arg_u32(3)?;
    match message {
        3 => {
            if pointer == 0
                || ctx
                    .cpu
                    .check_guest_access(pointer, 16, Prot::WRITE)
                    .is_err()
            {
                set_thread_error(ctx, 87);
                return ret(0);
            }
            // Dynamic descriptors let the host select localized strings and
            // caller-owned icons from the supplied CE image.
            ctx.cpu.write_mem(
                pointer,
                &[0, 0, 0, index as u32]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )?;
            ret(0)
        }
        8 => {
            if pointer == 0
                || ctx
                    .cpu
                    .check_guest_access(pointer, 468, Prot::WRITE)
                    .is_err()
            {
                set_thread_error(ctx, 87);
                return ret(0);
            }
            let instance = module(ctx);
            let name = label(ctx, instance, info.string, info.name);
            let description = label(ctx, instance, info.string + 1, info.description);
            let icon =
                images::resource_icon(ctx, instance, &ResourceKey::Id(info.icon), false, 32, 32)?
                    .map_or(0, |image| ctx.kernel.gdi.register_icon(image));
            let mut bytes = [0u8; 468];
            for (offset, value) in [(0, 468u32), (12, index as u32), (16, icon)] {
                bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
            wide_field(&mut bytes[20..84], &name);
            wide_field(&mut bytes[84..212], &description);
            ctx.cpu.write_mem(pointer, &bytes)?;
            ret(1)
        }
        5 | 9 | 10 => run(ctx, index, info),
        _ => ret(0),
    }
}

fn text(k: &KernelState, key: &str, name: &str, fallback: &str) -> String {
    match k.registry.value(key, name) {
        Some(RegistryValue::Sz(s)) => s,
        _ => fallback.into(),
    }
}
fn number(k: &KernelState, key: &str, name: &str, fallback: u32) -> u32 {
    match k.registry.value(key, name) {
        Some(RegistryValue::Dword(n)) => n,
        _ => fallback,
    }
}
fn begin(ctx: &mut CallCtx<'_>, index: usize, info: Applet) -> Result<ControlPanel, KernelError> {
    let frame = GuestCallFrame {
        args: [
            ctx.arg_u32(0)?,
            ctx.arg_u32(1)?,
            ctx.arg_u32(2)?,
            ctx.arg_u32(3)?,
        ],
        sp: ctx.cpu.read_reg(ArmReg::Sp)?,
        lr: ctx.cpu.read_reg(ArmReg::Lr)?,
    };
    let hwnd = (0xdead2100..0xdead2200)
        .find(|h| !ctx.kernel.window_classes.contains_key(h))
        .ok_or_else(|| KernelError::Loader("Control Panel window handles exhausted".into()))?;
    let (sw, sh) = (
        ctx.kernel.framebuffer.width as i32,
        ctx.kernel.framebuffer.height as i32 - 26,
    );
    let (w, h) = (sw.min(440), sh.min(296));
    let (x, y) = ((sw - w) / 2, (sh - h) / 2);
    let instance = module(ctx);
    let caption = label(ctx, instance, info.string + 2, info.name);
    let previous_active = ctx.kernel.active_window;
    let previous_focus = ctx.kernel.controls.focus;
    ctx.kernel.controls.add_panel(DialogPanel {
        hwnd,
        x,
        y,
        w,
        h,
        visible: true,
        border: true,
        caption: Some(caption),
        ok_pressed: false,
        caption_height: None,
        caption_command: Some(2),
        caption_help: false,
    });
    ctx.kernel.window_classes.insert(hwnd, "#32770".into());
    ctx.kernel.window_rects.insert(hwnd, (x, y, w, h));
    ctx.kernel.window_styles.insert(hwnd, (0x90000000, 0));
    ctx.kernel.window_parents.insert(hwnd, frame.args[0]);
    ctx.kernel
        .window_threads
        .insert(hwnd, ctx.kernel.current_thread);
    ctx.kernel.window_visibility.insert(hwnd, true);
    ctx.kernel.active_window = hwnd;
    let mut fields = Vec::new();
    let description = if index == 4 {
        let version = ctx.kernel.ce.version.unwrap_or((6, 0, 0));
        format!(
            "Windows CE {}.{} (Build {})\nDisplay: {} x {}",
            version.0,
            version.1,
            version.2,
            sw,
            sh + 26
        )
    } else {
        info.description.into()
    };
    ctx.kernel.controls.create(
        hwnd,
        ControlClass::Static,
        10,
        description,
        0x50000000,
        12,
        8,
        w - 26,
        42,
    );
    let values: Vec<(&str, String, bool)> = match index {
        0 => [
            ("Name", "Owner"),
            ("Company", "Company"),
            ("Telephone", "Phone"),
            ("E-mail", "Email"),
        ]
        .into_iter()
        .map(|(label, name)| {
            (
                label,
                text(ctx.kernel, r"HKCU\ControlPanel\Owner", name, ""),
                false,
            )
        })
        .collect(),
        1 => vec![(
            "Brightness (1-31)",
            number(ctx.kernel, r"HKCU\ControlPanel\Backlight", "OnLevel", 11).to_string(),
            false,
        )],
        2 => vec![
            (
                "Initial delay (ms)",
                number(
                    ctx.kernel,
                    r"HKCU\ControlPanel\Keyboard",
                    "InitialDelay",
                    400,
                )
                .to_string(),
                false,
            ),
            (
                "Repeat interval (ms)",
                number(ctx.kernel, r"HKCU\ControlPanel\Keyboard", "RepeatRate", 80).to_string(),
                false,
            ),
        ],
        3 => vec![(
            "Double-click (ms)",
            number(
                ctx.kernel,
                r"HKCU\ControlPanel\Pen",
                "DoubleTapTimeout",
                500,
            )
            .to_string(),
            false,
        )],
        4 => vec![
            (
                "Device name",
                text(ctx.kernel, r"HKLM\Ident", "Name", "PocketHLE"),
                false,
            ),
            (
                "Description",
                text(ctx.kernel, r"HKLM\Ident", "Desc", "Windows CE device"),
                false,
            ),
        ],
        5 => vec![
            (
                "Volume (0-100)",
                ((number(ctx.kernel, r"HKCU\ControlPanel\Volume", "Volume", u32::MAX) & 0xffff)
                    * 100
                    / 65535)
                    .to_string(),
                false,
            ),
            (
                "Enable application sounds",
                (number(ctx.kernel, r"HKCU\ControlPanel\Volume", "Mute", 2) & 2 != 0).to_string(),
                true,
            ),
        ],
        _ => vec![(
            "Show on-screen keyboard",
            (number(ctx.kernel, r"HKCU\ControlPanel\SIP", "Visible", 0) != 0).to_string(),
            true,
        )],
    };
    for (i, (label, value, check)) in values.into_iter().enumerate() {
        let y = 58 + i as i32 * 32;
        let handle = if check {
            let handle = ctx.kernel.controls.create(
                hwnd,
                ControlClass::Button,
                20 + i as u32,
                label.into(),
                0x50010003,
                12,
                y,
                w - 26,
                24,
            );
            ctx.kernel.controls.get_mut(handle).unwrap().checked = value == "true";
            handle
        } else {
            ctx.kernel.controls.create(
                hwnd,
                ControlClass::Static,
                40 + i as u32,
                label.into(),
                0x50000000,
                12,
                y + 3,
                (w / 2 - 12).max(1),
                22,
            );
            ctx.kernel.controls.create(
                hwnd,
                ControlClass::Edit,
                20 + i as u32,
                value,
                0x50810080,
                w / 2,
                y,
                w / 2 - 14,
                25,
            )
        };
        ctx.kernel
            .window_threads
            .insert(handle, ctx.kernel.current_thread);
        fields.push(handle);
    }
    let bottom = h - DialogPanel::CAPTION_H - 2;
    let status = ctx.kernel.controls.create(
        hwnd,
        ControlClass::Static,
        50,
        String::new(),
        0x50000000,
        12,
        bottom - 66,
        w - 26,
        28,
    );
    for (i, (id, label)) in [(1, "OK"), (2, "Cancel"), (3, "Apply")]
        .into_iter()
        .enumerate()
    {
        ctx.kernel.controls.create(
            hwnd,
            ControlClass::Button,
            id,
            label.into(),
            0x50010000,
            (w - 258).max(8) + i as i32 * 82,
            bottom - 34,
            76,
            26,
        );
    }
    ctx.kernel.controls.focus = fields.first().copied().unwrap_or(hwnd);
    repaint_controls(ctx);
    ctx.kernel.framebuffer.mark_dirty();
    Ok(ControlPanel {
        hwnd,
        frame,
        applet: index,
        thread: ctx.kernel.current_thread,
        thunk: ctx.thunk.thunk_va,
        previous_active,
        previous_focus,
        fields,
        status,
    })
}
fn apply(k: &mut KernelState, state: &ControlPanel) -> Result<(), String> {
    let old_device_name = text(k, r"HKLM\Ident", "Name", "");
    let values: Vec<_> = state
        .fields
        .iter()
        .map(|&h| {
            k.controls
                .get(h)
                .map(|c| (c.text.clone(), c.checked))
                .unwrap_or_default()
        })
        .collect();
    let numeric = |i: usize, min, max| -> Result<u32, String> {
        values[i]
            .0
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| format!("Enter a value from {min} to {max}."))
    };
    let mut changes = Vec::new();
    match state.applet {
        0 => {
            for (i, name) in ["Owner", "Company", "Phone", "Email"]
                .into_iter()
                .enumerate()
            {
                if values[i].0.encode_utf16().count() > 255 {
                    return Err("Owner information is too long.".into());
                }
                changes.push((
                    r"HKCU\ControlPanel\Owner",
                    name,
                    RegistryValue::Sz(values[i].0.clone()),
                ));
            }
        }
        1 => {
            let level = numeric(0, 1, 31)?;
            for name in ["Level", "OnLevel"] {
                changes.push((
                    r"HKCU\ControlPanel\Backlight",
                    name,
                    RegistryValue::Dword(level),
                ));
            }
        }
        2 => {
            changes.push((
                r"HKCU\ControlPanel\Keyboard",
                "InitialDelay",
                RegistryValue::Dword(numeric(0, 100, 2000)?),
            ));
            changes.push((
                r"HKCU\ControlPanel\Keyboard",
                "RepeatRate",
                RegistryValue::Dword(numeric(1, 10, 1000)?),
            ));
        }
        3 => changes.push((
            r"HKCU\ControlPanel\Pen",
            "DoubleTapTimeout",
            RegistryValue::Dword(numeric(0, 100, 1000)?),
        )),
        4 => {
            if values[0].0.is_empty()
                || values[0].0.len() > 255
                || !values[0]
                    .0
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err("Use letters, digits, - or _ for the device name.".into());
            }
            if values[1].0.encode_utf16().count() > 255 {
                return Err("The description is too long.".into());
            }
            for (i, name) in ["Name", "Desc"].into_iter().enumerate() {
                changes.push((r"HKLM\Ident", name, RegistryValue::Sz(values[i].0.clone())));
            }
        }
        5 => {
            let volume = numeric(0, 0, 100)? * 65535 / 100;
            changes.push((
                r"HKCU\ControlPanel\Volume",
                "Volume",
                RegistryValue::Dword(volume | volume << 16),
            ));
            let mute = number(k, r"HKCU\ControlPanel\Volume", "Mute", 2);
            changes.push((
                r"HKCU\ControlPanel\Volume",
                "Mute",
                RegistryValue::Dword((mute & !2) | if values[1].1 { 2 } else { 0 }),
            ));
        }
        _ => changes.push((
            r"HKCU\ControlPanel\SIP",
            "Visible",
            RegistryValue::Dword(u32::from(values[0].1)),
        )),
    }
    // Validate the whole form before applying it. Save to the profile registry,
    // which is shared by Explorer and every independently running child.
    for (key, name, value) in changes {
        if !k.registry.set_value(key, name, value) {
            return Err("Not enough device storage to save settings.".into());
        }
    }
    k.registry
        .flush()
        .map_err(|error| format!("Cannot save settings: {error}"))?;
    k.sync_backlight();
    k.sync_ce_settings();
    if state.applet == 4 && text(k, r"HKLM\Ident", "Name", "") != old_device_name {
        k.object_handles.emit_app_event(13);
    }
    Ok(())
}
fn finish(ctx: &mut CallCtx<'_>, state: ControlPanel) -> Result<DispatchOutcome, KernelError> {
    discard_window_metadata(ctx, &state.hwnd);
    ctx.kernel.active_window = state.previous_active;
    ctx.kernel.controls.focus = state.previous_focus;
    for (i, value) in state.frame.args.into_iter().enumerate() {
        ctx.cpu.write_arg(i as u8, value)?;
    }
    ctx.cpu.write_reg(ArmReg::Sp, state.frame.sp)?;
    ctx.cpu.write_reg(ArmReg::Lr, state.frame.lr)?;
    queue_visible_window_paints(ctx.kernel, state.previous_active);
    repaint_controls(ctx);
    ctx.kernel.framebuffer.mark_dirty();
    ret(u32::from(matches!(state.frame.args[1], 9 | 10)))
}
fn run(ctx: &mut CallCtx<'_>, index: usize, info: Applet) -> Result<DispatchOutcome, KernelError> {
    let state = if let Some(state) = ctx.kernel.ce.control_panel.take() {
        if state.thread != ctx.kernel.current_thread
            || state.thunk != ctx.thunk.thunk_va
            || state.frame.sp != ctx.cpu.read_reg(ArmReg::Sp)?
        {
            ctx.kernel.ce.control_panel = Some(state);
            return ret(0);
        }
        state
    } else {
        begin(ctx, index, info)?
    };
    crate::shell_keyboard::service(ctx);
    while let Some(event) = take_pending_input(ctx) {
        update_key_state(ctx, event);
        if crate::shell_keyboard::input(ctx, event) {
            continue;
        }
        let action = match event {
            InputEvent::PointerDown { x, y } => {
                ctx.kernel
                    .controls
                    .pointer_down_in(state.hwnd, i32::from(x), i32::from(y))
            }
            InputEvent::PointerUp { x, y } => {
                ctx.kernel
                    .controls
                    .pointer_up_in(state.hwnd, i32::from(x), i32::from(y))
            }
            InputEvent::Text { character } => {
                let focus = ctx.kernel.controls.focus;
                control_message(ctx, focus, 0x102, character as u32, 1)?;
                None
            }
            InputEvent::KeyDown { vk: 0x1b } => Some(ControlAction::Clicked {
                parent: state.hwnd,
                hwnd: 0,
                id: 2,
            }),
            InputEvent::KeyDown { vk: 0x0d } => Some(ControlAction::Clicked {
                parent: state.hwnd,
                hwnd: 0,
                id: 1,
            }),
            InputEvent::KeyDown { vk } => ctx.kernel.controls.key_down_in(state.hwnd, vk),
            _ => None,
        };
        if let Some(ControlAction::Clicked { id, .. }) = action {
            if id == 2 {
                return finish(ctx, state);
            }
            if matches!(id, 1 | 3) {
                match apply(ctx.kernel, &state) {
                    Ok(()) if id == 1 => return finish(ctx, state),
                    Ok(()) => {
                        ctx.kernel.controls.get_mut(state.status).unwrap().text =
                            "Settings saved.".into()
                    }
                    Err(error) => ctx.kernel.controls.get_mut(state.status).unwrap().text = error,
                }
            }
        }
        repaint_controls(ctx);
        ctx.kernel.framebuffer.mark_dirty();
    }
    ctx.kernel.ce.control_panel = Some(state);
    if let Some(outcome) = wait_display_until(
        ctx,
        std::time::Instant::now() + std::time::Duration::from_millis(1),
    )? {
        return Ok(outcome);
    }
    Ok(DispatchOutcome::JumpTo(ctx.thunk.thunk_va))
}
fn double_click_time(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    ret(number(
        ctx.kernel,
        r"HKCU\ControlPanel\Pen",
        "DoubleTapTimeout",
        500,
    )
    .clamp(1, 5000))
}
fn set_double_click_time(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    let time = ctx.arg_u32(0)?;
    let time = if time == 0 { 500 } else { time.min(5000) };
    let saved = ctx.kernel.registry.set_value(
        r"HKCU\ControlPanel\Pen",
        "DoubleTapTimeout",
        RegistryValue::Dword(time),
    );
    if saved {
        ctx.kernel.controls.double_click_time_ms = u64::from(time);
    }
    ctx.kernel.sync_ce_settings();
    ret(u32::from(saved))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_panel_validates_before_saving_and_updates_shared_audio_and_registry() {
        let mut kernel = super::super::tests::fresh_kernel();
        kernel.ce.version = Some((6, 0, 0));
        let volume = kernel.controls.create(
            1,
            ControlClass::Edit,
            20,
            "101".into(),
            0x50000000,
            0,
            0,
            100,
            20,
        );
        let enabled = kernel.controls.create(
            1,
            ControlClass::Button,
            21,
            String::new(),
            0x50000003,
            0,
            30,
            100,
            20,
        );
        kernel.controls.get_mut(enabled).unwrap().checked = true;
        let state = ControlPanel {
            hwnd: 1,
            thread: 0,
            thunk: 0,
            frame: GuestCallFrame {
                args: [0; 4],
                sp: 0,
                lr: 0,
            },
            applet: 5,
            previous_active: 0,
            previous_focus: 0,
            fields: vec![volume, enabled],
            status: 0,
        };
        let (child, _) = kernel.object_handles.new_child().unwrap();
        let mut other = super::super::tests::fresh_kernel();
        other.attach_handle_context(kernel.child_handle_context(child).unwrap());
        other.ce.version = Some((6, 0, 0));
        assert!(apply(&mut kernel, &state).is_err());
        assert_eq!(
            other.registry.value(r"HKCU\ControlPanel\Volume", "Volume"),
            None
        );
        kernel.controls.get_mut(volume).unwrap().text = "0".into();
        apply(&mut kernel, &state).unwrap();
        assert_eq!(
            other.registry.value(r"HKCU\ControlPanel\Volume", "Volume"),
            Some(RegistryValue::Dword(0))
        );
        assert_eq!(kernel.audio.wave_volume(0), Some(0));
        other.sync_ce_settings();
        assert_eq!(other.audio.wave_volume(0), Some(0));
        kernel.controls.get_mut(volume).unwrap().text = "100".into();
        kernel.controls.get_mut(enabled).unwrap().checked = false;
        apply(&mut kernel, &state).unwrap();
        assert_eq!(kernel.audio.wave_volume(0), Some(0));
        assert_eq!(
            other.registry.value(r"HKCU\ControlPanel\Volume", "Volume"),
            Some(RegistryValue::Dword(u32::MAX))
        );
    }
}
