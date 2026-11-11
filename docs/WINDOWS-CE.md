# Windows CE desktop profiles

PocketHLE-GDI runs guest applications with Rust implementations of CE APIs.
It can use an extracted CE filesystem for original resources, fonts, system
version metadata, ordinal names and registry hives. This is high-level emulation;
OEM kernels, drivers and every original Control Panel applet are not implemented.

## Windows

Extract the Windows release and start `pockethle-gui.exe`. The **Windows CE**
page accepts an application EXE, an extracted image directory, and a device data
directory. Use **Save profile** to keep those choices. For CE6, select the `fs`
directory containing `Windows/coredll.dll`, or its parent containing `fs/Windows`.

The profile ZIP contains `profiles/wince6-portable.json` and a launcher script.
Place your filesystem under `wince6/fs` and Portable Explorer under
`portable-explorer/bin`, beside the `profiles` directory. Put the Windows emulator
beside `run-wince6-gui.cmd`, or pass its path as the script's first argument.
Paths in a JSON profile file are resolved relative to that file's directory.

```cmd
pockethle-gui.exe --ce-profile profiles\wince6-portable.json --windowed
```

`profiles/wince6.json` retains the original local workstation configuration.
For a different location, use the portable template or the GUI's folder selectors.

The private device directory receives writable filesystem overlays and
`registry.json`. The supplied image stays read-only. Original hives are imported
when that device registry is created; later launches preserve its changes.
Applications and independently running children inherit the same device registry
and mounted filesystem. Relative image/data paths in library backups are resolved
against the library root on both Windows and Android.

## Android

Install the signed universal APK (Android 7.0 or later; ARM64 and ARMv7). Its
application ID is `com.pockethle.gdi`, so it can coexist with the upstream APK.
Backups are portable between the two applications.

Import the application and open its settings. Enable **Use Windows CE profile**,
then **Choose extracted CE image**. The Android file selector copies the selected
image into the app's library without changing the original. CE uses 640x480,
classic frames and an unlimited runtime by default. Command-line and classic-frame
options are saved with that application.

If a complete private CE backup ZIP has been prepared, use **Restore backup** and
**Merge without overwriting** instead. The backup includes the application,
relative profile paths and image files; the device registry is created from its
hives on first launch. The public profile-template ZIP alone has no CE binaries
or firmware and is not a complete Android library backup.

## Desktop behavior and Control Panel

Classic windows have caption buttons and can be moved by dragging their title
bar. The desktop background is `#3A6EA5`. Explorer remains behind child processes
such as BlankApp; closing a child removes its pixels and exposes the desktop.
Pointer releases and key releases return to the process that received the press.
The taskbar and active shell menus retain their own layers.

Portable Explorer's Start menus, Run/Browse dialog and native list controls use
the mounted CE filesystem. Image lists retain copied icon pixels and overlay
state, and caption icons use the registered class or `WM_SETICON` resource.
The private compatibility-DLL builder in `tools/build-portable-explorer-compat.py`
fixes its folder-history comparison without modifying the original port.

The supplied CE6 `control.exe` can discover HLE applets through `CPlApplet`:
Owner, Display brightness, Keyboard repeat, Mouse double-click timing, System
identity, Volume & Sounds, and Input Panel. Forms validate and save to the private
shared registry. The original ROM `cplmain.cpl` is used only for read-only
resources; its instructions and `DllMain` are not executed. Other OEM CPLs and
hardware settings remain outside this implementation.

Original CE6 application DLLs such as `ceshell.dll` may have no relocation
table. The application loader reserves their preferred address and places import
thunks in a separate module slot. Existing mappings, including cold and
no-access pages, prevent the load with error 487. Unloading releases only that
module's exact mappings and allocator slot. Core OS DLLs and drivers continue
to use HLE providers; their ROM code is never executed.

The shell bootstrap exposes the existing keyboard through `SipEnumIM` and the
CE6 input-method CLSID ABI. `__GetUserKData` reads a DWORD at the SDK byte offset
from the live thread's KData window. API readiness queries report implemented
device providers and preserve cooperative waits for unavailable sets.

CERF SampleApp's TreeView, Header and UpDown classes have independent control
state and rendering. Trees retain Unicode labels, item data, hierarchy,
expansion and selection; headers retain columns and display order; numeric
controls retain signed ranges and edit buddies. Tree selection/expansion and
numeric changes send synchronous parent notifications, including vetoes and
continuation restoration. In CE desktop mode, an unregistered class fails with
error 1407 instead of reentering the application's main WM_CREATE handler.
This is a supported common-control subset, not complete commctrl conformance.

## Build and distribution

```cmd
tools\build-windows.cmd build --release -j 1 -p pocket-cli -p pocket-desktop --features pocket-cli/unicorn --locked
```

See [Android build](ANDROID-BUILD.md) for the pinned Linux SDK/NDK workflow.
`tools/package-wince-profile.py --output profile.zip` produces public templates.
Adding `--image PATH --explorer-bin PATH` produces a private library backup with
the supplied files. Do not publish that private backup as an emulator release.

Source and static FFmpeg reconstruction materials accompany the public binaries.
The xBRZ-containing binary distribution uses GPL-3.0 terms; individual components
retain their own licenses. Keep the permanent Android signing key outside Git
and reuse it for subsequent releases.
