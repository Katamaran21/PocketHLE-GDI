"""Build a private history fix for the user's existing portable Explorer port.

The supplied image, portable sources and original binaries remain read-only.
This is an application fix, not a replacement for PocketHLE's CE API handlers.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil


REPO = Path(__file__).resolve().parents[1]
DEFAULT_SOURCE = Path(r'D:\DownloadNEW\DreamcastCE212\portable-wm-explorer')


def inside(parent, child):
    return child == parent or parent in child.parents


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=DEFAULT_SOURCE)
    parser.add_argument('--output', type=Path, default=REPO / 'ce-data/portable-explorer')
    parser.add_argument('--profile', type=Path, default=REPO / 'profiles/wince6.json')
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    output = args.output.resolve()
    private = (REPO / 'ce-data').resolve()
    if not inside(private, output) or inside(source, output) or inside(output, source):
        raise SystemExit('Output must be private PocketHLE ce-data outside the original port')
    source_file = source / 'src/portable/folderbrowser.cpp'
    text = source_file.read_text(encoding='latin-1')
    old = 'if (m_index >= 0 && ILIsEqual(m_history[m_index], pidl)) return;'
    new = 'if (m_index >= 0 && sameLocation(m_history[m_index], pidl)) return;'
    if text.count(old) != 1:
        raise SystemExit('The portable browser source has changed; inspect its history implementation')
    anchor = 'struct Navigation {'
    if text.count(anchor) != 1:
        raise SystemExit('Cannot locate the portable navigation type')
    # The sample desktop CompareIDs compares the first namespace component.
    # History needs the entire parsing path, including every folder component.
    helper = '''BOOL sameLocation(LPCITEMIDLIST left, LPCITEMIDLIST right) {
    while (!ILIsEmpty(left) && !ILIsEmpty(right)) {
        if (ILIsNameSpacePidl(left) != ILIsNameSpacePidl(right) ||
            !ILIsEqualParsingNames(left, right)) return FALSE;
        left = ILNext(left); right = ILNext(right);
    }
    return ILIsEmpty(left) && ILIsEmpty(right);
}

'''
    patched = text.replace(old, new).replace(anchor, helper + anchor)
    builder_path = source / 'tools/build.py'
    spec = importlib.util.spec_from_file_location('portable_builder', builder_path)
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    report = json.loads((source / 'build/compile.json').read_text(encoding='utf-8'))
    objects = [Path(p) for group, paths in report['objects'].items()
               if group.startswith('shell_') for p in paths]
    if not objects or any(not p.is_file() for p in objects):
        raise SystemExit('The original portable build must have its native shell objects available')
    if sum(p.name == 'folderbrowser.obj' for p in objects) != 1:
        raise SystemExit('Expected exactly one portable folder browser object')
    build = output / 'build'
    binary = output / 'bin'
    obj_dir = build / 'obj'
    for directory in (binary, obj_dir):
        directory.mkdir(parents=True, exist_ok=True)
    staged_source = build / 'folderbrowser.cpp'
    staged_source.write_text(patched, encoding='latin-1')
    obj = obj_dir / 'folderbrowser.obj'
    original_portable = source / 'src/portable'
    includes = [original_portable, source / 'src/HPC/CESHELL/INC',
                source / 'src/HPC/INC', source / 'src/HPC/CESHELL/UI',
                builder.CE6 / 'PUBLIC/SHELL/OAK/INC', builder.CE6 / 'PUBLIC/SHELLSDK/SDK/INC',
                builder.CE6 / 'PUBLIC/IE/SDK/INC', builder.CE6 / 'PUBLIC/COMMON/SDK/INC',
                builder.CE6 / 'PUBLIC/COMMON/OAK/INC', builder.CE6 / 'PUBLIC/GDIEX/SDK/INC',
                builder.VS / 'VC/ce/include']
    command = [builder.ARM / 'cl.exe', '/nologo', '/c', '/TP', '/O1', '/Os', '/W3', '/GS-',
               '/Zl', '/Gy', '/GR-', '/QRarch4T', '/QRinterwork-return',
               '/DARM', '/D_ARM_', '/DARMV4I', '/DUNDER_CE', '/DWIN32',
               '/DUNICODE', '/D_UNICODE', '/D_WIN32_WCE=0x500', '/DWINCEOEM=1',
               '/DSHIP_BUILD', '/DNDEBUG', '/D_SHELL32_', '/D_SHLWAPI_',
               '/DPORTABLE_SHELL_BUILD', '/DPORTABLE_NO_REDIRECT']
    command += ['/I' + str(p) for p in includes if p.is_dir()]
    command += ['/FI' + str(original_portable / 'portable.h'), '/Fo' + str(obj), staged_source]
    code, compiler_output, _ = builder.run(command, build / 'compile.log')
    if code:
        raise SystemExit(compiler_output)
    lib_dir = source / 'build/lib'
    libraries = [lib_dir / (name + '.lib') for name in ('uuid', 'ole32', 'oleaut32', 'commctrl', 'coredll')]
    libraries += [builder.VS / 'VC/ce/lib/armv4i/libcmt.lib', lib_dir / 'corelibc.lib']
    command = [builder.ARM / 'link.exe', '/nologo', '/dll', '/machine:THUMB',
               '/subsystem:windowsce,5.00', '/entry:_DllMainCRTStartup', '/nodefaultlib',
               '/opt:ref', '/opt:icf', '/base:0x10000000', '/stack:0x20000,0x1000',
               '/out:' + str(binary / 'pceshell.dll'), '/implib:' + str(build / 'pceshell.lib'),
               '/def:' + str(source / 'build/pceshell.def'), '/map:' + str(build / 'pceshell.map')]
    command += [obj if p.name == 'folderbrowser.obj' else p for p in objects]
    command += [source / 'build/resources/pceshell.res'] + libraries
    code, linker_output, _ = builder.run(command, build / 'link.log')
    if code:
        raise SystemExit(linker_output)
    for name in ('explorer-portable.exe', 'xplorer.exe'):
        original = source / 'build/bin' / name
        if original.is_file():
            shutil.copy2(original, binary / name)
    # The port deliberately refuses to create shell folders in the firmware.
    # Provision its own Favorites/Recent so the original Start menu can include
    # those cascades, with the profile's writable overlay owning their contents.
    profile = json.loads(args.profile.read_text(encoding='utf-8'))
    data = Path(profile['data_dir']).resolve()
    parts = [p for p in profile['app_prefix'].replace('/', '\\').split('\\') if p]
    if not inside(private, data) or any(p in ('.', '..') or ':' in p for p in parts):
        raise SystemExit('The portable profile must keep device data below PocketHLE ce-data')
    application = data.joinpath('fs', *parts)
    for name in ('Desktop', 'Favorites', 'Recent', 'Startup'):
        directory = (application / name).resolve()
        if not inside(data, directory) or inside(source, directory):
            raise SystemExit('The profile application folder escapes private device data')
        directory.mkdir(parents=True, exist_ok=True)
    metadata = {
        'source': str(source_file),
        'source_sha256': hashlib.sha256(source_file.read_bytes()).hexdigest(),
        'private_source_sha256': hashlib.sha256(staged_source.read_bytes()).hexdigest(),
        'dll_sha256': hashlib.sha256((binary / 'pceshell.dll').read_bytes()).hexdigest(),
        'fix': 'History compares every PIDL parsing component, not only the first namespace',
    }
    (output / 'build.json').write_text(json.dumps(metadata, indent=2), encoding='utf-8')
    print('Private portable Explorer:', binary)


if __name__ == '__main__':
    main()
