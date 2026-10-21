#!/usr/bin/env bash
# Tests install.sh against a release directory.
#
#   scripts/test-install-sh.sh DIR
#
# DIR must hold dotvault-<target>.tar.xz and dotvault-<target>.tar.xz.sha256 for
# the machine running the test (what `dist build` produces, or what the CI job
# assembles). The script is served from localhost, so nothing leaves the machine.

# The checks are eval'd strings that read these variables, which shellcheck cannot see.
# shellcheck disable=SC2034
set -u

script="$(cd "$(dirname "$0")/.." && pwd)/install.sh"
rel=$(cd "${1:?usage: test-install-sh.sh DIR}" && pwd)

archive=$(ls "$rel"/dotvault-*.tar.xz | head -n 1)
asset=$(basename "$archive")
host_target=${asset#dotvault-}
host_target=${host_target%.tar.xz}

tmp=$(mktemp -d)
srv=
cleanup() {
    [ -n "$srv" ] && kill "$srv" 2>/dev/null
    rm -rf "$tmp"
}
trap cleanup EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); printf 'ok    %s\n' "$1"; }
bad() { fail=$((fail + 1)); printf 'FAIL  %s\n' "$1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

serve() { # dir -> sets $url, $srv
    port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
    python3 -m http.server "$port" --bind 127.0.0.1 --directory "$1" >/dev/null 2>&1 &
    srv=$!
    url="http://127.0.0.1:$port"
    for _ in $(seq 1 150); do
        curl -fs -o /dev/null "$url/$asset" 2>/dev/null && return 0
        sleep 0.1
    done
    echo "could not start the test server"
    exit 1
}
serve "$rel"
good_url=$url

run() { # runs install.sh, capturing output in $out and the status in $status
    out=$("$@" 2>&1)
    status=$?
}

# ---- installing ---------------------------------------------------------------
dir="$tmp/a"
export DOTVAULT_DOWNLOAD_URL=$good_url DOTVAULT_TARGET=$host_target
DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "installs and the binary runs" '[ $status -eq 0 ] && "$dir/dotvault" --version | grep -q "^dotvault "'
check "reports the install location" 'echo "$out" | grep -q "installed dotvault .* to $dir/dotvault"'
check "binary is executable" '[ -x "$dir/dotvault" ]'
check "leaves no temp files in the install folder" '[ "$(ls -A "$dir")" = "dotvault" ]'
check "hints at PATH when the folder is not on it" 'echo "$out" | grep -q "is not on your PATH"'

DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "installing again (update) works" '[ $status -eq 0 ]'

dir="$tmp/b"
PATH="$dir:$PATH" DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "no PATH hint when the folder is on PATH" '[ $status -eq 0 ] && ! echo "$out" | grep -q "PATH"'

dir="$tmp/c"
out=$(cat "$script" | DOTVAULT_INSTALL_DIR=$dir sh 2>&1)
status=$?
check "works piped into sh (curl | sh)" '[ $status -eq 0 ] && [ -x "$dir/dotvault" ]'

dir="$tmp/d"
out=$(cat "$script" | sh -s -- --dir "$dir" 2>&1)
status=$?
check "--dir works through sh -s --" '[ $status -eq 0 ] && [ -x "$dir/dotvault" ]'

if command -v dash >/dev/null 2>&1; then
    dir="$tmp/e"
    DOTVAULT_INSTALL_DIR=$dir run dash "$script"
    check "runs under dash (strict POSIX sh)" '[ $status -eq 0 ] && [ -x "$dir/dotvault" ]'
fi

dir="$tmp/nested/deeper/bin"
DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "creates missing folders" '[ $status -eq 0 ] && [ -x "$dir/dotvault" ]'

# ---- download tools -----------------------------------------------------------
only="$tmp/only-wget"
mkdir -p "$only"
for c in sh dash uname tar xz mktemp find cut awk tr head rm mkdir cp chmod mv cat wget sha256sum shasum openssl sed grep ldd sysctl basename dirname; do
    p=$(command -v "$c" 2>/dev/null) && ln -sf "$p" "$only/$c"
done
if command -v wget >/dev/null 2>&1; then
    dir="$tmp/wget"
    PATH="$only" DOTVAULT_INSTALL_DIR=$dir run /bin/sh "$script"
    check "falls back to wget when curl is missing" '[ $status -eq 0 ] && [ -x "$dir/dotvault" ]'
fi
rm -f "$only/wget"
dir="$tmp/nonet"
PATH="$only" DOTVAULT_INSTALL_DIR=$dir run /bin/sh "$script"
check "fails clearly without curl or wget" '[ $status -ne 0 ] && echo "$out" | grep -q "needs curl or wget" && [ ! -e "$dir/dotvault" ]'

# ---- integrity ------------------------------------------------------------------
bad_dir="$tmp/bad-release"
mkdir -p "$bad_dir"
cp "$rel"/dotvault-*.tar.xz "$rel"/dotvault-*.tar.xz.sha256 "$bad_dir/"
python3 - "$bad_dir/$asset" <<'EOF'
import sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
b[len(b) // 2] ^= 0xFF
open(p, "wb").write(bytes(b))
EOF
serve "$bad_dir"
dir="$tmp/tampered"
DOTVAULT_DOWNLOAD_URL=$url DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "rejects a tampered archive and installs nothing" '[ $status -ne 0 ] && echo "$out" | grep -q "checksum mismatch" && [ ! -e "$dir/dotvault" ]'
kill "$srv" 2>/dev/null

wrong_dir="$tmp/wrong-sum"
mkdir -p "$wrong_dir"
cp "$rel/$asset" "$wrong_dir/"
echo "0000000000000000000000000000000000000000000000000000000000000000 *$asset" >"$wrong_dir/$asset.sha256"
serve "$wrong_dir"
dir="$tmp/wrongsum"
DOTVAULT_DOWNLOAD_URL=$url DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "rejects a wrong checksum" '[ $status -ne 0 ] && echo "$out" | grep -q "checksum mismatch" && [ ! -e "$dir/dotvault" ]'
kill "$srv" 2>/dev/null

nosum_dir="$tmp/no-sum"
mkdir -p "$nosum_dir"
cp "$rel/$asset" "$nosum_dir/"
serve "$nosum_dir"
dir="$tmp/nosum"
DOTVAULT_DOWNLOAD_URL=$url DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "refuses to install without a checksum file" '[ $status -ne 0 ] && echo "$out" | grep -q "checksum file" && [ ! -e "$dir/dotvault" ]'
kill "$srv" 2>/dev/null

serve "$rel"
dir="$tmp/missing"
DOTVAULT_TARGET=riscv64gc-unknown-linux-gnu DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "explains a missing release for a platform" '[ $status -ne 0 ] && echo "$out" | grep -q "could not download" && [ ! -e "$dir/dotvault" ]'

# ---- options and exit codes -----------------------------------------------------
run sh "$script" --help
check "--help exits 0 and shows usage" '[ $status -eq 0 ] && echo "$out" | grep -q "Usage: install.sh"'
run sh "$script" --bogus
check "unknown option exits non-zero" '[ $status -ne 0 ] && echo "$out" | grep -q "unknown option"'
run sh "$script" --version
check "--version without a value exits non-zero" '[ $status -ne 0 ] && echo "$out" | grep -q "needs a value"'

# ---- platform detection and URLs (stubbed system tools) -----------------------------
unset DOTVAULT_DOWNLOAD_URL DOTVAULT_TARGET
stubs="$tmp/stubs"
mkdir -p "$stubs"
cat >"$stubs/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo "${STUB_OS:-Linux}" ;; -m) echo "${STUB_ARCH:-x86_64}" ;; *) echo "${STUB_OS:-Linux}" ;; esac
EOF
cat >"$stubs/sysctl" <<'EOF'
#!/bin/sh
echo "${STUB_TRANSLATED:-0}"
EOF
cat >"$stubs/ldd" <<'EOF'
#!/bin/sh
echo "${STUB_LDD:-ldd (GNU libc) 2.39}"
EOF
cat >"$stubs/curl" <<EOF
#!/bin/sh
echo "\$*" >>"$tmp/curl.log"
exit 22
EOF
chmod +x "$stubs"/*

# detect OS ARCH [TRANSLATED] [LDD_OUTPUT]
# Runs the installer with stubbed system tools. Sets $out and $status, and $got to the
# URL the installer asked curl for (empty if it never got that far). It must not run
# in a subshell, or $out and $status would be lost and the checks would read stale values.
detect() {
    : >"$tmp/curl.log"
    out=$(STUB_OS=$1 STUB_ARCH=$2 STUB_TRANSLATED=${3:-0} STUB_LDD=${4:-"ldd (GNU libc) 2.39"} \
        PATH="$stubs:$PATH" DOTVAULT_INSTALL_DIR="$tmp/detect" sh "$script" 2>&1)
    status=$?
    got=$(head -n 1 "$tmp/curl.log" | tr ' ' '\n' | grep '^https://' | head -n 1)
}

base=https://github.com/YatharthKaushal/dotvault/releases/latest/download
for case in \
    "Linux x86_64|x86_64-unknown-linux-gnu" \
    "Linux amd64|x86_64-unknown-linux-gnu" \
    "Linux aarch64|aarch64-unknown-linux-gnu" \
    "Linux arm64|aarch64-unknown-linux-gnu" \
    "Darwin arm64|aarch64-apple-darwin" \
    "Darwin x86_64|x86_64-apple-darwin"; do
    want=${case##*|}
    spec=${case%%|*}
    set -- $spec
    detect "$1" "$2"
    check "detects $1 $2 as $want" '[ "$got" = "$base/dotvault-$want.tar.xz" ]'
done

detect Darwin x86_64 1
check "a shell under Rosetta gets the Apple Silicon build" '[ "$got" = "$base/dotvault-aarch64-apple-darwin.tar.xz" ]'

for case in "Linux armv7l" "Linux riscv64" "FreeBSD amd64" "MINGW64_NT-10.0 x86_64" "Darwin ppc"; do
    set -- $case
    detect "$1" "$2"
    check "refuses unsupported $1 $2 without downloading" '[ $status -ne 0 ] && [ -z "$got" ] && [ ! -s "$tmp/curl.log" ] && echo "$out" | grep -q "^error:"'
done

detect MINGW64_NT-10.0 x86_64
check "points Windows users at install.ps1" '[ $status -ne 0 ] && echo "$out" | grep -q "install.ps1"'

detect Linux x86_64 0 "musl libc (x86_64) Version 1.2.4"
check "refuses musl Linux with a clear message" '[ $status -ne 0 ] && [ -z "$got" ] && [ ! -s "$tmp/curl.log" ] && echo "$out" | grep -q "glibc"'

for case in \
    "ldd (Debian GLIBC 2.31-13+deb11u11) 2.31|2.31" \
    "ldd (Ubuntu GLIBC 2.31-0ubuntu9.16) 2.31|2.31" \
    "ldd (GNU libc) 2.28|2.28" \
    "ldd (GNU libc) 2.33|2.33"; do
    lddout=${case%%|*}
    want=${case##*|}
    detect Linux x86_64 0 "$lddout"
    check "refuses glibc $want as too old, before downloading" '[ $status -ne 0 ] && [ -z "$got" ] && [ ! -s "$tmp/curl.log" ] && echo "$out" | grep -q "glibc 2.34 or newer" && echo "$out" | grep -q "has $want"'
done

for lddout in "ldd (GNU libc) 2.34" "ldd (Ubuntu GLIBC 2.35-0ubuntu3.15) 2.35" "ldd (Debian GLIBC 2.36-9+deb12u7) 2.36" "ldd (GNU libc) 2.39" "ldd (GNU libc) 3.0"; do
    detect Linux x86_64 0 "$lddout"
    check "accepts $lddout" '[ -n "$got" ]'
done

# never block a system whose glibc version cannot be read
for lddout in "ldd: something unexpected" ""; do
    detect Linux x86_64 0 "$lddout"
    check "does not block when the glibc version is unreadable (${lddout:-empty output})" '[ -n "$got" ]'
done

# The checks above must be able to fail: a supported platform really reaches curl.
detect Linux x86_64
check "a supported platform does reach curl" '[ -n "$got" ] && [ -s "$tmp/curl.log" ]'

: >"$tmp/curl.log"
STUB_OS=Linux STUB_ARCH=x86_64 DOTVAULT_VERSION=v9.9.9 PATH="$stubs:$PATH" DOTVAULT_INSTALL_DIR="$tmp/detect" run sh "$script"
check "DOTVAULT_VERSION selects that release" 'grep -q "https://github.com/YatharthKaushal/dotvault/releases/download/v9.9.9/dotvault-x86_64-unknown-linux-gnu.tar.xz" "$tmp/curl.log"'
: >"$tmp/curl.log"
STUB_OS=Linux STUB_ARCH=x86_64 PATH="$stubs:$PATH" DOTVAULT_INSTALL_DIR="$tmp/detect" run sh "$script" --version v8.8.8
check "--version selects that release" 'grep -q "releases/download/v8.8.8/" "$tmp/curl.log"'
check "downloads from GitHub only over https with TLS 1.2+" 'grep -q -- "--proto =https --tlsv1.2" "$tmp/curl.log"'

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
