# dotvault installer for Windows (PowerShell 5.1 or newer).
#
#   irm https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.ps1 | iex
#
# It downloads the archive for this platform from GitHub Releases, checks its
# SHA-256 checksum, copies dotvault.exe into place and adds that folder to your
# user PATH. It does not need administrator rights.
#
# Environment variables (set them before running):
#   DOTVAULT_VERSION          release tag to install, for example v0.1.0 (default: latest)
#   DOTVAULT_INSTALL_DIR      where to put dotvault.exe (default: %LOCALAPPDATA%\Programs\dotvault)
#   DOTVAULT_NO_MODIFY_PATH   set to 1 to leave your PATH alone
#   DOTVAULT_DOWNLOAD_URL     base URL that holds the release files (default: GitHub Releases)
#   DOTVAULT_TARGET           use this target triple instead of detecting one

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
            $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
            $entries = @()
            if ($userPath) {
                $entries = $userPath.Split(';') | Where-Object { $_ }
            }
            $normalized = $dir.TrimEnd('\')
            $present = $entries | Where-Object { $_.TrimEnd('\') -ieq $normalized }
            if (-not $present) {
                $newPath = (@($entries) + $dir) -join ';'
                [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
                Write-Host "added $dir to your user PATH; open a new terminal to use it"
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
