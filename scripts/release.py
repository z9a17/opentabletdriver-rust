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
COMMON = ['README.md', 'LICENSE', 'LICENSE.LGPL-3.0', 'NOTICE.md', 'driver.example.toml',
          'driver.relative.example.toml', 'driver.plugins.example.toml', 'docs']


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
    packages = ['opentabletdriver-rust', 'otd-ema-filter'] if platform == 'win-x64' else ['otd-linux' if platform == 'linux-x64' else 'otd-macos']
    invocation = [cargo, 'build', '--locked', '--release', '--target', target, '-j4']
    for package in packages:
        invocation += ['-p', package]
    subprocess.run(invocation, cwd=ROOT, check=True)
    binaries = ROOT / 'target' / target / 'release'
    if platform == 'win-x64':
        dotnet = os.environ.get('DOTNET', 'dotnet')
        dotnet_directory = Path(shutil.which(dotnet) or dotnet).resolve().parent
        nethost = os.environ.get('NETHOST_DLL')
        if not nethost:
            packs = list(dotnet_directory.glob('packs/Microsoft.NETCore.App.Host.win-x64/*/runtimes/win-x64/native/nethost.dll'))
            packs = [path for path in packs if re.fullmatch(r'\d+\.\d+\.\d+', path.parts[-5])]
            if not packs:
                raise ValueError('set NETHOST_DLL to the Windows x64 .NET host pack nethost.dll')
            nethost = str(max(packs, key=lambda path: tuple(int(n) for n in path.parts[-5].split('.'))))
        compat = binaries / 'compat'
        subprocess.run([dotnet, 'restore', 'compat/OtdCompat', '--locked-mode', '--nologo'], cwd=ROOT, check=True)
        subprocess.run([dotnet, 'build', 'compat/OtdCompat', '-c', 'Release', '--no-restore', '-o', str(compat), '--nologo'], cwd=ROOT, check=True)
        shutil.copy2(nethost, compat / 'nethost.dll')
        shutil.copy2(ROOT / 'compat/THIRD_PARTY_NOTICES.txt', compat)
        for name in ['LICENSE.txt', 'ThirdPartyNotices.txt']:
            if (dotnet_directory / name).is_file():
                shutil.copy2(dotnet_directory / name, compat / ('DOTNET-' + name))
    license_directory = binaries / 'runtime-licenses'
    license_directory.mkdir(exist_ok=True)
    rust_documentation = Path(command(rustc, '--print', 'sysroot')) / 'share/doc/rust'
    shutil.copy2(rust_documentation / 'COPYRIGHT-library.html', license_directory / 'RUST-COPYRIGHT.html')
    for name in ['MIT.txt', 'Apache-2.0.txt', 'LLVM-exception.txt', 'GCC-exception-3.1.txt', 'GPL-3.0-or-later.txt']:
        shutil.copy2(rust_documentation / 'licenses' / name, license_directory / name)
    if platform == 'linux-x64':
        shutil.copy2(ROOT / 'packaging/linux/MUSL-COPYRIGHT.txt', license_directory)
    elif platform == 'win-x64' and target.endswith('-gnu'):
        for path in (ROOT / 'packaging/windows').glob('*.txt'):
            shutil.copy2(path, license_directory)
    after = source_state()
    if before != after:
        raise ValueError('source changed during the release build; rebuild the final source')
    metadata = dict(after, platform=platform, rust_target=target, rustc=command(rustc, '--version'),
                    binaries={name: digest((binaries / name).read_bytes()) for name in MATRIX[platform]['binaries']})
    metadata['licenses'] = {path.name: digest(path.read_bytes()) for path in sorted(license_directory.iterdir()) if path.is_file()}
    if platform == 'win-x64':
        metadata['compat'] = {path.relative_to(binaries / 'compat').as_posix(): digest(path.read_bytes())
                              for path in sorted((binaries / 'compat').rglob('*')) if path.is_file()}
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
    with tempfile.TemporaryDirectory(prefix='otd-package-') as scratch:
        stage = Path(scratch) / package_name(platform)
        stage.mkdir()
        for name in MATRIX[platform]['binaries']:
            shutil.copy2(binaries / name, stage / name)
            if platform != 'win-x64':
                (stage / name).chmod(0o755)
        for name in COMMON:
            source = ROOT / name
            if source.is_dir():
                shutil.copytree(source, stage / name, ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
            else:
                shutil.copy2(source, stage / name)
        licenses = binaries / 'runtime-licenses'
        if {path.name: digest(path.read_bytes()) for path in sorted(licenses.iterdir()) if path.is_file()} != metadata['licenses']:
            raise ValueError('runtime license files changed since recorded build')
        shutil.copytree(licenses, stage / 'runtime-licenses')
        for backend in ['otd-linux', 'otd-macos']:
            guide_dir = stage / 'crates' / backend
            guide_dir.mkdir(parents=True)
            shutil.copy2(ROOT / 'crates' / backend / 'README.md', guide_dir / 'README.md')
        if platform == 'win-x64':
            compat = (args.compat_dir or binaries / 'compat').resolve()
            for name in ['OtdCompat.dll', 'OtdCompat.runtimeconfig.json', 'OpenTabletDriver.Plugin.dll',
                         'nethost.dll', 'THIRD_PARTY_NOTICES.txt']:
                if not (compat / name).is_file():
                    raise ValueError(f'missing Windows compatibility component: {compat / name}')
            check_binary('win-x64', 'nethost.dll', (compat / 'nethost.dll').read_bytes())
            actual_compat = {path.relative_to(compat).as_posix(): digest(path.read_bytes())
                             for path in sorted(compat.rglob('*')) if path.is_file()}
            if actual_compat != metadata.get('compat'):
                raise ValueError('compatibility bridge changed since recorded build; rebuild Windows')
            shutil.copytree(compat, stage / 'compat')
        elif platform == 'linux-x64':
            shutil.copytree(ROOT / 'packaging/linux', stage / 'setup')
            shutil.copy2(ROOT / 'crates/otd-linux/README.md', stage / 'LINUX.md')
            (stage / 'setup/install.sh').chmod(0o755)
        else:
            shutil.copy2(ROOT / 'crates/otd-macos/README.md', stage / 'MACOS.md')
        (stage / 'BUILD-INFO.json').write_text(json.dumps(metadata, indent=2) + '\n')
        archive = archive_path(destination, platform)
        if MATRIX[platform]['format'] == 'zip':
            with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
                for path in sorted(stage.rglob('*')):
                    if path.is_file():
                        output.write(path, path.relative_to(stage.parent).as_posix())
        else:
            with tarfile.open(archive, 'w:gz') as output:
                output.add(stage, arcname=stage.name)
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
                if member.name.endswith(tuple('/' + name for spec in MATRIX.values() for name in spec['binaries']) + ('/setup/install.sh',)) and member.mode & 0o111 == 0:
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
        metadata = json.loads(files[prefix + 'BUILD-INFO.json'])
        if metadata['version'] != VERSION or metadata['platform'] != platform or metadata['source_commit'] != revision:
            raise ValueError(f'package source/version mismatch: {archive.name}')
        if require_clean and metadata['source_dirty']:
            raise ValueError(f'rebuild {archive.name} from a clean committed checkout')
        if metadata['source_tree_sha256'] != source_state()['source_tree_sha256']:
            raise ValueError(f'source tree mismatch: {archive.name}')
        if metadata['cargo_lock_sha256'] != digest((ROOT / 'Cargo.lock').read_bytes()):
            raise ValueError(f'lockfile mismatch: {archive.name}')
        required = spec['binaries'] + ['README.md', 'LICENSE', 'NOTICE.md']
        if platform == 'win-x64':
            required += ['compat/OtdCompat.dll', 'compat/OpenTabletDriver.Plugin.dll', 'compat/nethost.dll',
                         'compat/OtdCompat.runtimeconfig.json', 'compat/THIRD_PARTY_NOTICES.txt']
        elif platform == 'linux-x64':
            required += ['setup/install.sh', 'setup/70-opentabletdriver-rust.rules', 'LINUX.md']
        else:
            required += ['MACOS.md']
        for name in required:
            if prefix + name not in files:
                raise ValueError(f'{archive.name} is missing {name}')
        for name in spec['binaries']:
            binary = files[prefix + name]
            check_binary(platform, name, binary)
            if digest(binary) != metadata['binaries'][name]:
                raise ValueError(f'binary provenance mismatch: {name}')
        for name, expected_hash in metadata['licenses'].items():
            if digest(files[prefix + 'runtime-licenses/' + name]) != expected_hash:
                raise ValueError(f'runtime license mismatch: {name}')
        if platform == 'win-x64':
            check_binary('win-x64', 'nethost.dll', files[prefix + 'compat/nethost.dll'])
            for name, expected_hash in metadata['compat'].items():
                if digest(files[prefix + 'compat/' + name]) != expected_hash:
                    raise ValueError(f'compatibility bridge provenance mismatch: {name}')
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
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'error: {error}\n')


if __name__ == '__main__':
    main()
