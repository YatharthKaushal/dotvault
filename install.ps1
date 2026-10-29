# dotvault installer for Windows (PowerShell 5.1 or newer).
#
#   irm https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.ps1 | iex
#
# It downloads the archive for this platform from GitHub Releases, checks its
# SHA-256 checksum, copies dotvault.exe into place and adds that folder to your
# user PATH (and to this PowerShell window, so dotvault works straight away). It
# does not need administrator rights.
#
# Environment variables (set them before running):
#   DOTVAULT_VERSION          release tag to install, for example v0.1.0 (default: latest)
#   DOTVAULT_INSTALL_DIR      where to put dotvault.exe (default: %LOCALAPPDATA%\Programs\dotvault)
#   DOTVAULT_NO_MODIFY_PATH   set to 1 to leave your PATH alone
#   DOTVAULT_DOWNLOAD_URL     base URL that holds the release files (default: GitHub Releases)
#   DOTVAULT_TARGET           use this target triple instead of detecting one

# Adds $Dir to the user PATH and changes nothing else in it. The value is read and
# written through the registry so that entries such as %JAVA_HOME%\bin stay
# unexpanded and the value keeps its type (REG_EXPAND_SZ). Reading it with
# [Environment]::GetEnvironmentVariable expands the variables, and writing it back
# with SetEnvironmentVariable stores plain text, which would freeze every entry at
# today's value.
# Returns $true if it added the folder and $false if it was already listed.
function Add-DotvaultToUserPath {
    param([string]$Dir)
    $normalized = $Dir.TrimEnd('\')
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        $raw = ''
        $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        if ($key.GetValueNames() -contains 'Path') {
            $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $kind = $key.GetValueKind('Path')
            if (($kind -ne [Microsoft.Win32.RegistryValueKind]::String) -and ($kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString)) {
                $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
            }
        }
        # An entry counts as present whether it is written out or through a variable.
        $present = $raw.Split(';') | Where-Object { $_ } | Where-Object {
            [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $normalized
        }
        if ($present) {
            return $false
        }
        if ($raw) {
            $newRaw = $raw.TrimEnd(';') + ';' + $Dir
        } else {
            $newRaw = $Dir
        }
        $key.SetValue('Path', $newRaw, $kind)
    } finally {
        $key.Close()
    }
    # A direct registry write does not tell running programs. Setting and removing a
    # throwaway variable makes .NET broadcast WM_SETTINGCHANGE, so terminals opened
    # from the Start menu see the new PATH without logging off.
    try {
        [Environment]::SetEnvironmentVariable('DOTVAULT_PATH_REFRESH', '1', 'User')
        [Environment]::SetEnvironmentVariable('DOTVAULT_PATH_REFRESH', $null, 'User')
    } catch {
        # Only costs the convenience of not having to log off.
    }
    return $true
}

function Install-Dotvault {
    # Errors use `throw`, not `exit`: when this runs through `iex`, `exit`
    # would close the user's terminal.
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue' # the progress bar makes downloads very slow on 5.1

    $repo = 'YatharthKaushal/dotvault'
    $app = 'dotvault'

    # Windows PowerShell 5.1 does not enable TLS 1.2 by default.
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        # Newer runtimes manage this themselves.
    }

    if ($env:DOTVAULT_TARGET) {
        $target = $env:DOTVAULT_TARGET
    } else {
        $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
        switch ($arch) {
            'AMD64' { $target = 'x86_64-pc-windows-msvc' }
            'ARM64' {
                $target = 'x86_64-pc-windows-msvc'
                Write-Host 'note: there is no native ARM64 build; installing the x64 build, which runs under emulation'
            }
            default { throw "unsupported CPU '$arch' (the prebuilt binary is for x64); build from source instead" }
        }
    }
    $asset = "$app-$target.zip"

    if ($env:DOTVAULT_DOWNLOAD_URL) {
        $base = $env:DOTVAULT_DOWNLOAD_URL.TrimEnd('/')
        Write-Host "using download URL $base"
    } elseif ($env:DOTVAULT_VERSION) {
        $base = "https://github.com/$repo/releases/download/$($env:DOTVAULT_VERSION)"
    } else {
        $base = "https://github.com/$repo/releases/latest/download"
    }

    if ($env:DOTVAULT_INSTALL_DIR) {
        $dir = $env:DOTVAULT_INSTALL_DIR
    } else {
        $dir = Join-Path $env:LOCALAPPDATA 'Programs\dotvault'
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("dotvault-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zip = Join-Path $tmp $asset
        Write-Host "downloading $asset"
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $zip
        } catch {
            throw "could not download $base/$asset. Check the version, and that a release exists for $target. ($($_.Exception.Message))"
        }
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset.sha256" -OutFile "$zip.sha256"
        } catch {
            throw "could not download the checksum file $base/$asset.sha256. ($($_.Exception.Message))"
        }

        $expected = ((Get-Content -LiteralPath "$zip.sha256" -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
        if (-not $expected -or $expected -ne $actual) {
            throw "checksum mismatch for $asset (expected '$expected', got '$actual'); nothing was installed"
        }

        $unpacked = Join-Path $tmp 'unpacked'
        Expand-Archive -LiteralPath $zip -DestinationPath $unpacked -Force
        $exe = Get-ChildItem -LiteralPath $unpacked -Recurse -Filter "$app.exe" | Select-Object -First 1
        if (-not $exe) {
            throw "the archive does not contain $app.exe"
        }

        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $dest = Join-Path $dir "$app.exe"
        try {
            Copy-Item -LiteralPath $exe.FullName -Destination $dest -Force
        } catch {
            throw "could not write $dest. Is dotvault running? Close it and try again. ($($_.Exception.Message))"
        }

        $version = & $dest --version
        if ($LASTEXITCODE -ne 0) {
            throw "installed $dest but it does not run on this system"
        }
        Write-Host "installed $version to $dest"

        if ($env:DOTVAULT_NO_MODIFY_PATH -ne '1') {
            $normalized = $dir.TrimEnd('\')
            try {
                if (Add-DotvaultToUserPath -Dir $dir) {
                    Write-Host "added $dir to your user PATH. dotvault works in this window now; terminals that were already open need to be reopened."
                }
            } catch {
                Write-Host "warning: could not change your user PATH ($($_.Exception.Message)). Add $dir to it yourself to run dotvault from anywhere."
            }
            # Make it usable in this session too.
            if (-not (($env:Path -split ';') | Where-Object { $_ -and $_.TrimEnd('\') -ieq $normalized })) {
                $env:Path = "$env:Path;$dir"
            }
        } else {
            Write-Host "left PATH alone; add $dir to it yourself to run dotvault from anywhere"
        }
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Install-Dotvault
