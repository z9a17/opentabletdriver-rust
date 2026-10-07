#!/usr/bin/env python3
"""Package and publish one source revision for every required release platform."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path, PurePosixPath
import shutil
import struct
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent
MATRIX = json.loads((ROOT / 'packaging/release-matrix.json').read_text())
MANDATORY = {'win-x64', 'linux-x64', 'macos-x64', 'macos-arm64'}
if set(MATRIX) != MANDATORY:
    raise SystemExit('error: the release matrix must include Windows, Linux and both Mac architectures')
VERSION = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
PROJECT_LICENSES = ['LICENSE', 'LICENSE.LGPL-3.0', 'NOTICE.md']
COMPAT_FILES = ['OtdCompat.dll', 'OtdCompat.runtimeconfig.json', 'OtdCompat.deps.json',
                'OpenTabletDriver.dll', 'OpenTabletDriver.Plugin.dll',
                'OpenTabletDriver.Configurations.dll', 'OpenTabletDriver.Native.dll',
                'OpenTabletDriver.Desktop.dll', 'HidSharpCore.dll',
                'Newtonsoft.Json.dll', 'JetBrains.Annotations.dll', 'MessagePack.dll',
                'MessagePack.Annotations.dll', 'Microsoft.Extensions.DependencyInjection.dll',
                'Microsoft.Extensions.DependencyInjection.Abstractions.dll',
                'Microsoft.NET.StringTools.dll', 'Microsoft.VisualStudio.Threading.dll',
                'Microsoft.VisualStudio.Validation.dll', 'Nerdbank.Streams.dll', 'Octokit.dll',
                'ICSharpCode.SharpZipLib.dll', 'StreamJsonRpc.dll', 'System.CommandLine.dll',
                'System.IO.Pipelines.dll',
                'WaylandNET.dll', 'nethost.dll']
# Preserve the original dependency satellites, not only their English fallback.
COMPAT_FILES += [f'{locale}/{assembly}.resources.dll'
                 for locale in ['cs', 'de', 'es', 'fr', 'it', 'ja', 'ko', 'pl', 'pt-BR',
                                'ru', 'tr', 'zh-Hans', 'zh-Hant']
                 for assembly in ['Microsoft.VisualStudio.Threading', 'Microsoft.VisualStudio.Validation',
                                  'StreamJsonRpc', 'System.CommandLine']]
# Exact runtime DLL closure from the net8.0 restored projects, pinned by both
# packages.lock.json files. This remains an allowlist, not an arbitrary-output glob.
# System.ComponentModel.Annotations resolves from the shared net8.0 framework;
# it is not a copy-local runtime dependency in OtdCompat.deps.json.
BRIDGE_LICENSES = {'DOTNET-BRIDGE-NOTICES.txt': 'compat/THIRD_PARTY_NOTICES.txt',
                  'OTD-DESKTOP-LICENSE.txt': 'compat/UpstreamDesktop/LICENSE',
                  'OTD-DESKTOP-SOURCE.txt': 'compat/UpstreamDesktop/PROVENANCE.md',
                  'OTD-CORE-LICENSE.txt':'compat/UpstreamCore/LICENSE',
                  'OTD-CORE-SOURCE.txt':'compat/UpstreamCore/PROVENANCE.md'}
LINUX_SETUP = ['install.sh', '70-opentabletdriver-rust.rules', 'opentabletdriver-rust.conf', 'generate-rules.py']

RID = {'win-x64':'win-x64','linux-x64':'linux-x64','macos-x64':'osx-x64','macos-arm64':'osx-arm64'}
NETHOST = {'win-x64':'nethost.dll','linux-x64':'libnethost.so','macos-x64':'libnethost.dylib','macos-arm64':'libnethost.dylib'}
UX_PROJECT = {'linux-x64':('compat/UpstreamUX.Gtk/OpenTabletDriver.UX.Gtk.csproj','OpenTabletDriver.UX.Gtk'),
              'macos-x64':('compat/UpstreamUX.MacOS/OpenTabletDriver.UX.MacOS.csproj','OpenTabletDriver.UX.MacOS'),
              'macos-arm64':('compat/UpstreamUX.MacOS/OpenTabletDriver.UX.MacOS.csproj','OpenTabletDriver.UX.MacOS')}
UX_LICENSES = {'OTD-UX-LICENSE.txt':'compat/UpstreamUX/LICENSE','OTD-UX-SOURCE.txt':'compat/UpstreamUX/PROVENANCE.md',
               'OTD-CONSOLE-LICENSE.txt':'compat/UpstreamConsole/LICENSE',
               'OTD-CONSOLE-SOURCE.txt':'compat/UpstreamConsole/PROVENANCE.md',
               **{name:'packaging/licenses/'+name for name in ['ETO-LICENSE.txt','MONOMAC-LICENSE.txt','GTKSHARP-LICENSE.txt','UX-SOURCE-NOTICES.txt']}}
CONSOLE_PROJECT = ('compat/UpstreamConsole/OpenTabletDriver.Console.csproj','OpenTabletDriver.Console',False)


def safe_component(name):
    path=PurePosixPath(name)
    if not name or not path.parts or path.is_absolute() or '..' in path.parts or '\\' in name or ':' in name:
        raise ValueError(f'unsafe managed component path: {name}')
    return path


def managed_assets(directory, assembly, platform, apphost=False):
    """Use the published deps graph, excluding symbols/build output/legacy launchers."""
    names=managed_assets_from_reader(lambda name:(directory/name).read_bytes(),'',assembly,platform,apphost)
    for name in names:
        if not (directory/name).is_file():raise ValueError(f'missing published {assembly} runtime dependency: {name}')
    if apphost:check_binary(platform,assembly,(directory/assembly).read_bytes())
    for name in names:
        if name.endswith(('.so','.dylib')):check_binary(platform,name,(directory/name).read_bytes())
    return names


def native_host(dotnet_directory,platform):
    override=os.environ.get('NETHOST_'+RID[platform].upper().replace('-','_'))
    override=override or (os.environ.get('NETHOST_DLL') if platform=='win-x64' else os.environ.get('NETHOST_LIBRARY'))
    if override:path=Path(override)
    else:
        package='microsoft.netcore.app.host.'+RID[platform]
        cache=Path(os.environ.get('NUGET_PACKAGES',str(Path.home()/'.nuget/packages')))
        candidates=list(dotnet_directory.glob('packs/Microsoft.NETCore.App.Host.'+RID[platform]+'/*/runtimes/'+RID[platform]+'/native/'+NETHOST[platform]))
        candidates+=list((cache/package).glob('*/runtimes/'+RID[platform]+'/native/'+NETHOST[platform]))
        candidates=[path for path in candidates if re.fullmatch(r'\d+\.\d+\.\d+',path.parts[-5])]
        if not candidates:raise ValueError(f'supply NETHOST_{RID[platform].upper().replace("-","_")} from the target .NET host pack')
        path=max(candidates,key=lambda path:tuple(int(part) for part in path.parts[-5].split('.')))
    check_binary(platform,NETHOST[platform],path.read_bytes())
    return path


def publish_managed(dotnet,project,platform,output,apphost):
    # Disable Eto's old x64-only Mono launcher/bundle target. SDK apphosts are
    # generated from the selected RID; the native ui command starts those.
    properties=['-p:MacBuildBundle=false','-p:MacAutoPublishBundle=false','-p:PublishTrimmed=false',
                '-p:UseAppHost='+str(apphost).lower()]
    # restore --runtime overrides RuntimeIdentifiers and invalidates a lock
    # containing several RIDs. Restore the declared graph once, then publish
    # one of the already locked runtime targets without another restore.
    subprocess.run([dotnet,'restore',project,'--locked-mode','--nologo',*properties],cwd=ROOT,check=True)
    subprocess.run([dotnet,'publish',project,'-c','Release','--no-restore','-r',RID[platform],
                    '--self-contained','false','-o',str(output),'--nologo',*properties],cwd=ROOT,check=True)


def build_managed(binaries,platform):
    dotnet=os.environ.get('DOTNET','dotnet')
    dotnet_directory=Path(shutil.which(dotnet) or dotnet).resolve().parent
    compat=binaries/'compat'
    publish_managed(dotnet,'compat/OtdCompat/OtdCompat.csproj',platform,compat,False)
    host=native_host(dotnet_directory,platform)
    shutil.copy2(host,compat/NETHOST[platform])
    compat_names=managed_assets(compat,'OtdCompat',platform)|{NETHOST[platform]}
    required=set(COMPAT_FILES)-{'nethost.dll'}
    if not required<=compat_names:raise ValueError(f'managed bridge closure is missing pinned public dependencies: {sorted(required-compat_names)}')
    desktop={}
    if platform!='win-x64':
        destination=binaries/'desktop';destination.mkdir(exist_ok=True)
        projects=[(*UX_PROJECT[platform],True),('compat/NativeDaemonLauncher/OpenTabletDriver.Daemon.csproj','OpenTabletDriver.Daemon',True),
                  ('compat/ArchiveTools/OtdArchiveTools.csproj','OtdArchiveTools',False),CONSOLE_PROJECT]
        for project,assembly,apphost in projects:
            output=binaries/('publish-'+assembly)
            publish_managed(dotnet,project,platform,output,apphost)
            for name in managed_assets(output,assembly,platform,apphost):
                data=(output/name).read_bytes()
                if name in desktop and desktop[name]!=digest(data):raise ValueError(f'conflicting desktop dependency: {name}')
                target=destination/name;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(data)
                desktop[name]=digest(data)
    return dotnet_directory,{name:digest((compat/name).read_bytes()) for name in sorted(compat_names)},desktop


def validate_managed_inventory(platform,compat,desktop,read):
    expected=managed_assets_from_reader(read,'data/compat/','OtdCompat',platform,False)|{NETHOST[platform]}
    if set(compat)!=expected:raise ValueError('managed bridge inventory differs from its actual published dependency graph')
    if not (set(COMPAT_FILES)-{'nethost.dll'})<=expected:raise ValueError('managed bridge lacks pinned public dependencies')
    check_binary(platform,NETHOST[platform],read('data/compat/'+NETHOST[platform]))
    if platform=='win-x64':
        if desktop:raise ValueError('Windows has unexpected Unix frontend payload')
    else:
        expected=set()
        for assembly,apphost in [(UX_PROJECT[platform][1],True),('OpenTabletDriver.Daemon',True),('OtdArchiveTools',False),CONSOLE_PROJECT[1:]]:
            expected|=managed_assets_from_reader(read,'',assembly,platform,apphost)
        if set(desktop)!=expected:raise ValueError('desktop inventory differs from actual published dependency graphs')
    for prefix,inventory in [('data/compat/',compat),('',desktop)]:
        for name,expected_hash in inventory.items():
            safe_component(name)
            data=read(prefix+name)
            if digest(data)!=expected_hash:raise ValueError(f'managed provenance mismatch: {name}')
            if name.endswith(('.so','.dylib')) or (prefix=='' and name in [UX_PROJECT.get(platform,('',None))[1],'OpenTabletDriver.Daemon']):
                check_binary(platform,name,data)


def managed_assets_from_reader(read,prefix,assembly,platform,apphost):
    # Same graph resolution for recorded folders and archives; no extraction.
    deps=json.loads(read(prefix+assembly+'.deps.json'))
    target=deps['runtimeTarget']['name']
    if not target.endswith('/'+RID[platform]):raise ValueError('managed dependency graph has the wrong runtime target')
    names={assembly+'.dll',assembly+'.deps.json',assembly+'.runtimeconfig.json'}
    fallback={RID[platform],RID[platform].split('-')[0]}
    if platform!='win-x64':fallback.add('unix')
    for library in deps['targets'][target].values():
        for group in ['runtime','native','resources','runtimeTargets']:
            for asset,info in library.get(group,{}).items():
                asset=safe_component(asset)
                if asset.name=='_._' or (group=='runtimeTargets' and info.get('rid') not in fallback):continue
                names.add(str(safe_component((info.get('locale') or asset.parent.name)+'/'+asset.name)) if group=='resources' else asset.name)
    if apphost:
        names.add(assembly)
        # Apply the same target-architecture fence to folder and archive graph
        # readers, including future apphosts added to the desktop payload.
        check_binary(platform,assembly,read(prefix+assembly))
    return names


def quickstart(platform):
    backend = 'windows' if platform == 'win-x64' else 'linux' if platform == 'linux-x64' else 'macos'
    return ROOT / 'packaging' / f'README.{backend}.md'


def platform_guide(backend):
    text = (ROOT / 'crates' / backend / 'README.md').read_bytes()
    return text.replace(b'](../../', f'](https://github.com/z9a17/opentabletdriver-rust/blob/v{VERSION}/'.encode())


def pe_resources(data):
    """Read resource bytes without loading or executing a Windows image."""
    pe = struct.unpack_from('<I', data, 60)[0]
    sections, optional_size = struct.unpack_from('<H', data, pe + 6)[0], struct.unpack_from('<H', data, pe + 20)[0]
    optional = pe + 24
    if struct.unpack_from('<H', data, optional)[0] != 0x20b:
        raise ValueError('expected PE32+ resources')
    resource_rva = struct.unpack_from('<I', data, optional + 112 + 16)[0]

    def file_offset(rva, size):
        for index in range(sections):
            offset = optional + optional_size + index * 40
            virtual_size, address, raw_size, raw_offset = struct.unpack_from('<IIII', data, offset + 8)
            if address <= rva and rva + size <= address + min(virtual_size, raw_size):
                result = raw_offset + rva - address
                if result + size <= len(data):
                    return result
        raise ValueError('invalid PE resource address')

    base = file_offset(resource_rva, 16)
    result = {}

    def visit(offset, path):
        if len(path) > 2:
            raise ValueError('invalid PE resource tree')
        named, ids = struct.unpack_from('<HH', data, base + offset + 12)
        for index in range(named + ids):
            name, target = struct.unpack_from('<II', data, base + offset + 16 + index * 8)
            if name & 0x80000000:
                continue
            if target & 0x80000000:
                visit(target & 0x7fffffff, path + (name,))
            else:
                rva, size = struct.unpack_from('<II', data, base + target)
                start = file_offset(rva, size)
                result[path + (name,)] = data[start:start + size]

    visit(0, ())
    return result


def verify_windows_icon(data):
    resources = pe_resources(data)
    groups = [(key, value) for key, value in resources.items() if key[:2] == (14, 1)]
    if len(groups) != 1:
        raise ValueError('Windows executable must embed application icon resource 1')
    group_key, group = groups[0]
    icon = (ROOT / 'resources/opentabletdriver.ico').read_bytes()
    count = struct.unpack_from('<H', icon, 4)[0]
    if group[:6] != icon[:6] or len(group) != 6 + count * 14:
        raise ValueError('Windows application icon group differs from the source asset')
    for index in range(count):
        source = 6 + index * 16
        target = 6 + index * 14
        size, offset = struct.unpack_from('<II', icon, source + 8)
        resource_id = struct.unpack_from('<H', group, target + 12)[0]
        if group[target:target + 12] != icon[source:source + 12] or resources.get((3, resource_id, group_key[2])) != icon[offset:offset + size]:
            raise ValueError('Windows application icon image differs from the source asset')


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def package_name(platform):
    return f'opentabletdriver-rust-v{VERSION}-{platform}'


def archive_path(directory, platform):
    return directory / f'{package_name(platform)}.{MATRIX[platform]["format"]}'


def check_binary(platform, name, data):
    """Reject wrong OS/architecture or empty/stale executables before packaging."""
    if platform == 'linux-x64':
        if len(data) < 20 or data[:6] != b'\x7fELF\x02\x01' or struct.unpack_from('<H', data, 18)[0] != 62:
            raise ValueError(f'{name}: expected Linux x86-64 ELF')
    elif platform.startswith('macos-'):
        cpu = 0x01000007 if platform == 'macos-x64' else 0x0100000C
        if len(data) < 32 or data[:4] != b'\xcf\xfa\xed\xfe' or struct.unpack_from('<I', data, 4)[0] != cpu:
            raise ValueError(f'{name}: expected {platform} Mach-O')
    else:
        if len(data) < 64 or data[:2] != b'MZ':
            raise ValueError(f'{name}: expected Windows PE')
        offset = struct.unpack_from('<I', data, 60)[0]
        if len(data) < offset + 6 or data[offset:offset + 4] != b'PE\0\0' or struct.unpack_from('<H', data, offset + 4)[0] != 0x8664:
            raise ValueError(f'{name}: expected Windows x64 PE')
    if name.startswith('opentabletdriver-') and VERSION.encode() not in data:
        raise ValueError(f'{name}: release version {VERSION} is missing from the executable')
    if platform == 'win-x64' and name.startswith('opentabletdriver-') and name.endswith('.exe'):
        verify_windows_icon(data)


def source_state():
    dirty = bool(command('git', 'status', '--porcelain'))
    hasher = hashlib.sha256()
    names = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=ROOT)
    for name in sorted(set(names.split(b'\0')) - {b''}):
        path = ROOT / os.fsdecode(name)
        hasher.update(name + b'\0')
        hasher.update(path.read_bytes() if path.is_file() else b'<deleted>')
    tree_hash = hasher.hexdigest() if dirty else digest(command('git', 'rev-parse', 'HEAD^{tree}').encode())
    return {'version': VERSION, 'source_commit': command('git', 'rev-parse', 'HEAD'),
            'source_dirty': dirty,
            'cargo_lock_sha256': digest((ROOT / 'Cargo.lock').read_bytes()),
            'source_tree_sha256': tree_hash}


def build(args):
    platform = args.platform
    target = args.rust_target or MATRIX[platform]['target']
    permitted = {MATRIX[platform]['target']}
    if platform == 'win-x64':
        permitted.add('x86_64-pc-windows-msvc')
    if target not in permitted:
        raise ValueError(f'{target} does not match {platform}')
    before = source_state()
    cargo = os.environ.get('CARGO', 'cargo')
    rustc = os.environ.get('RUSTC', 'rustc')
    packages = ['opentabletdriver-rust'] if platform == 'win-x64' else ['otd-linux' if platform == 'linux-x64' else 'otd-macos']
    invocation = [cargo, 'build', '--locked', '--release', '--target', target, '-j4']
    for package in packages:
        invocation += ['-p', package]
    subprocess.run(invocation, cwd=ROOT, check=True)
    binaries = ROOT / 'target' / target / 'release'
    dotnet_directory,compat_metadata,desktop_metadata=build_managed(binaries,platform)
    license_directory = binaries / 'runtime-licenses'
    license_directory.mkdir(exist_ok=True)
    rust_documentation = Path(command(rustc, '--print', 'sysroot')) / 'share/doc/rust'
    shutil.copy2(rust_documentation / 'COPYRIGHT-library.html', license_directory / 'RUST-COPYRIGHT.html')
    for name in ['MIT.txt', 'Apache-2.0.txt', 'LLVM-exception.txt', 'GCC-exception-3.1.txt', 'GPL-3.0-or-later.txt']:
        shutil.copy2(rust_documentation / 'licenses' / name, license_directory / name)
    for name in PROJECT_LICENSES:
        shutil.copy2(ROOT / name, license_directory / name)
    for packaged_name,source_name in BRIDGE_LICENSES.items():
        shutil.copy2(ROOT/source_name,license_directory/packaged_name)
    if platform!='win-x64':
        for packaged_name,source_name in UX_LICENSES.items():
            shutil.copy2(ROOT/source_name,license_directory/packaged_name)
    for name in ['LICENSE.txt', 'ThirdPartyNotices.txt']:
        source = dotnet_directory / name
        if not source.is_file():
            raise ValueError(f'missing .NET SDK license: {source}')
        shutil.copy2(source, license_directory / ('DOTNET-' + name))
    if platform == 'win-x64' and target.endswith('-gnu'):
        for path in (ROOT / 'packaging/windows').glob('*.txt'):
            shutil.copy2(path, license_directory)
    after = source_state()
    if before != after:
        raise ValueError('source changed during the release build; rebuild the final source')
    metadata = dict(after, platform=platform, rust_target=target, rustc=command(rustc, '--version'),
                    binaries={name: digest((binaries / name).read_bytes()) for name in MATRIX[platform]['binaries']})
    metadata['licenses'] = {path.name: digest(path.read_bytes()) for path in sorted(license_directory.iterdir()) if path.is_file()}
    metadata['compat']=compat_metadata
    metadata['desktop']=desktop_metadata
    metadata['package_layout'] = 3
    (binaries / 'OTD-BUILD.json').write_text(json.dumps(metadata, indent=2) + '\n')
    args.bin_dir = binaries
    args.compat_dir = None
    make_package(args)


def make_package(args):
    platform = args.platform
    binaries = args.bin_dir.resolve()
    destination = args.output.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    metadata = json.loads((binaries / 'OTD-BUILD.json').read_text())
    current = source_state()
    if any(metadata.get(key) != value for key, value in current.items()) or metadata.get('platform') != platform:
        raise ValueError('build provenance differs from source; use release.py build to compile and record this revision')
    for name in MATRIX[platform]['binaries']:
        if digest((binaries / name).read_bytes()) != metadata['binaries'][name]:
            raise ValueError(f'changed binary since build: {name}')
        check_binary(platform, name, (binaries / name).read_bytes())
    scratch_root=ROOT/'target/package-scratch';scratch_root.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='otd-package-',dir=scratch_root) as scratch:
        stage = Path(scratch) / package_name(platform)
        stage.mkdir()
        data_directory = stage / 'data'
        data_directory.mkdir()
        for name in MATRIX[platform]['binaries']:
            shutil.copy2(binaries / name, stage / name)
            if platform != 'win-x64':
                (stage / name).chmod(0o755)
        shutil.copy2(quickstart(platform), stage / 'README.md')
        licenses = binaries / 'runtime-licenses'
        if {path.name: digest(path.read_bytes()) for path in sorted(licenses.iterdir()) if path.is_file()} != metadata['licenses']:
            raise ValueError('runtime license files changed since recorded build')
        shutil.copytree(licenses, data_directory / 'licenses')
        compat=(args.compat_dir or binaries/'compat').resolve()
        desktop=binaries/'desktop'
        def read_component(name):
            return ((compat/name.removeprefix('data/compat/')) if name.startswith('data/compat/') else desktop/name).read_bytes()
        validate_managed_inventory(platform,metadata['compat'],metadata['desktop'],read_component)
        for prefix,source,inventory in [('data/compat/',compat,metadata['compat']),('',desktop,metadata['desktop'])]:
            for name in inventory:
                target=stage/prefix/name;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source/name,target)
        if platform == 'linux-x64':
            (stage / 'setup').mkdir()
            for name in LINUX_SETUP:
                # Git on Windows may check text out with CRLF. Linux shebangs,
                # shell commands and setup configuration must retain Unix lines.
                source = (ROOT / 'packaging/linux' / name).read_bytes()
                (stage / 'setup' / name).write_bytes(source.replace(b'\r\n', b'\n'))
            (data_directory / 'LINUX.md').write_bytes(platform_guide('otd-linux'))
            (stage / 'setup/install.sh').chmod(0o755)
        elif platform.startswith('macos-'):
            (data_directory / 'MACOS.md').write_bytes(platform_guide('otd-macos'))
        (data_directory / 'BUILD-INFO.json').write_text(json.dumps(metadata, indent=2) + '\n')
        archive = archive_path(destination, platform)
        if MATRIX[platform]['format'] == 'zip':
            with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
                for path in sorted(stage.rglob('*')):
                    if path.is_file():
                        output.write(path, path.relative_to(stage.parent).as_posix())
        else:
            executables = set(MATRIX[platform]['binaries'])|{UX_PROJECT[platform][1],'OpenTabletDriver.Daemon'}
            if platform == 'linux-x64':
                executables.update({'setup/install.sh', 'setup/generate-rules.py'})

            def unix_member(member):
                relative = PurePosixPath(member.name).relative_to(stage.name).as_posix()
                member.mode = 0o755 if member.isdir() or relative in executables else 0o644
                member.uid = member.gid = 0
                member.uname = member.gname = ''
                return member

            with tarfile.open(archive, 'w:gz') as output:
                output.add(stage, arcname=stage.name, filter=unix_member)
    checksum = digest(archive.read_bytes())
    Path(str(archive) + '.sha256').write_text(f'{checksum}  {archive.name}\n')
    print(archive)
    print(f'SHA256: {checksum}')


def read_archive(path):
    files = {}
    if path.suffix == '.zip':
        with zipfile.ZipFile(path) as archive:
            if archive.testzip() is not None:
                raise ValueError(f'corrupt ZIP: {path}')
            for member in archive.infolist():
                if not member.is_dir():
                    if member.filename in files:
                        raise ValueError(f'duplicate ZIP member: {member.filename}')
                    files[member.filename] = archive.read(member)
    else:
        with tarfile.open(path, 'r:gz') as archive:
            for member in archive:
                if member.isdir():
                    continue
                if not member.isfile() or member.name in files:
                    raise ValueError(f'unsafe or duplicate tar member: {member.name}')
                if member.name.endswith(tuple('/' + name for spec in MATRIX.values() for name in spec['binaries']) + ('/setup/install.sh', '/setup/generate-rules.py','/OpenTabletDriver.UX.Gtk','/OpenTabletDriver.UX.MacOS','/OpenTabletDriver.Daemon')) and member.mode & 0o111 != 0o111:
                    raise ValueError(f'package executable mode missing: {member.name}')
                stream = archive.extractfile(member)
                if stream is None:
                    raise ValueError(f'unreadable tar member: {member.name}')
                files[member.name] = stream.read()
    for name in files:
        relative = PurePosixPath(name)
        if relative.is_absolute() or '..' in relative.parts or '\\' in name:
            raise ValueError(f'unsafe archive member: {name}')
    return files


def verify(directory, require_clean):
    if require_clean and command('git', 'status', '--porcelain'):
        raise ValueError('commit all source changes before building publishable packages')
    revision = command('git', 'rev-parse', 'HEAD')
    assets = []
    for platform, spec in MATRIX.items():
        archive = archive_path(directory, platform)
        sidecar = Path(str(archive) + '.sha256')
        actual = digest(archive.read_bytes())
        if sidecar.read_text().strip() != f'{actual}  {archive.name}':
            raise ValueError(f'checksum mismatch: {archive.name}')
        files = read_archive(archive)
        prefix = package_name(platform) + '/'
        if any(not name.startswith(prefix) for name in files):
            raise ValueError(f'wrong package root: {archive.name}')
        metadata = json.loads(files[prefix + 'data/BUILD-INFO.json'])
        if metadata.get('package_layout') != 3:
            raise ValueError(f'unsupported package layout: {archive.name}')
        if metadata['version'] != VERSION or metadata['platform'] != platform or metadata['source_commit'] != revision:
            raise ValueError(f'package source/version mismatch: {archive.name}')
        if require_clean and metadata['source_dirty']:
            raise ValueError(f'rebuild {archive.name} from a clean committed checkout')
        if metadata['source_tree_sha256'] != source_state()['source_tree_sha256']:
            raise ValueError(f'source tree mismatch: {archive.name}')
        if metadata['cargo_lock_sha256'] != digest((ROOT / 'Cargo.lock').read_bytes()):
            raise ValueError(f'lockfile mismatch: {archive.name}')
        required = spec['binaries'] + ['README.md', 'data/BUILD-INFO.json']
        required += ['data/licenses/' + name for name in metadata['licenses']]
        required += ['data/compat/'+name for name in metadata['compat']]
        required += list(metadata['desktop'])
        required += ['data/licenses/'+name for name in BRIDGE_LICENSES]
        required += ['data/licenses/DOTNET-LICENSE.txt','data/licenses/DOTNET-ThirdPartyNotices.txt']
        if platform!='win-x64':required += ['data/licenses/'+name for name in UX_LICENSES]
        if platform == 'linux-x64':
            required += ['setup/' + name for name in LINUX_SETUP] + ['data/LINUX.md']
        elif platform.startswith('macos-'):
            required += ['data/MACOS.md']
        required=list(dict.fromkeys(required))
        for name in required:
            if prefix + name not in files:
                raise ValueError(f'{archive.name} is missing {name}')
        if set(files) != {prefix + name for name in required}:
            raise ValueError(f'unexpected files in runtime package: {archive.name}')
        if files[prefix + 'README.md'] != quickstart(platform).read_bytes():
            raise ValueError(f'quick-start guide mismatch: {archive.name}')
        if platform == 'linux-x64' and files[prefix + 'data/LINUX.md'] != platform_guide('otd-linux'):
            raise ValueError('Linux platform guide mismatch')
        if platform.startswith('macos-') and files[prefix + 'data/MACOS.md'] != platform_guide('otd-macos'):
            raise ValueError('macOS platform guide mismatch')
        for name in PROJECT_LICENSES:
            if files[prefix + 'data/licenses/' + name] != (ROOT / name).read_bytes():
                raise ValueError(f'project license/notice mismatch: {archive.name}')
        for name in spec['binaries']:
            binary = files[prefix + name]
            check_binary(platform, name, binary)
            if digest(binary) != metadata['binaries'][name]:
                raise ValueError(f'binary provenance mismatch: {name}')
        for name, expected_hash in metadata['licenses'].items():
            if digest(files[prefix + 'data/licenses/' + name]) != expected_hash:
                raise ValueError(f'runtime license mismatch: {name}')
        for packaged_name,source_name in {**BRIDGE_LICENSES,**(UX_LICENSES if platform!='win-x64' else {})}.items():
            if files[prefix+'data/licenses/'+packaged_name]!=(ROOT/source_name).read_bytes():
                raise ValueError(f'managed license/source notice mismatch: {packaged_name}')
        validate_managed_inventory(platform,metadata['compat'],metadata['desktop'],lambda name:files[prefix+name])
        if platform == 'linux-x64':
            for name in LINUX_SETUP:
                if b'\r\n' in files[prefix + 'setup/' + name]:
                    raise ValueError(f'Linux setup file has Windows line endings: {name}')
        print(f'{platform}: archive, architecture, version, source and SHA256 verified')
        assets.extend([archive, sidecar])
    return assets


def publish(args):
    if not args.notes.is_file():
        raise ValueError('release notes file does not exist')
    assets = verify(args.directory.resolve(), require_clean=True)
    tag = 'v' + VERSION
    revision = command('git', 'rev-parse', 'HEAD')
    # Publishing a partial or mixed-revision release is never an option.
    command('git', 'fetch', 'origin', 'main', '--tags')
    if command('git', 'rev-parse', 'origin/main') != revision:
        raise ValueError('publish from the exact merged origin/main revision')
    remote = command('git', 'ls-remote', '--tags', 'origin', f'refs/tags/{tag}')
    if remote:
        if command('git', 'rev-list', '-n1', tag) != revision:
            raise ValueError(f'{tag} already points at another revision; never rewrite a release tag')
    else:
        if subprocess.run(['git', 'show-ref', '--verify', '--quiet', f'refs/tags/{tag}'], cwd=ROOT).returncode == 0:
            raise ValueError(f'local tag {tag} already exists; inspect it before publishing')
        subprocess.run(['git', 'tag', '-a', tag, '-m', f'Release {VERSION}'], cwd=ROOT, check=True)
        subprocess.run(['git', 'push', 'origin', tag], cwd=ROOT, check=True)
    subprocess.run(['gh', 'release', 'create', tag, '--verify-tag', '--draft', '--title',
                    f'v{VERSION}: Windows, Linux and macOS', '--notes-file', str(args.notes.resolve()),
                    *map(str, assets)], cwd=ROOT, check=True)
    uploaded = json.loads(command('gh', 'release', 'view', tag, '--json', 'assets'))['assets']
    expected = {asset.name: 'sha256:' + digest(asset.read_bytes()) for asset in assets}
    if {asset['name']: asset['digest'] for asset in uploaded} != expected:
        raise ValueError('uploaded asset digest mismatch; release remains a draft')
    subprocess.run(['gh', 'release', 'edit', tag, '--draft=false', '--latest'], cwd=ROOT, check=True)
    print(command('gh', 'release', 'view', tag, '--json', 'url', '--jq', '.url'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    for action in ['build', 'package']:
        package = commands.add_parser(action, help='build and record provenance' if action == 'build' else 'package recorded build binaries')
        package.add_argument('--platform', choices=MATRIX, required=True)
        package.add_argument('--output', type=Path, default=ROOT / 'target/releases')
        if action == 'package':
            package.add_argument('--bin-dir', type=Path, required=True)
            package.add_argument('--compat-dir', type=Path)
        else:
            package.add_argument('--rust-target')
    for action in ['verify', 'publish']:
        sub = commands.add_parser(action, help='require every platform in the release matrix')
        sub.add_argument('--directory', type=Path, default=ROOT / 'target/releases')
        if action == 'publish':
            sub.add_argument('--notes', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.action == 'build':
            build(args)
        elif args.action == 'package':
            make_package(args)
        elif args.action == 'verify':
            verify(args.directory.resolve(), require_clean=True)
        else:
            publish(args)
    except (OSError, ValueError, KeyError, struct.error, subprocess.CalledProcessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
