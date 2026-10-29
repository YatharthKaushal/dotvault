#!/bin/sh
# dotvault installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh | sh
#
# It downloads the archive for this platform from GitHub Releases, checks its
# SHA-256 checksum, and copies the binary into place. It never uses sudo.
#
# If the install folder is not on your PATH it adds it for new terminals: one
# line appended to your login shell's startup file (~/.zshrc, or ~/.bashrc and
# the login file bash reads, or ~/.profile; fish gets its own conf.d file). The
# line only sources ~/.local/share/dotvault/env, so deleting that file undoes it.
# Nothing is changed when the folder is already on PATH, and --no-modify-path
# leaves your shell alone.
#
# Environment variables:
#   DOTVAULT_VERSION         release tag to install, for example v0.1.0 (default: latest)
#   DOTVAULT_INSTALL_DIR     where to put the binary (default: $HOME/.local/bin)
#   DOTVAULT_NO_MODIFY_PATH  set to 1 to leave your PATH and shell startup files alone
#   DOTVAULT_DOWNLOAD_URL    base URL that holds the release files (default: GitHub Releases)
#   DOTVAULT_TARGET          use this target triple instead of detecting one
#
# Options (after `sh -s --` when piping): -h, --help, --version TAG, --dir DIR,
# --no-modify-path

set -eu

REPO="YatharthKaushal/dotvault"
APP="dotvault"

say() {
    printf '%s\n' "$*" >&2
}

err() {
    say "error: $*"
    exit 1
}

usage() {
    cat >&2 <<EOF
Install $APP.

Usage: install.sh [--version TAG] [--dir DIR] [--no-modify-path]

  --version TAG     release to install, for example v0.1.0 (default: latest)
  --dir DIR         install into DIR (default: \$HOME/.local/bin)
  --no-modify-path  do not put DIR on your PATH or touch your shell startup files
  -h, --help        show this help

If DIR is not on your PATH, the installer adds it for new terminals by appending
one line to your shell's startup file, and says which file.

Environment: DOTVAULT_VERSION, DOTVAULT_INSTALL_DIR, DOTVAULT_NO_MODIFY_PATH,
DOTVAULT_DOWNLOAD_URL, DOTVAULT_TARGET
EOF
}

need() {
    command -v "$1" >/dev/null 2>&1 || err "this installer needs '$1', which was not found"
}

is_musl() {
    if command -v ldd >/dev/null 2>&1; then
        case "$(ldd --version 2>&1 || true)" in
            *musl*) return 0 ;;
        esac
    fi
    return 1
}

# The Linux binaries are built on Ubuntu 22.04 and need glibc 2.34 or newer.
MIN_GLIBC_MAJOR=2
MIN_GLIBC_MINOR=34

# Prints the glibc version ("2.35"), or nothing if it cannot be read.
glibc_version() {
    command -v ldd >/dev/null 2>&1 || return 0
    ldd --version 2>&1 | head -n 1 | awk '{print $NF}' | grep -E '^[0-9]+\.[0-9]+' | cut -d . -f 1,2
}

# True when the version is new enough, or when it cannot be determined (the
# check must never block a working system).
glibc_new_enough() {
    ver=$(glibc_version)
    [ -n "$ver" ] || return 0
    major=${ver%%.*}
    minor=${ver#*.}
    [ "$major" -gt "$MIN_GLIBC_MAJOR" ] && return 0
    [ "$major" -eq "$MIN_GLIBC_MAJOR" ] && [ "$minor" -ge "$MIN_GLIBC_MINOR" ]
}

detect_target() {
    if [ -n "${DOTVAULT_TARGET:-}" ]; then
        printf '%s' "$DOTVAULT_TARGET"
        return 0
    fi

    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux)
            case "$arch" in
                x86_64 | amd64) cpu=x86_64 ;;
                aarch64 | arm64) cpu=aarch64 ;;
                *) err "unsupported CPU '$arch' (prebuilt binaries cover x86_64 and aarch64); build from source instead" ;;
            esac
            if is_musl; then
                err "the prebuilt Linux binaries need glibc and this system uses musl; build from source instead: cargo install --path crates/dotvault-cli"
            fi
            if ! glibc_new_enough; then
                err "the prebuilt Linux binaries need glibc $MIN_GLIBC_MAJOR.$MIN_GLIBC_MINOR or newer (Ubuntu 22.04, Debian 12, RHEL 9 or later) and this system has $(glibc_version); build from source instead: cargo install --path crates/dotvault-cli"
            fi
            printf '%s-unknown-linux-gnu' "$cpu"
            ;;
        Darwin)
            case "$arch" in
                x86_64) cpu=x86_64 ;;
                arm64 | aarch64) cpu=aarch64 ;;
                *) err "unsupported CPU '$arch'" ;;
            esac
            # A shell running under Rosetta reports x86_64 on an Apple Silicon Mac.
            if [ "$cpu" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
                cpu=aarch64
            fi
            printf '%s-apple-darwin' "$cpu"
            ;;
        *)
            err "unsupported system '$os'. On Windows use install.ps1"
            ;;
    esac
}

download() {
    url=$1
    dest=$2
    case "$url" in
        https://*) proto='=https' ;;
        *) proto='=http,https' ;;
    esac
    if command -v curl >/dev/null 2>&1; then
        curl --proto "$proto" --tlsv1.2 --retry 3 -fsSL "$url" -o "$dest"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$dest" "$url"
    else
        err "this installer needs curl or wget"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$1" | awk '{print $NF}'
    else
        err "need sha256sum, shasum or openssl to verify the download"
    fi
}

# ---- putting the install folder on PATH --------------------------------------------

MARK="# added by the dotvault installer"

# Single-quotes $1 for a POSIX shell, so it is safe to paste into a script:
# it's -> 'it'\''s'
sh_quote() {
    q="'"
    rest=$1
    quoted=
    while :; do
        case $rest in
            *"$q"*)
                head=${rest%%"$q"*}
                quoted="$quoted$head'\\''"
                rest=${rest#*"$q"}
                ;;
            *)
                quoted="$quoted$rest"
                break
                ;;
        esac
    done
    printf "'%s'" "$quoted"
}

# Prints $1 as it should appear in a profile script: "$HOME/..." when it lives
# under $HOME (so a dotfile synced to another machine still works), else quoted.
sh_path() {
    case $1 in
        "$HOME") printf '"$HOME"' ;;
        "$HOME"/*)
            tail_part=${1#"$HOME"}
            case $tail_part in
                *[!A-Za-z0-9._/+-]*) printf '"$HOME"%s' "$(sh_quote "$tail_part")" ;;
                *) printf '"$HOME%s"' "$tail_part" ;;
            esac
            ;;
        *) sh_quote "$1" ;;
    esac
}

# $1 with a leading $HOME shown as ~
show_path() {
    case $1 in
        "$HOME"/*) printf '~%s' "${1#"$HOME"}" ;;
        *) printf '%s' "$1" ;;
    esac
}

# What to do by hand, for when the installer cannot or was told not to.
manual_path_hint() {
    say ""
    say "$1 is not on your PATH. Add it with:"
    say "  export PATH=\"$1:\$PATH\""
    say "and put that line in your shell profile (~/.bashrc, ~/.zshrc, ...)."
}

# Writes the env file that puts $2 on PATH, once, whatever shell sources it.
write_env_file() {
    envp=$(sh_path "$2")
    mkdir -p "${1%/*}" 2>/dev/null || return 1
    {
        printf '%s\n' "# Added by the dotvault installer: puts the install folder on PATH."
        printf '%s\n' "# Delete this file, and the line that sources it, to undo."
        printf 'case ":${PATH}:" in\n'
        printf '    *":"%s":"*) ;;\n' "$envp"
        printf '    *) export PATH=%s"${PATH:+:${PATH}}" ;;\n' "$envp"
        printf 'esac\n'
    } >"$1" 2>/dev/null
}

# Appends $2 to the startup file $1 unless dotvault is already sourced there.
# Prints added, present or failed. Keeps the file's mode, owner and symlink.
add_source_line() {
    file=$1
    if [ -f "$file" ] && grep -F -q 'dotvault/env' "$file" 2>/dev/null; then
        printf present
        return 0
    fi
    mkdir -p "${file%/*}" 2>/dev/null || {
        printf failed
        return 0
    }
    # The leading newline also ends a last line that has none, so our comment
    # never lands on the end of the user's last line.
    if printf '\n%s\n%s\n' "$MARK" "$2" >>"$file" 2>/dev/null; then
        printf added
    else
        printf failed
    fi
}

# The startup files for the login shell, one per line. Fails for shells whose
# syntax is not POSIX sh.
profile_files() {
    case $1 in
        zsh) printf '%s\n' "${ZDOTDIR:-$HOME}/.zshrc" ;;
        bash)
            printf '%s\n' "$HOME/.bashrc"
            # bash reads only the first of these, and creating .bash_profile
            # when .profile exists would hide .profile
            for f in .bash_profile .bash_login .profile; do
                if [ -e "$HOME/$f" ]; then
                    printf '%s\n' "$HOME/$f"
                    return 0
                fi
            done
            printf '%s\n' "$HOME/.profile"
            ;;
        sh | dash | ksh | ash | mksh | "") printf '%s\n' "$HOME/.profile" ;;
        *) return 1 ;;
    esac
}

# $1 is the install folder, which is not on PATH.
path_setup() {
    dir=$1
    if [ "${DOTVAULT_NO_MODIFY_PATH:-}" = 1 ]; then
        manual_path_hint "$dir"
        return 0
    fi
    if [ -z "${HOME:-}" ] || ! command -v grep >/dev/null 2>&1; then
        manual_path_hint "$dir"
        return 0
    fi
    shell=${SHELL:-}
    shell=${shell##*/}

    if [ "$shell" = fish ]; then
        conf="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/dotvault.fish"
        case $dir in
            *"'"* | *'\'*)
                manual_path_hint "$dir"
                return 0
                ;;
        esac
        wanted="# Added by the dotvault installer. Delete this file to undo.
if not contains -- '$dir' \$PATH
    set -gx PATH '$dir' \$PATH
end"
        if [ -f "$conf" ] && [ "$(cat "$conf" 2>/dev/null)" = "$wanted" ]; then
            say ""
            say "$(show_path "$dir") is already set up for new terminals in $(show_path "$conf")"
        elif mkdir -p "${conf%/*}" 2>/dev/null && printf '%s\n' "$wanted" >"$conf" 2>/dev/null; then
            say ""
            say "added $(show_path "$dir") to your PATH in $(show_path "$conf")"
        else
            say ""
            say "warning: could not write $(show_path "$conf")"
            manual_path_hint "$dir"
            return 0
        fi
        say "open a new terminal, or run this to use it now:"
        say "  source $(show_path "$conf")"
        return 0
    fi

    files=$(profile_files "$shell") || {
        say ""
        say "your shell ($shell) is one the installer cannot set up for you."
        manual_path_hint "$dir"
        return 0
    }
    envfile="${XDG_DATA_HOME:-$HOME/.local/share}/dotvault/env"
    if ! write_env_file "$envfile" "$dir"; then
        say ""
        say "warning: could not write $(show_path "$envfile")"
        manual_path_hint "$dir"
        return 0
    fi
    envq=$(sh_path "$envfile")
    line="[ -f $envq ] && . $envq"

    added=
    present=
    failed=
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        case $(add_source_line "$f" "$line") in
            added) added="${added:+$added, }$(show_path "$f")" ;;
            present) present="${present:+$present, }$(show_path "$f")" ;;
            *) failed="${failed:+$failed, }$(show_path "$f")" ;;
        esac
    done <<EOF
$files
EOF

    if [ -n "$failed" ]; then
        say ""
        say "warning: could not write to $failed"
        manual_path_hint "$dir"
    fi
    if [ -n "$added" ]; then
        say ""
        say "added $(show_path "$dir") to your PATH in $added"
    elif [ -n "$present" ]; then
        say ""
        say "$(show_path "$dir") is already set up for new terminals in $present"
    fi
    if [ -n "$added$present" ]; then
        say "open a new terminal, or run this to use it now:"
        say "  . $envq"
    fi
}

main() {
    while [ $# -gt 0 ]; do
        case "$1" in
            -h | --help)
                usage
                exit 0
                ;;
            --version)
                [ $# -ge 2 ] || err "--version needs a value, for example --version v0.1.0"
                DOTVAULT_VERSION=$2
                shift 2
                ;;
            --dir)
                [ $# -ge 2 ] || err "--dir needs a folder"
                DOTVAULT_INSTALL_DIR=$2
                shift 2
                ;;
            --no-modify-path)
                DOTVAULT_NO_MODIFY_PATH=1
                shift
                ;;
            *)
                usage
                err "unknown option '$1'"
                ;;
        esac
    done

    need uname
    need tar
    need mktemp
    need find

    target=$(detect_target) || exit 1
    asset="$APP-$target.tar.xz"

    if [ -n "${DOTVAULT_DOWNLOAD_URL:-}" ]; then
        base=${DOTVAULT_DOWNLOAD_URL%/}
        say "using download URL $base"
    elif [ -n "${DOTVAULT_VERSION:-}" ]; then
        base="https://github.com/$REPO/releases/download/$DOTVAULT_VERSION"
    else
        base="https://github.com/$REPO/releases/latest/download"
    fi

    dir=${DOTVAULT_INSTALL_DIR:-}
    if [ -z "$dir" ]; then
        [ -n "${HOME:-}" ] || err "HOME is not set; pass --dir to choose where to install"
        dir="$HOME/.local/bin"
    fi

    tmp=$(mktemp -d) || err "could not create a temporary folder"
    trap 'rm -rf "$tmp"' EXIT HUP INT TERM

    say "downloading $asset"
    download "$base/$asset" "$tmp/$asset" ||
        err "could not download $base/$asset. Check the version, and that a release exists for $target"
    download "$base/$asset.sha256" "$tmp/$asset.sha256" ||
        err "could not download the checksum file $base/$asset.sha256"

    expected=$(awk '{print tolower($1); exit}' "$tmp/$asset.sha256")
    actual=$(sha256_of "$tmp/$asset")
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
        err "checksum mismatch for $asset (expected '$expected', got '$actual'); nothing was installed"
    fi

    mkdir "$tmp/unpacked"
    tar -xJf "$tmp/$asset" -C "$tmp/unpacked" ||
        err "could not extract $asset (tar needs xz support; on Debian or Ubuntu install the xz-utils package)"
    bin=$(find "$tmp/unpacked" -type f -name "$APP" | head -n 1)
    [ -n "$bin" ] || err "the archive does not contain '$APP'"

    mkdir -p "$dir" || err "could not create $dir"
    # Copy beside the destination, then rename: the swap is atomic and works
    # even if an older copy is running.
    staged="$dir/.$APP.new.$$"
    cp "$bin" "$staged" || err "could not write to $dir"
    chmod 755 "$staged"
    mv -f "$staged" "$dir/$APP" || {
        rm -f "$staged"
        err "could not install to $dir/$APP"
    }

    version=$("$dir/$APP" --version 2>/dev/null) || err "installed $dir/$APP but it does not run on this system"
    say "installed $version to $dir/$APP"

    # PATH entries must be absolute, so compare and set up the absolute form.
    abs=$(cd "$dir" 2>/dev/null && pwd) || abs=$dir
    case ":${PATH:-}:" in
        *":$abs:"*) ;;
        *) path_setup "$abs" ;;
    esac
}

main "$@"
