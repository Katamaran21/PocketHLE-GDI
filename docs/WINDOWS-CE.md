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

The profile ZIP starts the original `Windows/explorer.exe` through
`profiles/wince6-native.json`. Place your filesystem under `wince6/fs`, beside
the `profiles` directory. Put the Windows emulator
beside `run-wince6-gui.cmd`, or pass its path as the script's first argument.
Paths in a JSON profile file are resolved relative to that file's directory.

```cmd
pockethle-gui.exe --ce-profile profiles\wince6-native.json --windowed
```

`profiles/wince6.json` selects the workstation's original image Explorer and
mounts its CERF ARM applications at `\CERF\`. For a different location, use
the native template or the GUI's folder selectors. The `mounts` list stores
`guest_prefix` and `host_dir`; paths in profile files are relative to that file,
while library settings resolve them against the library root. Windows and
Android share this contract, and children inherit the parent's live namespace.
Extra application folders use writable device overlays, preserving their sources.

Portable Explorer remains a separate profile and `run-wince6-portable.cmd`
launcher. Its optional files belong in `portable-explorer/bin`; its old local
configuration is preserved in `profiles/wince6-portable-local.json`.

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
Native child windows hosted inside built-in controls participate in the same
composition. SampleApp's tab pages retain their controls, follow ancestor
visibility and clip to the tab and main window. Default painting and BeginPaint
erase a real window's own backing surface; they do not clear the shared screen
over already composed Control Panel icons or neighbouring windows.

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
Original Explorer registers its own desktop and taskbar windows. These keep
their normal OS desktop parent, participate in desktop/taskbar composition and
suppress the fallback PocketPC strip while the registered taskbar exists.
Destroying a registered window removes its registration. `SignalStarted` records
the shell's startup identifier; it does not boot OEM services or drivers.
Bounded `StringCchCopyNExW`/`CatNExW` and their byte-counted aliases use the
CE seven-argument ABI, return end/remaining pointers and do not read beyond
the requested source component. The original shell uses these for paths.
Unsigned CE long-to-text conversions preserve all 32 bits for shell file sizes.
Both FILETIME/local-FILETIME directions preserve 100-ns precision in the virtual
UTC timezone.

The session clipboard stores Unicode/ANSI text, DIBs and memory-backed custom
formats such as Explorer's file-copy packets. Children share owned snapshots,
while GetClipboardData returns a handle in the reader's own heap and
GetClipboardDataAlloc returns a separately owned LocalFree-compatible copy.
Open/close ownership is per thread, enumeration distinguishes completion from
errors, and stale local copies are released when contents change. Delayed
rendering and clipboard GDI-object formats remain unsupported.

The original Explorer's recycle database uses typed CEDB properties, including
Unicode names, integers, timestamps and binary data. Databases and mounted
volumes persist through the private guest filesystem, with independent open
cursors, bounded buffers, sort keys and unique-key checks. A failed read-size
negotiation preserves the cursor; closing a database releases its handle before
volume unmount. Gizmondo contacts retain their existing separate store. These
volumes use PocketHLE's storage format; pre-existing foreign database files are
rejected without being overwritten.

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
Adding `--image PATH` produces a private backup with the original Explorer.
`--cerf-apps PATH` includes the additional ARM applications and `--explorer-bin
PATH` optionally includes Portable Explorer as a separate library entry.
Do not publish that private backup as an emulator release.

Source and static FFmpeg reconstruction materials accompany the public binaries.
The xBRZ-containing binary distribution uses GPL-3.0 terms; individual components
retain their own licenses. Keep the permanent Android signing key outside Git
and reuse it for subsequent releases.
