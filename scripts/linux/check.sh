#!/usr/bin/env bash
# scripts/linux/check.sh <x86_64|aarch64|both|mac> [cargo check arguments...]: type-checks this tree for Linux from Windows
# (Git Bash), without a Linux toolchain: nothing is linked or run. Both triples, because `c_char` is signed on one and
# unsigned on the other. Without arguments: every crate that Linux builds, tests included. (`mac`: the same for Apple silicon, for
# what a change to shared code does there.)
#
#   scripts/linux/check.sh both
#   scripts/linux/check.sh aarch64 -p orbiters-core --tests
#
# What it needs lives in one folder, $N4N0_LINUX_T0 (default H:/n4n0-linux-t0; `setup` fills it, 0.5 GB and a target of 3-4 GB;
# delete the folder to undo): Rust's standard library for the three triples, beside an untouched toolchain, and a stand-in for
# the C compiler and archiver, whose "objects" are empty files.
set -u
# (All of it read before it runs: an edit meanwhile does not reach a run.)
main() {
T0=${N4N0_LINUX_T0:-H:/n4n0-linux-t0}
here=$(cd "$(dirname "$0")/../.." && pwd -W)
# The Linux crates: all of them but those that are Windows' alone (their build scripts or whole content).
NOT_LINUX="--exclude orbiters-web --exclude orbiters-setup --exclude orbiters-spout --exclude orbiters-n4n0-mobile"

if [[ ${1:-} == setup ]]; then
  version=$(rustc --version | cut -d' ' -f2)
  mkdir -p "$T0/sysroot/lib/rustlib" "$T0/tools" "$T0/x" || exit 1
  for triple in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu aarch64-apple-darwin; do
    [[ -d $T0/sysroot/lib/rustlib/$triple ]] && continue
    name=rust-std-$version-$triple file=$T0/x/rust-std-$version-$triple.tar.xz
    curl -fsSL "https://static.rust-lang.org/dist/$name.tar.xz" -o "$file" || exit 1
    sum=$(curl -fsSL "https://static.rust-lang.org/dist/$name.tar.xz.sha256" | cut -d' ' -f1)
    [[ -n $sum && $(sha256sum < "$file" | cut -d' ' -f1) == "$sum" ]] || { echo "$name: not what rust-lang.org names" >&2; exit 1; }
    (cd "$T0/x" && tar xJf "$name.tar.xz" && cp -r "$name/rust-std-$triple/lib/rustlib/$triple" "$T0/sysroot/lib/rustlib/") || exit 1
  done
  rm -rf "$T0/x"
  rustc -O -C strip=symbols -o "$T0/tools/standin.exe" "$here/scripts/linux/standin.rs" || exit 1
  echo "ready in $T0"
  exit 0
fi

case ${1:-} in x86_64|aarch64) triples=$1-unknown-linux-gnu ;; both) triples="x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu" ;; mac) triples=aarch64-apple-darwin ;; *) sed -n '2,12p' "$0" >&2; exit 2 ;; esac
shift
[[ $# -gt 0 ]] || set -- --workspace $NOT_LINUX --tests
[[ -x $T0/tools/standin.exe && -d $T0/sysroot ]] || { echo "run scripts/linux/check.sh setup first" >&2; exit 2; }
[[ -e ${DESK_STOP:-C:/Users/Enzo/.orbiters-build/STOP} ]] && exit 75
free=$(df -BG --output=avail "$T0" | tail -1 | tr -dc 0-9)
(( free >= 8 )) || { echo "only $free GB free for $T0" >&2; exit 3; }

# One at a time: several worktrees share the target.
until mkdir "$T0/lock" 2>/dev/null; do
  # A holder touches it every 30 s; two minutes without means it died.
  at=$(stat -c %Y "$T0/lock" 2>/dev/null) && (( $(date +%s) - at > 120 )) && rmdir "$T0/lock" 2>/dev/null
  sleep 2
done
( while [[ -d $T0/lock ]]; do touch "$T0/lock"; sleep 30; done ) & beat=$!
trap 'kill $beat 2>/dev/null; rmdir "$T0/lock" 2>/dev/null' EXIT
trap 'exit 130' INT TERM

# cargo names its units by the crate's path inside the workspace: another worktree's crates are not this one's.
[[ $(cat "$T0/tree" 2>/dev/null) == "$here" ]] || rm -rf "$T0"/target/*-*-*/debug/.fingerprint/{orbiters-,xtask-}*
echo "$here" > "$T0/tree"

export CARGO_TARGET_DIR=$T0/target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
# Opus (libopus, linked through pkg-config or built by CMake) and sherpa-onnx (downloaded) are told where a library
# would be; nothing is linked, so nothing is there.
mkdir -p "$T0/libs" && touch "$T0/libs/libsherpa-onnx-c-api."{so,dylib} "$T0/libs/libonnxruntime."{so,dylib}
export OPUS_LIB_DIR=$T0/libs LIBOPUS_NO_PKG=1 SHERPA_ONNX_LIB_DIR=$T0/libs PKG_CONFIG_ALLOW_CROSS=1
code=0
for triple in $triples; do
  under=${triple//-/_}
  upper=$(echo "$under" | tr a-z A-Z)
  export CC_$under="$T0/tools/standin.exe" CXX_$under="$T0/tools/standin.exe" AR_$under="$T0/tools/standin.exe"
  export CARGO_TARGET_${upper}_RUSTFLAGS="--sysroot $T0/sysroot" CARGO_TARGET_${upper}_LINKER="$T0/tools/standin.exe"
  echo "== $triple: cargo check $*"
  (cd "$here" && cargo check --target "$triple" "$@") || code=$?
done
exit $code
}
main "$@"; exit
