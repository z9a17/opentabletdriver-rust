#!/usr/bin/env bash
# Setup is opt-in. No global wacom/hid_uclogic blacklist is installed.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
rules=/etc/udev/rules.d/70-opentabletdriver-rust.rules
modules=/etc/modules-load.d/opentabletdriver-rust.conf
state_dir=/run/opentabletdriver-rust-detached
setup_dir=/var/lib/opentabletdriver-rust-setup

usage() {
  cat <<'HELP'
Usage: install.sh install|uninstall|status|ignore-input VID PID|restore

install    Install this application's hidraw/USB/uinput permissions and load uinput.
           Replug the tablet afterwards. Run the driver as your normal user.
uninstall  Remove only unchanged setup files owned by this application.
status     Show setup files, uinput access and current tablet HID kernel drivers.
ignore-input  Opt in to a libinput ignore rule for physical input devices with
              the given four-digit hexadecimal USB VID/PID. Replug afterwards.
              Kernel bindings and hidraw access remain available.
restore    Remove unchanged ignore rules created here, then replug the tablet.
           Also restore any original kernel bindings recorded by older scripts.

Run commands that change system state with sudo. Setup does not install a daemon,
start the driver, change compositor settings or modify another driver's files.
HELP
}

require_root() {
  if (( EUID != 0 )); then
    echo 'This setup operation needs root. Invoke this script with sudo.' >&2
    exit 1
  fi
}

install_owned() {
  local source=$1 destination=$2 recorded digest
  recorded="$setup_dir/${destination##*/}.sha256"
  if [[ -L $destination || -L $recorded ]]; then
    echo "Refusing symlink setup file: $destination" >&2; exit 1
  fi
  if [[ -e $destination ]] && ! cmp -s -- "$source" "$destination" && ! unchanged_owned "$destination"; then
    echo "Refusing to overwrite a different file: $destination" >&2
    exit 1
  fi
  install -D -m 644 -- "$source" "$destination"
  digest=$(sha256sum -- "$destination"); digest=${digest%% *}
  printf '%s\n' "$digest" > "$recorded"
}

unchanged_owned() {
  local destination=$1 recorded digest expected
  recorded="$setup_dir/${destination##*/}.sha256"
  if [[ ! -f $recorded || -L $recorded || -L $destination ]]; then return 1; fi
  read -r expected < "$recorded"
  digest=$(sha256sum -- "$destination"); digest=${digest%% *}
  [[ $digest == "$expected" ]]
}

remove_owned() {
  local source=$1 destination=$2
  if [[ ! -e $destination ]]; then return; fi
  if [[ -L $destination ]] || { ! cmp -s -- "$source" "$destination" && ! unchanged_owned "$destination"; }; then
    echo "Preserving changed file: $destination" >&2
    return
  fi
  rm -- "$destination"
  rm -f -- "$setup_dir/${destination##*/}.sha256"
}

restore_devices() {
  local record device driver current
  shopt -s nullglob
  for record in "$state_dir"/*; do
    device=${record##*/}
    read -r driver < "$record"
    if [[ $driver != wacom && $driver != hid_uclogic ]]; then
      echo "Refusing invalid restore record: $record" >&2; exit 1
    fi
    if [[ ! -d /sys/bus/hid/devices/$device ]]; then
      echo "$device was unplugged; no restore needed."
      rm -- "$record"
      continue
    fi
    current=''
    if [[ -L /sys/bus/hid/devices/$device/driver ]]; then
      current=$(readlink -f -- "/sys/bus/hid/devices/$device/driver")
    fi
    if [[ ${current##*/} == "$driver" ]]; then
      rm -- "$record"; continue
    fi
    if [[ -n $current && ${current##*/} != hid-generic ]]; then
      echo "Preserving unexpected driver $current for $device" >&2; exit 1
    fi
    if [[ ${current##*/} == hid-generic ]]; then
      printf '%s' "$device" > /sys/bus/hid/drivers/hid-generic/unbind
    fi
    if ! printf '%s' "$device" > "/sys/bus/hid/drivers/$driver/bind"; then
      echo "Failed to restore $driver for $device. Replug the tablet." >&2; exit 1
    fi
    echo "Restored $driver for $device."
    rm -- "$record"
  done
  rmdir -- "$state_dir" 2>/dev/null || true
}

restore_ignore_rules() {
  local record name destination
  shopt -s nullglob
  for record in "$setup_dir"/71-opentabletdriver-rust-ignore-????-????.rules.sha256; do
    name=${record##*/}
    if [[ ! $name =~ ^71-opentabletdriver-rust-ignore-[[:xdigit:]]{4}-[[:xdigit:]]{4}\.rules\.sha256$ ]]; then
      echo "Preserving unexpected ignore-rule record: $record" >&2; continue
    fi
    destination="/etc/udev/rules.d/${name%.sha256}"
    if [[ ! -e $destination && ! -L $destination ]]; then rm -- "$record"; continue; fi
    if ! unchanged_owned "$destination"; then
      echo "Preserving changed ignore rule: $destination" >&2; continue
    fi
    rm -- "$destination" "$record"
    echo "Removed $destination."
  done
}

command=${1:-help}
case "$command" in
  -h|--help|help) usage ;;
  status)
    for file in "$rules" "$modules" /dev/uinput; do
      if [[ -e $file ]]; then ls -l -- "$file"; else echo "Missing: $file"; fi
    done
    if [[ -w /dev/uinput ]]; then echo 'Current user can write /dev/uinput.'; fi
    shopt -s nullglob
    for device in /sys/bus/hid/devices/*; do
      driver=$(readlink -f -- "$device/driver" || true)
      if [[ ${driver##*/} == wacom || ${driver##*/} == hid_uclogic ]]; then
        echo "${device##*/}: ${driver##*/}"
      fi
    done
    for file in /etc/udev/rules.d/71-opentabletdriver-rust-ignore-????-????.rules; do
      ls -l -- "$file"
    done
    ;;
  install)
    require_root
    if [[ -L $setup_dir ]]; then echo "Refusing symlink $setup_dir" >&2; exit 1; fi
    install -d -m 700 -- "$setup_dir"
    install_owned "$script_dir/70-opentabletdriver-rust.rules" "$rules"
    install_owned "$script_dir/opentabletdriver-rust.conf" "$modules"
    modprobe uinput
    udevadm control --reload-rules
    udevadm trigger --action=change --subsystem-match=misc --sysname-match=uinput
    udevadm settle
    echo 'Installed permissions and loaded uinput. Replug the tablet; run the driver as your normal user.'
    ;;
  uninstall)
    require_root
    restore_devices
    restore_ignore_rules
    remove_owned "$script_dir/70-opentabletdriver-rust.rules" "$rules"
    remove_owned "$script_dir/opentabletdriver-rust.conf" "$modules"
    rmdir -- "$setup_dir" 2>/dev/null || true
    udevadm control --reload-rules
    echo 'Removed owned setup files. Replug the tablet. uinput remains loaded until reboot.'
    ;;
  ignore-input)
    require_root
    if [[ $# != 3 || ! $2 =~ ^[[:xdigit:]]{4}$ || ! $3 =~ ^[[:xdigit:]]{4}$ ]]; then usage >&2; exit 1; fi
    vendor=${2,,}; product=${3,,}
    if [[ -L $setup_dir ]]; then echo "Refusing symlink $setup_dir" >&2; exit 1; fi
    install -d -m 700 -- "$setup_dir"
    temporary=$(mktemp)
    trap 'rm -f -- "$temporary"' EXIT
    cat > "$temporary" <<RULE
# OpenTabletDriver Rust opt-in physical libinput ignore rule for $vendor:$product.
# Remove with this application's install.sh restore command.
SUBSYSTEM=="input", ATTRS{idVendor}=="$vendor", ATTRS{idProduct}=="$product", ENV{LIBINPUT_IGNORE_DEVICE}="1"
RULE
    install_owned "$temporary" "/etc/udev/rules.d/71-opentabletdriver-rust-ignore-$vendor-$product.rules"
    udevadm control --reload-rules
    echo "Installed physical libinput ignore rule for $vendor:$product. Replug the tablet."
    echo 'Effect depends on compositor rediscovery; applications using raw evdev may still receive physical input.'
    ;;
  detach)
    echo 'Kernel detachment is unavailable: hid-generic refuses devices claimed by loaded specialized drivers and can still emit native input.' >&2
    echo 'For libinput desktops use: sudo install.sh ignore-input VID PID, then replug. No kernel binding was changed.' >&2
    exit 1
    ;;
  restore)
    require_root
    restore_devices
    restore_ignore_rules
    udevadm control --reload-rules
    echo 'Restored owned setup overrides. Replug the tablet for compositor rediscovery.'
    ;;
  *) usage >&2; exit 1 ;;
esac
