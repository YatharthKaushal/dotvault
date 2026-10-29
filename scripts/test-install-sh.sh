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
servers=
# The installer edits shell startup files, so no test may ever see a real home.
export HOME="$tmp/home"
mkdir -p "$HOME"
unset ZDOTDIR XDG_DATA_HOME XDG_CONFIG_HOME DOTVAULT_NO_MODIFY_PATH
srv=
cleanup() {
    # shellcheck disable=SC2086
    [ -n "$servers" ] && kill $servers 2>/dev/null
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
    servers="$servers $srv"
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
check "sets up PATH when the folder is not on it" 'echo "$out" | grep -q "to your PATH in"'

dir="$tmp/a-hint"
DOTVAULT_NO_MODIFY_PATH=1 DOTVAULT_INSTALL_DIR=$dir run sh "$script"
check "hints at PATH instead when told to leave it alone" '[ $status -eq 0 ] && echo "$out" | grep -q "is not on your PATH" && ! echo "$out" | grep -q "to your PATH in"'

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

# ---- putting the install folder on PATH ------------------------------------------------
# These tests install dozens of times and only care about the PATH setup, so they use a
# tiny stand-in binary (the checks above already installed the real one).
tiny="$tmp/tiny-release"
mkdir -p "$tiny/pkg/dotvault-$host_target"
printf '#!/bin/sh\necho "dotvault 0.0.0-test"\n' >"$tiny/pkg/dotvault-$host_target/dotvault"
chmod +x "$tiny/pkg/dotvault-$host_target/dotvault"
tar -C "$tiny/pkg" -cJf "$tiny/$asset" "dotvault-$host_target"
python3 - "$tiny/$asset" "$asset" >"$tiny/$asset.sha256" <<'EOF'
import hashlib, sys
print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest(), "*" + sys.argv[2])
EOF
serve "$tiny"
export DOTVAULT_DOWNLOAD_URL=$url

# Each case gets a new empty home and a chosen login shell. A brand new shell is then
# started with a bare PATH to prove that a new terminal finds dotvault.
base_path=/usr/bin:/bin
bash_bin=$(command -v bash)

newhome() { H="$tmp/home-$1"; rm -rf "$H"; mkdir -p "$H"; }

# inst SHELL [installer args]: installs into the default folder ($H/.local/bin)
inst() {
    sh_path_arg=$1
    shift
    out=$(HOME=$H SHELL=$sh_path_arg sh "$script" "$@" 2>&1)
    status=$?
}

# fresh SHELL_BIN FLAGS: what a brand new shell with a bare PATH resolves "dotvault" to
fresh() { env -i HOME="${FH:-$H}" PATH="$base_path" SHELL="$1" "$1" $2 'command -v dotvault' 2>/dev/null | tail -n 1; }

count() { grep -c "$1" "$2" 2>/dev/null || true; }

newhome bash
inst /bin/bash
check "bash: reports what it changed" '[ $status -eq 0 ] && echo "$out" | grep -q "to your PATH in ~/.bashrc, ~/.profile"'
check "bash: tells how to use it in this terminal" 'echo "$out" | grep -q "open a new terminal, or run this"'
check "bash: a new login shell finds dotvault" '[ "$(fresh "$bash_bin" -lc)" = "$H/.local/bin/dotvault" ]'
check "bash: a new interactive shell finds dotvault" '[ "$(fresh "$bash_bin" -ic)" = "$H/.local/bin/dotvault" ]'
check "bash: one marker line in each file" '[ "$(count "added by the dotvault installer" "$H/.bashrc")" = 1 ] && [ "$(count "added by the dotvault installer" "$H/.profile")" = 1 ]'
check "bash: the env file keeps the folder relative to HOME" 'grep -q "\"\$HOME/.local/bin\"" "$H/.local/share/dotvault/env"'
before_rc=$(cat "$H/.bashrc"; cat "$H/.profile")
inst /bin/bash
check "bash: installing again changes nothing and says it is already set up" '[ $status -eq 0 ] && [ "$(cat "$H/.bashrc"; cat "$H/.profile")" = "$before_rc" ] && echo "$out" | grep -q "already set up"'

# which login file bash reads
newhome bashlogin1
printf '# mine\n' >"$H/.profile"
inst /bin/bash
check "bash: with only .profile, edits .profile and never creates .bash_profile" '[ "$(count "dotvault/env" "$H/.profile")" = 1 ] && [ ! -e "$H/.bash_profile" ] && [ "$(fresh "$bash_bin" -lc)" = "$H/.local/bin/dotvault" ] && head -n 1 "$H/.profile" | grep -q "^# mine$"'
newhome bashlogin2
printf '# bp\n' >"$H/.bash_profile"
printf '# p\n' >"$H/.profile"
inst /bin/bash
check "bash: with .bash_profile and .profile, edits only .bash_profile" '[ "$(count "dotvault/env" "$H/.bash_profile")" = 1 ] && [ "$(cat "$H/.profile")" = "# p" ]'
newhome bashlogin3
printf '# bl\n' >"$H/.bash_login"
inst /bin/bash
check "bash: with only .bash_login, edits it and creates no .profile" '[ "$(count "dotvault/env" "$H/.bash_login")" = 1 ] && [ ! -e "$H/.profile" ]'

# existing content is kept exactly
newhome keep
printf 'alias a=b' >"$H/.bashrc"
inst /bin/bash
check "a startup file with no trailing newline keeps its last line intact" '[ "$(head -n 1 "$H/.bashrc")" = "alias a=b" ] && [ "$(count "dotvault/env" "$H/.bashrc")" = 1 ] && [ "$(fresh "$bash_bin" -ic)" = "$H/.local/bin/dotvault" ]'
newhome link
mkdir -p "$H/dots"
printf '# from my dotfiles repo\n' >"$H/dots/bashrc"
ln -s "$H/dots/bashrc" "$H/.bashrc"
inst /bin/bash
check "a symlinked startup file stays a link and its target is edited" '[ -L "$H/.bashrc" ] && [ "$(count "dotvault/env" "$H/dots/bashrc")" = 1 ] && head -n 1 "$H/dots/bashrc" | grep -q "^# from my dotfiles repo$"'

# zsh
newhome zsh
inst /usr/bin/zsh
check "zsh: edits only .zshrc" '[ "$(count "dotvault/env" "$H/.zshrc")" = 1 ] && [ ! -e "$H/.bashrc" ] && [ ! -e "$H/.profile" ] && echo "$out" | grep -q "to your PATH in ~/.zshrc"'
if command -v zsh >/dev/null 2>&1; then
    check "zsh: a new interactive zsh finds dotvault" '[ "$(fresh "$(command -v zsh)" -ic)" = "$H/.local/bin/dotvault" ]'
fi
newhome zdot
mkdir -p "$H/zdot"
out=$(HOME=$H SHELL=/usr/bin/zsh ZDOTDIR=$H/zdot sh "$script" 2>&1)
check "zsh: honours ZDOTDIR" '[ "$(count "dotvault/env" "$H/zdot/.zshrc")" = 1 ] && [ ! -e "$H/.zshrc" ]'

# fish gets a file of its own and no user file is edited
newhome fish
printf '# fish config\n' >"$H/.profile"
inst /usr/bin/fish
check "fish: writes only conf.d/dotvault.fish" '[ -f "$H/.config/fish/conf.d/dotvault.fish" ] && [ "$(cat "$H/.profile")" = "# fish config" ] && [ ! -e "$H/.bashrc" ] && [ ! -e "$H/.zshrc" ]'
check "fish: the file uses fish syntax" 'grep -q "^if not contains -- " "$H/.config/fish/conf.d/dotvault.fish" && grep -q "set -gx PATH " "$H/.config/fish/conf.d/dotvault.fish"'
check "fish: installing again says it is already set up" 'inst /usr/bin/fish; echo "$out" | grep -q "already set up"'
if command -v fish >/dev/null 2>&1; then
    check "fish: a new fish finds dotvault" '[ "$(fresh "$(command -v fish)" -c)" = "$H/.local/bin/dotvault" ]'
fi
newhome fishxdg
out=$(HOME=$H SHELL=/usr/bin/fish XDG_CONFIG_HOME=$H/cfg sh "$script" 2>&1)
check "fish: honours XDG_CONFIG_HOME" '[ -f "$H/cfg/fish/conf.d/dotvault.fish" ] && [ ! -e "$H/.config" ]'

# plain POSIX shells, an unknown login shell, and shells the installer cannot set up
newhome dash
inst /bin/sh
check "sh: edits .profile only" '[ "$(count "dotvault/env" "$H/.profile")" = 1 ] && [ ! -e "$H/.bashrc" ]'
if command -v dash >/dev/null 2>&1; then
    check "sh: a new login dash finds dotvault" '[ "$(fresh "$(command -v dash)" -lc)" = "$H/.local/bin/dotvault" ]'
fi
newhome noshell
out=$(env -u SHELL HOME=$H sh "$script" 2>&1)
check "no login shell recorded: falls back to .profile" '[ "$(count "dotvault/env" "$H/.profile")" = 1 ]'
for odd in /bin/tcsh /usr/bin/nu /bin/csh; do
    newhome odd
    inst "$odd"
    check "${odd##*/}: writes nothing and says what to do by hand" '[ $status -eq 0 ] && [ -z "$(find "$H" -maxdepth 1 -name ".*" ! -name .local ! -name . | head -n 1)" ] && [ ! -e "$H/.local/share/dotvault" ] && echo "$out" | grep -q "cannot set up for you" && echo "$out" | grep -q "is not on your PATH"'
done

# opting out
newhome optout
inst /bin/bash --no-modify-path
check "--no-modify-path writes nothing and hints instead" '[ $status -eq 0 ] && [ ! -e "$H/.bashrc" ] && [ ! -e "$H/.profile" ] && [ ! -e "$H/.local/share/dotvault" ] && echo "$out" | grep -q "is not on your PATH"'
out=$(cat "$script" | HOME=$H SHELL=/bin/bash sh -s -- --no-modify-path 2>&1)
check "--no-modify-path works through sh -s --" '[ ! -e "$H/.bashrc" ]'
out=$(HOME=$H SHELL=/bin/bash DOTVAULT_NO_MODIFY_PATH=1 sh "$script" 2>&1)
check "DOTVAULT_NO_MODIFY_PATH=1 writes nothing" '[ ! -e "$H/.bashrc" ] && [ ! -e "$H/.profile" ]'
check "the help mentions both ways to opt out" 'sh "$script" --help 2>&1 | grep -q -- "--no-modify-path" && sh "$script" --help 2>&1 | grep -q DOTVAULT_NO_MODIFY_PATH'

# nothing to do when the folder is already on PATH
newhome onpath
out=$(HOME=$H SHELL=/bin/bash PATH="$H/.local/bin:$PATH" sh "$script" 2>&1)
check "a folder already on PATH changes nothing and says nothing about PATH" '[ ! -e "$H/.bashrc" ] && [ ! -e "$H/.profile" ] && [ ! -e "$H/.local/share/dotvault" ] && ! echo "$out" | grep -q "PATH"'

# trouble with the startup file is a warning, never a failed install
newhome readonly
printf '# locked\n' >"$H/.bashrc"
chmod 444 "$H/.bashrc"
inst /bin/bash
if [ "$(id -u)" != 0 ] && ! (: >>"$H/.bashrc") 2>/dev/null; then
    check "an unwritable startup file: install succeeds, warns, and leaves it alone" '[ $status -eq 0 ] && [ -x "$H/.local/bin/dotvault" ] && echo "$out" | grep -q "could not write to ~/.bashrc" && echo "$out" | grep -q "is not on your PATH" && [ "$(cat "$H/.bashrc")" = "# locked" ]'
fi
chmod 644 "$H/.bashrc"
newhome dirasfile
mkdir -p "$H/.bashrc"
inst /bin/bash
check "a startup file that is a folder: install succeeds and warns" '[ $status -eq 0 ] && [ -x "$H/.local/bin/dotvault" ] && echo "$out" | grep -q "could not write"'

# an install folder with spaces and an apostrophe
newhome odddir
odd="$H/my dir/it's here/bin"
out=$(HOME=$H SHELL=/bin/bash DOTVAULT_INSTALL_DIR=$odd sh "$script" 2>&1)
check "a folder with spaces and an apostrophe works through the env file" '[ "$(fresh "$bash_bin" -lc)" = "$odd/dotvault" ] && [ "$(fresh "$bash_bin" -ic)" = "$odd/dotvault" ]'

# a relative --dir becomes an absolute PATH entry
newhome rel
out=$(cd "$H" && HOME=$H SHELL=/bin/bash sh "$script" --dir ./tools/bin 2>&1)
check "a relative --dir is put on PATH as an absolute path" '[ "$(fresh "$bash_bin" -lc)" = "$(cd "$H/tools/bin" && pwd)/dotvault" ]'

# another data folder
newhome xdgdata
out=$(HOME=$H SHELL=/bin/bash XDG_DATA_HOME=$H/data sh "$script" 2>&1)
check "XDG_DATA_HOME decides where the env file lives, and new shells still work" '[ -f "$H/data/dotvault/env" ] && [ ! -e "$H/.local/share" ] && [ "$(fresh "$bash_bin" -lc)" = "$H/.local/bin/dotvault" ]'

# deleting the env file must never break a login
newhome undo
inst /bin/bash
rm -rf "$H/.local/share/dotvault"
check "removing the env file leaves shells starting cleanly" '[ -z "$(env -i HOME="$H" PATH="$base_path" "$bash_bin" -lc "echo started" 2>&1 | grep -v "^started$")" ]'

# the way users run it
newhome piped
out=$(cat "$script" | HOME=$H SHELL=/bin/bash sh 2>&1)
check "piped into sh (curl | sh) sets up PATH too" '[ "$(fresh "$bash_bin" -lc)" = "$H/.local/bin/dotvault" ]'
if command -v dash >/dev/null 2>&1; then
    newhome dashrun
    out=$(HOME=$H SHELL=/bin/bash dash "$script" 2>&1)
    check "the PATH setup runs under dash (strict POSIX sh)" '[ "$(fresh "$bash_bin" -lc)" = "$H/.local/bin/dotvault" ]'
fi

export DOTVAULT_DOWNLOAD_URL=$good_url

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
