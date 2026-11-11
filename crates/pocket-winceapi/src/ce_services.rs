//! Service-manager queries for the emulated CE device.
//! BTH1: is a stream driver; it must not be confused with the absent BTS1:
//! Bluetooth shell service that GTShell tries to unload on startup.
use crate::{CallCtx, WinCeDispatcher};
use pocket_cpu::Prot;
use pocket_kernel::{DispatchOutcome, KernelError};
pub fn register(d: &mut WinCeDispatcher) {
    d.register_handler("coredll.dll", "GetServiceHandle", get);
    d.register_handler("coredll.dll", "DeregisterService", deregister);
    d.register_handler("coredll.dll", "__GetUserKData", user_kdata);
    d.register_handler("coredll.dll", "IsAPIReady", is_api_ready);
    d.register_handler("coredll.dll", "WaitForAPIReady", wait_api_ready);
}

fn user_kdata(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    let offset = ctx.arg_u32(0)?;
    if offset % 4 != 0 || offset > 0x800 - 4 {
        return failed(ctx, 87, 0);
    }
    ctx.kernel.sync_thread_tls(ctx.cpu)?;
    let value = ctx
        .cpu
        .read_u32_le(pocket_kernel::USER_KDATA_STRUCT_VA + offset)?;
    Ok(DispatchOutcome::ReturnedR0(value))
}
fn api_available(ctx: &CallCtx<'_>, api: u32) -> bool {
    if ctx.kernel.object_handles.api_set_ready(api) {
        return true;
    }
    // CE6 moved non-handle API sets to 80. Older SDKs use 2/3/7/8/9.
    // These are the device providers implemented here. Networking, TAPI and
    // services.exe are not made ready just because their files exist in ROM.
    if ctx.kernel.ce.version.is_some_and(|version| version.0 >= 6) {
        matches!(api, 0 | 80 | 81 | 83 | 84 | 85 | 91)
    } else {
        matches!(api, 0 | 2 | 3 | 6 | 7 | 8 | 14)
    }
}
fn is_api_ready(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    let api = ctx.arg_u32(0)?;
    if api >= 128 {
        return failed(ctx, 87, 0);
    }
    Ok(DispatchOutcome::ReturnedR0(u32::from(api_available(
        ctx, api,
    ))))
}
fn wait_api_ready(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    let api = ctx.arg_u32(0)?;
    let timeout = ctx.arg_u32(1)?;
    let key = (
        ctx.kernel.current_thread,
        ctx.thunk.thunk_va,
        ctx.cpu.read_reg(pocket_cpu::regs::ArmReg::Sp)?,
    );
    if api >= 128 {
        ctx.kernel.ce.api_waits.remove(&key);
        return failed(ctx, 87, u32::MAX);
    }
    if api_available(ctx, api) {
        ctx.kernel.ce.api_waits.remove(&key);
        ctx.kernel.wait_deadlines.remove(&key);
        return Ok(DispatchOutcome::ReturnedR0(0));
    }
    if timeout == 0 {
        return Ok(DispatchOutcome::ReturnedR0(258));
    }
    let deadline = ctx
        .kernel
        .ce
        .api_waits
        .entry(key)
        .or_insert_with(|| {
            (
                api,
                std::time::Instant::now() + std::time::Duration::from_millis(u64::from(timeout)),
            )
        })
        .1;
    if let Some(outcome) = crate::coredll::wait_display_until(ctx, deadline)? {
        return Ok(outcome);
    }
    ctx.kernel.ce.api_waits.remove(&key);
    Ok(DispatchOutcome::ReturnedR0(258))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluetooth::tests::{call, setup};
    use pocket_cpu::Cpu;
    #[test]
    fn ce6_bootstrap_uses_live_kdata_and_only_implemented_api_sets() {
        let (mut cpu, mut kernel, mut dispatcher) = setup();
        kernel.ce.version = Some((6, 0, 0));
        cpu.map_region(pocket_kernel::USER_KDATA_PAGE_BASE, 4096, Prot::READ)
            .unwrap();
        cpu.write_mem(
            pocket_kernel::USER_KDATA_STRUCT_VA + 0x20,
            &0x12345678u32.to_le_bytes(),
        )
        .unwrap();
        assert_eq!(
            call(
                &mut cpu,
                &mut kernel,
                &mut dispatcher,
                "coredll.dll",
                "__GetUserKData",
                &[0x20]
            ),
            DispatchOutcome::ReturnedR0(0x12345678)
        );
        let pid = kernel.object_handles.process_id();
        assert_eq!(
            call(
                &mut cpu,
                &mut kernel,
                &mut dispatcher,
                "coredll.dll",
                "__GetUserKData",
                &[12]
            ),
            DispatchOutcome::ReturnedR0(pid)
        );
        for api in [0, 80, 81, 84, 85] {
            assert_eq!(
                call(
                    &mut cpu,
                    &mut kernel,
                    &mut dispatcher,
                    "coredll.dll",
                    "WaitForAPIReady",
                    &[api, 0]
                ),
                DispatchOutcome::ReturnedR0(0)
            );
        }
        assert_eq!(
            call(
                &mut cpu,
                &mut kernel,
                &mut dispatcher,
                "coredll.dll",
                "WaitForAPIReady",
                &[87, 0]
            ),
            DispatchOutcome::ReturnedR0(258)
        );
        assert_eq!(
            call(
                &mut cpu,
                &mut kernel,
                &mut dispatcher,
                "coredll.dll",
                "WaitForAPIReady",
                &[128, 0]
            ),
            DispatchOutcome::ReturnedR0(u32::MAX)
        );
        assert_eq!(
            call(
                &mut cpu,
                &mut kernel,
                &mut dispatcher,
                "coredll.dll",
                "__GetUserKData",
                &[1]
            ),
            DispatchOutcome::ReturnedR0(0)
        );
        assert_eq!(kernel.thread_last_errors[&0], 87);
    }
    #[test]
    fn ce_services_absent_shell_service_is_invalid_handle_not_stream_driver() {
        let (mut cpu, mut k, mut d) = setup();
        cpu.write_mem(
            0x1000,
            &"BTS1:"
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(
            call(
                &mut cpu,
                &mut k,
                &mut d,
                "coredll.dll",
                "GetServiceHandle",
                &[0x1000, 0, 0]
            ),
            DispatchOutcome::ReturnedR0(u32::MAX)
        );
        assert_eq!(
            call(&mut cpu, &mut k, &mut d, "coredll.dll", "GetLastError", &[]),
            DispatchOutcome::ReturnedR0(1060)
        );
        assert_eq!(
            call(
                &mut cpu,
                &mut k,
                &mut d,
                "coredll.dll",
                "DeregisterService",
                &[u32::MAX]
            ),
            DispatchOutcome::ReturnedR0(0)
        );
        assert!(k.vfs.bluetooth.service.is_enabled());
    }
}
fn failed(ctx: &mut CallCtx<'_>, code: u32, value: u32) -> Result<DispatchOutcome, KernelError> {
    crate::coredll::set_thread_error(ctx, code);
    Ok(DispatchOutcome::ReturnedR0(value))
}
fn get(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    let prefix = ctx.arg_u32(0)?;
    let dll = ctx.arg_u32(1)?;
    let size = ctx.arg_u32(2)?;
    let prefix = match crate::ce_shortcut::text(ctx, prefix, 6) {
        Ok(n) => n,
        Err(e) => return failed(ctx, e, u32::MAX),
    };
    let bytes = prefix.as_bytes();
    if bytes.len() != 5
        || !bytes[..3].iter().all(u8::is_ascii_alphabetic)
        || !bytes[3].is_ascii_digit()
        || bytes[4] != b':'
    {
        return failed(ctx, 87, u32::MAX);
    }
    if dll != 0 && size == 0 {
        return failed(ctx, 87, u32::MAX);
    }
    if size != 0
        && ctx
            .cpu
            .check_guest_access(size, 4, Prot::READ | Prot::WRITE)
            .is_err()
    {
        return failed(ctx, 87, u32::MAX);
    }
    // No services.exe DLL instance is loaded by this device model. A registry
    // entry or a similarly named stream driver is not a running service.
    // Failure leaves optional output buffers untouched and prevents GTShell
    // from attempting to deregister a spurious zero handle.
    failed(ctx, 1060, u32::MAX)
}
fn deregister(ctx: &mut CallCtx<'_>) -> Result<DispatchOutcome, KernelError> {
    let _handle = ctx.arg_u32(0)?;
    // No service instance can have been obtained in this device model.
    // Never close an unrelated VFS/device handle or stop a host OS service.
    failed(ctx, 6, 0)
}
