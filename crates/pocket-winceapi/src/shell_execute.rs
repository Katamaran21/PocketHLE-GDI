//! Windows CE ShellExecuteEx: launch guest executables through the existing
//! asynchronous CreateProcess runner, never through the host operating system.
use super::*;
const SIZE: usize = 60;
const NOCLOSEPROCESS: u32 = 0x40;
const NO_UI: u32 = 0x400;

fn registry_text(kernel: &KernelState, key: &str) -> Option<String> {
    use pocket_kernel::registry::RegistryValue;
    match kernel
        .registry
        .value(key, "")
        .or_else(|| kernel.registry.value(key, "default"))
    {
        Some(RegistryValue::Sz(text)) if !text.is_empty() => Some(text),
        _ => None,
    }
}

fn command_parts(command: &str) -> Result<(String, String), u32> {
    let command = command.trim();
    let (file, rest) = if let Some(quoted) = command.strip_prefix('"') {
        let end = quoted.find('"').ok_or(13u32)?;
        (&quoted[..end], &quoted[end + 1..])
    } else {
        let end = command.find(char::is_whitespace).unwrap_or(command.len());
        (&command[..end], &command[end..])
    };
    if file.is_empty() {
        return Err(2);
    }
    Ok((file.to_owned(), rest.trim_start().to_owned()))
}

fn existing_path(kernel: &KernelState, file: &str) -> Option<String> {
    let bare = !file.contains(['\\', '/']);
    let mut candidates = vec![file.to_owned()];
    if bare {
        candidates.push(format!(r"\Windows\{file}"));
    }
    if bare && !file.contains('.') {
        candidates.push(format!("{file}.exe"));
        candidates.push(format!(r"\Windows\{file}.exe"));
    }
    candidates.into_iter().find(|name| {
        kernel.vfs.resolve(name).is_some_and(|p| p.exists())
            || kernel.vfs.ram_file_size(name).is_some()
    })
}

fn read_shortcut(kernel: &mut KernelState, file: &str) -> Result<String, u32> {
    let handle = kernel.vfs.open_file(file, 1, 3, 3, false, 0)?.handle;
    let result = (|| {
        let size = kernel.vfs.size(handle).ok_or(13u32)?;
        if size > 32768 {
            return Err(13);
        }
        let mut bytes = vec![0; size as usize];
        let mut position = 0;
        while position < bytes.len() {
            let read = kernel
                .vfs
                .read(handle, &mut bytes[position..])
                .ok_or_else(|| kernel.vfs.read_failure_error(handle))?;
            if read == 0 {
                return Err(13);
            }
            position += read;
        }
        let split = bytes.iter().position(|&b| b == b'#').ok_or(13u32)?;
        let size: usize = std::str::from_utf8(&bytes[..split])
            .map_err(|_| 13u32)?
            .parse()
            .map_err(|_| 13u32)?;
        if size == 0 || size > 32768 {
            return Err(13);
        }
        let body = &bytes[split + 1..];
        let body = &body[..size.min(body.len())];
        // The CE6 ROM's Messenger/RDP links count two trailing bytes omitted
        // from the extracted files. Bound by both the declared length and EOF;
        // never read beyond the VFS snapshot or rewrite the supplied shortcut.
        // SHCreateShortcut uses guest CP_ACP (Latin-1), not UTF-8.
        Ok(body.iter().copied().map(char::from).collect())
    })();
    kernel.vfs.close(handle);
    result
}

fn association_command(command: &str, file: &str, parameters: &str) -> (String, bool, bool) {
    let mut expanded = String::new();
    let mut quoted = false;
    let mut file_placeholder = false;
    let mut args_placeholder = false;
    let mut chars = command.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '%' {
            match chars.peek().copied() {
                Some('1' | 'L' | 'l') => {
                    chars.next();
                    file_placeholder = true;
                    if !quoted {
                        expanded.push('"');
                    }
                    expanded.push_str(file);
                    if !quoted {
                        expanded.push('"');
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    args_placeholder = true;
                    expanded.push_str(parameters);
                    continue;
                }
                _ => {}
            }
        }
        if character == '"' {
            quoted = !quoted;
        }
        expanded.push(character);
    }
    (expanded, file_placeholder, args_placeholder)
}

fn resolve_launch(
    kernel: &mut KernelState,
    file: &str,
    parameters: &str,
    verb: &str,
) -> Result<(String, String), u32> {
    let mut file = file.trim().trim_matches('"').to_owned();
    let mut parameters = parameters.to_owned();
    let mut associated = false;
    let mut visited = std::collections::HashSet::new();
    for _ in 0..16 {
        file = existing_path(kernel, &file).ok_or(2u32)?;
        if !visited.insert(file.replace('/', "\\").to_ascii_lowercase()) {
            return Err(13);
        }
        if file.to_ascii_lowercase().ends_with(".lnk") {
            let command = read_shortcut(kernel, &file)?;
            let (target, mut embedded) = command_parts(&command)?;
            if !parameters.is_empty() {
                if !embedded.is_empty() {
                    embedded.push(' ');
                }
                embedded.push_str(&parameters);
            }
            file = target;
            parameters = embedded;
            continue;
        }
        if file.to_ascii_lowercase().ends_with(".exe") {
            if !associated && !verb.is_empty() && !verb.eq_ignore_ascii_case("open") {
                return Err(1155);
            }
            return Ok((file, parameters));
        }
        let directory = kernel.vfs.resolve(&file).is_some_and(|p| p.is_dir());
        let class = if directory {
            Some("Directory".to_owned())
        } else {
            file.rsplit(['\\', '/'])
                .next()
                .and_then(|name| name.rfind('.').map(|pos| &name[pos..]))
                .and_then(|extension| registry_text(kernel, &format!("HKCR\\{extension}")))
        };
        let operation = if verb.is_empty() {
            class
                .as_ref()
                .and_then(|class| registry_text(kernel, &format!(r"HKCR\{class}\shell")))
                .unwrap_or_else(|| "open".into())
        } else {
            verb.to_owned()
        };
        let command = class.as_ref().and_then(|class| {
            registry_text(kernel, &format!(r"HKCR\{class}\shell\{operation}\command"))
        });
        if let Some(command) = command {
            // Expand once: inserted filenames/arguments can themselves contain
            // literal %1 or %L and must never be treated as new placeholders.
            let (command, file_placeholder, args_placeholder) =
                association_command(&command, &file, &parameters);
            let (target, mut args) = command_parts(&command)?;
            if !file_placeholder {
                if !args.is_empty() {
                    args.push(' ');
                }
                args.push_str(&format!("\"{file}\""));
            }
            if !args_placeholder && !parameters.is_empty() {
                args.push(' ');
                args.push_str(&parameters);
            }
            file = target;
            parameters = args;
            associated = true;
        } else if directory && operation.eq_ignore_ascii_case("open") {
            parameters = format!("\"{file}\" {}", parameters).trim_end().to_owned();
            file = r"\Windows\explorer.exe".into();
        } else {
            return Err(1155);
        }
    }
    Err(13)
}

fn failed(ctx: &mut CallCtx<'_>, ptr: u32, error: u32) -> Result<DispatchOutcome, KernelError> {
    let shell_error: u32 = match error {
        2 | 3 | 5 | 8 => error,
        193 => 11,
        1155 => 31,
        _ => 5,
    };
    let _ = ctx
        .cpu
        .write_mem(ptr.wrapping_add(32), &shell_error.to_le_bytes());
    let _ = ctx.cpu.write_mem(ptr.wrapping_add(56), &0u32.to_le_bytes());
    set_thread_error(ctx, error);
    Ok(DispatchOutcome::ReturnedR0(0))
}

pub(super) fn execute(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    let ptr = ctx.arg_u32(0)?;
    let Ok(info) = ctx.cpu.read_mem(ptr, SIZE as u32) else {
        set_thread_error(ctx, ERROR_INVALID_PARAMETER);
        return Ok(DispatchOutcome::ReturnedR0(0));
    };
    let word = |off: usize| u32::from_le_bytes(info[off..off + 4].try_into().unwrap());
    if ptr == 0
        || word(0) != SIZE as u32
        || ctx
            .cpu
            .check_guest_access(ptr, SIZE as u32, Prot::WRITE)
            .is_err()
    {
        set_thread_error(ctx, ERROR_INVALID_PARAMETER);
        return Ok(DispatchOutcome::ReturnedR0(0));
    }
    // CE has no process current directory; lpDirectory has no effect.
    let mask = word(4);
    let key = (
        ctx.kernel.current_thread,
        ctx.thunk.thunk_va,
        ctx.cpu.read_reg(ArmReg::Sp)?,
    );
    if let Some(result) = ctx.kernel.process_launch_results.remove(&key) {
        let handle = match result {
            Ok(h) => h,
            Err(e) => return failed(ctx, ptr, e),
        };
        let table = ctx.kernel.object_handles.child(handle).unwrap();
        ctx.kernel
            .object_handles
            .bind(handle, HandleObject::ChildProcess(handle));
        ctx.kernel
            .object_handles
            .bind(handle + 1, HandleObject::ChildThread(handle));
        ctx.kernel.child_processes.insert(
            handle,
            pocket_kernel::ChildProcess {
                thread_handle: handle + 1,
                exit_code: None,
                process_handle_closed: false,
                thread_handle_closed: false,
            },
        );
        // CE returns a process handle only when explicitly requested. Closing
        // our temporary handles must not terminate the launched application.
        ctx.cpu.write_mem(ptr + 32, &handle.to_le_bytes())?;
        ctx.cpu.write_mem(
            ptr + 56,
            &(if mask & NOCLOSEPROCESS != 0 {
                handle
            } else {
                0
            })
            .to_le_bytes(),
        )?;
        close_owned_handle(ctx.kernel, handle + 1);
        if mask & NOCLOSEPROCESS == 0 {
            close_owned_handle(ctx.kernel, handle);
        }
        table.allow_start();
        return Ok(DispatchOutcome::ReturnedR0(1));
    }
    ctx.cpu.write_mem(ptr + 56, &0u32.to_le_bytes())?;
    if mask & !(NOCLOSEPROCESS | NO_UI) != 0 {
        return failed(ctx, ptr, ERROR_NOT_SUPPORTED);
    }
    let mut string = |off| -> Result<String, KernelError> {
        let p = word(off);
        if p == 0 {
            return Ok(String::new());
        }
        Ok(String::from_utf16_lossy(&read_wstr(ctx, p, 32768)?))
    };
    let (verb, file, parameters) = match (string(12), string(16), string(20)) {
        (Ok(v), Ok(f), Ok(p)) => (v, f, p),
        _ => return failed(ctx, ptr, ERROR_INVALID_PARAMETER),
    };
    if file.is_empty() {
        return failed(ctx, ptr, 2);
    }
    log::debug!(
        "ShellExecuteEx: file={file:?}, verb={verb:?}, parameters={parameters:?}, mask=0x{mask:x}"
    );
    // The ROM player is exposed as a system command. An explicit path to a
    // different executable still uses the ordinary guest process launcher.
    if file.eq_ignore_ascii_case("ceplayer")
        || file.eq_ignore_ascii_case("ceplayer.exe")
        || file.eq_ignore_ascii_case(r"\Windows\ceplayer.exe")
    {
        if !verb.is_empty() && !verb.eq_ignore_ascii_case("open") {
            return failed(ctx, ptr, 1155);
        }
        let path = parameters.trim();
        let path = path
            .strip_prefix('"')
            .and_then(|path| path.strip_suffix('"'))
            .unwrap_or(path);
        if path.is_empty() {
            return failed(ctx, ptr, ERROR_INVALID_PARAMETER);
        }
        if let Err(error) = crate::directshow::play_system_media(ctx, path) {
            return failed(ctx, ptr, error);
        }
        // ShellExecute success uses a value greater than 32. There is no
        // child process handle for an in-process system command.
        ctx.cpu.write_mem(ptr + 32, &33u32.to_le_bytes())?;
        return Ok(DispatchOutcome::ReturnedR0(1));
    }
    // Native Explorer passes its Desktop .lnk files here. Resolve their CE
    // commands and registered document verbs before validating the real EXE.
    let (file, parameters) = match resolve_launch(ctx.kernel, &file, &parameters, &verb) {
        Ok(launch) => launch,
        Err(error) => return failed(ctx, ptr, error),
    };
    if !ctx.kernel.process_launch_enabled
        || ctx.kernel.dll_notification_frame.is_some()
        || !ctx.kernel.module_attach_frames.is_empty()
    {
        return failed(ctx, ptr, ERROR_NOT_SUPPORTED);
    }
    log::info!("ShellExecuteEx: launch {file:?}, parameters={parameters:?}");
    let outcome = queue_child_launch(ctx, file, parameters, 0, key)?;
    if outcome == DispatchOutcome::ReturnedR0(0) {
        let error = ctx
            .kernel
            .thread_last_errors
            .get(&ctx.kernel.current_thread)
            .copied()
            .unwrap_or(5);
        failed(ctx, ptr, error)
    } else {
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocket_cpu::{stub::StubCpu, Cpu};

    #[test]
    fn desktop_links_preserve_guest_commands_and_documents_use_registered_verbs() {
        use pocket_kernel::registry::RegistryValue;
        let root = tempfile::tempdir().unwrap();
        let windows = root.path().join("Windows");
        std::fs::create_dir_all(windows.join("Desktop")).unwrap();
        std::fs::create_dir(root.path().join("My Documents")).unwrap();
        for name in ["pword.exe", "peghelp.exe", "explorer.exe", "ctlpnl.exe"] {
            std::fs::write(windows.join(name), b"fixture").unwrap();
        }
        let command = r#""\Windows\pword.exe" -new"#;
        std::fs::write(
            windows.join("Desktop/WordPad.lnk"),
            format!("{}#{command}", command.len()),
        )
        .unwrap();
        let mut kernel = super::super::tests::fresh_kernel();
        kernel.vfs.mount("\\", root.path());
        assert_eq!(
            resolve_launch(
                &mut kernel,
                r"\Windows\Desktop\WordPad.lnk",
                "-extra",
                "open"
            )
            .unwrap(),
            (r"\Windows\pword.exe".into(), "-new -extra".into())
        );
        assert_eq!(
            resolve_launch(&mut kernel, r"\My Documents", "", "").unwrap(),
            (r"\Windows\explorer.exe".into(), "\"\\My Documents\"".into())
        );
        std::fs::write(root.path().join("My Documents/Read me.htm"), b"help").unwrap();
        kernel.registry.set_value(
            r"HKCR\.htm",
            "default",
            RegistryValue::Sz("htmlfile".into()),
        );
        kernel.registry.set_value(
            r"HKCR\htmlfile\shell\open\command",
            "default",
            RegistryValue::Sz(r#""\Windows\peghelp.exe" "%1" %*"#.into()),
        );
        kernel.registry.set_value(
            r"HKCR\htmlfile\shell\print\command",
            "default",
            RegistryValue::Sz(r#""\Windows\peghelp.exe" /print %1"#.into()),
        );
        assert_eq!(
            resolve_launch(&mut kernel, r"\My Documents\Read me.htm", "-local", "open").unwrap(),
            (
                r"\Windows\peghelp.exe".into(),
                "\"\\My Documents\\Read me.htm\" -local".into()
            )
        );
        assert_eq!(
            resolve_launch(&mut kernel, r"\My Documents\Read me.htm", "", "print").unwrap(),
            (
                r"\Windows\peghelp.exe".into(),
                "/print \"\\My Documents\\Read me.htm\"".into()
            )
        );
        assert_eq!(
            resolve_launch(&mut kernel, "ctlpnl.exe", "", "").unwrap().0,
            r"\Windows\ctlpnl.exe"
        );
        assert_eq!(resolve_launch(&mut kernel, r"\Missing.exe", "", ""), Err(2));
        assert_eq!(
            resolve_launch(&mut kernel, r"\Windows\pword.exe", "", "print"),
            Err(1155)
        );
    }

    #[test]
    fn shortcut_share_denial_malformed_lengths_and_cycles_fail_without_launching() {
        let root = tempfile::tempdir().unwrap();
        let command = r"\Cycle.lnk";
        std::fs::write(
            root.path().join("Cycle.lnk"),
            format!("{}#{command}", command.len()),
        )
        .unwrap();
        std::fs::write(root.path().join("Broken.lnk"), b"invalid#short").unwrap();
        let mut kernel = super::super::tests::fresh_kernel();
        kernel.vfs.mount("\\", root.path());
        assert_eq!(resolve_launch(&mut kernel, r"\Cycle.lnk", "", ""), Err(13));
        assert_eq!(resolve_launch(&mut kernel, r"\Broken.lnk", "", ""), Err(13));
        let lease = kernel
            .vfs
            .open_file(r"\Cycle.lnk", 1, 0, 3, false, 0)
            .unwrap()
            .handle;
        assert_eq!(resolve_launch(&mut kernel, r"\Cycle.lnk", "", ""), Err(32));
        kernel.vfs.close(lease);
        assert!(kernel.pending_process_launch.is_none());
        assert_eq!(
            std::fs::read(root.path().join("Broken.lnk")).unwrap(),
            b"invalid#short"
        );
    }

    #[test]
    fn rom_shortcut_lengths_and_default_verbs_preserve_literal_percent_in_document_paths() {
        use pocket_kernel::registry::RegistryValue;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("Windows")).unwrap();
        std::fs::write(root.path().join("Windows/cetsc.exe"), b"test image").unwrap();
        std::fs::write(root.path().join("Windows/peghelp.exe"), b"test image").unwrap();
        std::fs::write(root.path().join("RDP.lnk"), b"20#\\windows\\cetsc.exe").unwrap();
        std::fs::write(root.path().join("Read %1 %L.htm"), b"document").unwrap();
        let mut kernel = super::super::tests::fresh_kernel();
        kernel.vfs.mount("\\", root.path());
        kernel.registry.set_value(
            r"HKCR\.htm",
            "default",
            RegistryValue::Sz("htmlfile".into()),
        );
        kernel.registry.set_value(
            r"HKCR\htmlfile\shell",
            "default",
            RegistryValue::Sz("view".into()),
        );
        kernel.registry.set_value(
            r"HKCR\htmlfile\shell\view\command",
            "default",
            RegistryValue::Sz(r#""\Windows\peghelp.exe" "%1" %*"#.into()),
        );
        assert_eq!(
            resolve_launch(&mut kernel, r"\RDP.lnk", "", "").unwrap().0,
            r"\windows\cetsc.exe"
        );
        assert_eq!(
            resolve_launch(&mut kernel, r"\Read %1 %L.htm", "-literal %1", "").unwrap(),
            (
                r"\Windows\peghelp.exe".into(),
                r#""\Read %1 %L.htm" -literal %1"#.into()
            )
        );
        assert_eq!(
            association_command(r#"tool %L "%l" %*"#, "read %L file", "%1"),
            (
                r#"tool "read %L file" "read %L file" %1"#.into(),
                true,
                true
            )
        );
        assert_eq!(
            std::fs::read(root.path().join("RDP.lnk")).unwrap(),
            b"20#\\windows\\cetsc.exe"
        );
    }

    #[test]
    fn shell_launch_acknowledgement_preserves_requested_handle_and_closes_temporary_handles() {
        for keep in [false, true] {
            let mut cpu = StubCpu::new();
            cpu.map_region(0x1000, 0x5000, Prot::READ | Prot::WRITE)
                .unwrap();
            cpu.write_reg(ArmReg::R0, 0x2000).unwrap();
            cpu.write_reg(ArmReg::Sp, 0x5000).unwrap();
            cpu.write_mem(0x2000, &60u32.to_le_bytes()).unwrap();
            cpu.write_mem(
                0x2004,
                &(if keep { NOCLOSEPROCESS } else { 0 }).to_le_bytes(),
            )
            .unwrap();
            let mut kernel = super::super::tests::fresh_kernel();
            let thunk = super::super::tests::dummy_thunk();
            let (handle, table) = kernel.object_handles.new_child().unwrap();
            table.hold_start();
            kernel
                .process_launch_results
                .insert((0, thunk.thunk_va, 0x5000), Ok(handle));
            assert_eq!(
                execute(&mut CallCtx {
                    cpu: &mut cpu,
                    kernel: &mut kernel,
                    thunk: &thunk
                })
                .unwrap(),
                DispatchOutcome::ReturnedR0(1)
            );
            assert_eq!(cpu.read_mem(0x2020, 4).unwrap(), handle.to_le_bytes());
            assert_eq!(
                cpu.read_mem(0x2038, 4).unwrap(),
                (if keep { handle } else { 0 }).to_le_bytes()
            );
            let child = &kernel.child_processes[&handle];
            assert!(child.thread_handle_closed);
            assert_eq!(child.process_handle_closed, !keep);
            assert_eq!(table.exit_code(table.process_id(), None), None);
        }
    }

    #[test]
    fn ce_directory_is_ignored_and_missing_executable_reports_file_not_found() {
        let mut cpu = StubCpu::new();
        cpu.map_region(0x1000, 0x5000, Prot::READ | Prot::WRITE)
            .unwrap();
        cpu.write_reg(ArmReg::R0, 0x2000).unwrap();
        cpu.write_reg(ArmReg::Sp, 0x5000).unwrap();
        cpu.write_mem(0x2000, &60u32.to_le_bytes()).unwrap();
        cpu.write_mem(0x2010, &0x3000u32.to_le_bytes()).unwrap();
        cpu.write_mem(0x2018, &0xffffffffu32.to_le_bytes()).unwrap();
        write_wide_str(&mut cpu, 0x3000, 100, "\\Missing\\tool.exe").unwrap();
        let mut kernel = super::super::tests::fresh_kernel();
        kernel.process_launch_enabled = true;
        let thunk = super::super::tests::dummy_thunk();
        assert_eq!(
            execute(&mut CallCtx {
                cpu: &mut cpu,
                kernel: &mut kernel,
                thunk: &thunk
            })
            .unwrap(),
            DispatchOutcome::ReturnedR0(0)
        );
        assert_eq!(kernel.thread_last_errors.get(&0), Some(&2));
        assert!(kernel.pending_process_launch.is_none());
    }

    #[test]
    fn failed_shell_launch_reports_error_and_never_returns_a_fake_process() {
        let mut cpu = StubCpu::new();
        cpu.map_region(0x1000, 0x5000, Prot::READ | Prot::WRITE)
            .unwrap();
        cpu.write_reg(ArmReg::R0, 0x2000).unwrap();
        cpu.write_reg(ArmReg::Sp, 0x5000).unwrap();
        cpu.write_mem(0x2000, &60u32.to_le_bytes()).unwrap();
        cpu.write_mem(0x2038, &0xdeadbeefu32.to_le_bytes()).unwrap();
        let mut kernel = super::super::tests::fresh_kernel();
        let thunk = super::super::tests::dummy_thunk();
        kernel
            .process_launch_results
            .insert((0, thunk.thunk_va, 0x5000), Err(2));
        assert_eq!(
            execute(&mut CallCtx {
                cpu: &mut cpu,
                kernel: &mut kernel,
                thunk: &thunk
            })
            .unwrap(),
            DispatchOutcome::ReturnedR0(0)
        );
        assert_eq!(cpu.read_mem(0x2020, 4).unwrap(), 2u32.to_le_bytes());
        assert_eq!(cpu.read_mem(0x2038, 4).unwrap(), 0u32.to_le_bytes());
        assert_eq!(kernel.thread_last_errors.get(&0), Some(&2));
        assert!(kernel.pending_process_launch.is_none());
    }
}
