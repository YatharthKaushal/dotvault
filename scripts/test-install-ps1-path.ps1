# Tests how install.ps1 changes the user PATH, against the real registry of the machine
# that runs it. It saves your Path and puts it back afterwards.
#
#   powershell -File scripts/test-install-ps1-path.ps1
#
# The release files must already be served at $env:DOTVAULT_DOWNLOAD_URL (the CI step
# starts a local server for that). Works in Windows PowerShell 5.1 and PowerShell 7.

$ErrorActionPreference = 'Stop'

if (-not $env:DOTVAULT_DOWNLOAD_URL) {
    throw 'set DOTVAULT_DOWNLOAD_URL to the folder that serves the release files'
}

$installer = Join-Path (Split-Path -Parent $PSScriptRoot) 'install.ps1'
$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
$expand = [Microsoft.Win32.RegistryValueKind]::ExpandString
$plain = [Microsoft.Win32.RegistryValueKind]::String
$noExpand = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames

function Get-RawPath {
    if ($key.GetValueNames() -contains 'Path') {
        return [string]$key.GetValue('Path', '', $noExpand)
    }
    return $null
}

function Get-PathKind {
    if ($key.GetValueNames() -contains 'Path') {
        return $key.GetValueKind('Path').ToString()
    }
    return $null
}

# Runs the installer the way users do (piped into Invoke-Expression) and returns what it said.
function Invoke-Installer {
    $said = Get-Content -LiteralPath $installer -Raw | Invoke-Expression 6>&1
    return ($said | Out-String)
}

$failures = 0
function Check {
    param([string]$Name, [bool]$Condition)
    if ($Condition) {
        Write-Host "ok    $Name"
    } else {
        Write-Host "FAIL  $Name"
        $script:failures = $script:failures + 1
    }
}

$originalRaw = Get-RawPath
$originalKind = Get-PathKind
$originalSession = $env:Path
$originalNoModify = $env:DOTVAULT_NO_MODIFY_PATH
$originalDir = $env:DOTVAULT_INSTALL_DIR
$originalBase = $env:DVT_TEST_BASE

try {
    $base = [IO.Path]::GetTempPath().TrimEnd('\')
    $env:DVT_TEST_BASE = $base
    $leaf = 'dotvault-path-test-' + [Guid]::NewGuid().ToString('N')
    $dir = Join-Path $base $leaf
    $env:DOTVAULT_INSTALL_DIR = $dir
    Remove-Item Env:DOTVAULT_NO_MODIFY_PATH -ErrorAction SilentlyContinue

    # 1. an expandable PATH that holds a variable: nothing else may change
    $seed = '%USERPROFILE%\dotvault-test-bin;C:\Windows'
    $key.SetValue('Path', $seed, $expand)
    $said = Invoke-Installer
    $parts = (Get-RawPath) -split ';'
    Check 'the PATH stays expandable (REG_EXPAND_SZ)' ((Get-PathKind) -eq 'ExpandString')
    Check 'other entries are kept exactly, variables unexpanded' ((Get-RawPath).StartsWith($seed + ';'))
    Check 'the folder is added once, at the end' (($parts.Count -eq 3) -and ($parts[2] -eq $dir))
    Check 'the current session can use it straight away' (($env:Path -split ';') -contains $dir)
    Check 'it says what it did, and that open terminals need reopening' (($said -match 'added .* to your user PATH') -and ($said -match 'reopened'))
    Check 'it installed the program' (Test-Path -LiteralPath (Join-Path $dir 'dotvault.exe'))

    # 2. installing again changes nothing and stays quiet about PATH
    $before = Get-RawPath
    $said = Invoke-Installer
    Check 'a second install leaves the PATH exactly as it was' ((Get-RawPath) -eq $before)
    Check 'a second install does not claim to have added it again' ($said -notmatch 'added .* to your user PATH')

    # 3. a PATH stored as a plain string keeps that type
    $key.SetValue('Path', 'C:\Windows;C:\Tools', $plain)
    Invoke-Installer | Out-Null
    Check 'a plain-string PATH stays a plain string' ((Get-PathKind) -eq 'String')
    Check 'and gets the folder appended' ((Get-RawPath) -eq ('C:\Windows;C:\Tools;' + $dir))

    # 4. no PATH value at all
    $key.DeleteValue('Path', $false)
    Invoke-Installer | Out-Null
    Check 'a missing PATH is created as expandable' ((Get-PathKind) -eq 'ExpandString')
    Check 'and holds just the folder' ((Get-RawPath) -eq $dir)

    # 5. an entry that is already there, written through a variable, in other letters
    $via = '%DVT_TEST_BASE%\' + $leaf.ToUpper() + '\'
    $key.SetValue('Path', ('C:\Windows;' + $via), $expand)
    $before = Get-RawPath
    Invoke-Installer | Out-Null
    Check 'a folder already listed through a variable is recognised, whatever its case' ((Get-RawPath) -eq $before)

    # 6. a trailing semicolon does not leave an empty entry
    $key.SetValue('Path', 'C:\Windows;', $expand)
    Invoke-Installer | Out-Null
    Check 'a trailing semicolon is not doubled' ((Get-RawPath) -eq ('C:\Windows;' + $dir))

    # 7. opting out leaves the registry alone
    $key.SetValue('Path', $seed, $expand)
    $env:DOTVAULT_NO_MODIFY_PATH = '1'
    $said = Invoke-Installer
    Check 'DOTVAULT_NO_MODIFY_PATH=1 leaves the user PATH alone' ((Get-RawPath) -eq $seed)
    Check 'and says so' ($said -match 'left PATH alone')
    Remove-Item Env:DOTVAULT_NO_MODIFY_PATH -ErrorAction SilentlyContinue
}
finally {
    if ($null -ne $originalRaw) {
        $key.SetValue('Path', $originalRaw, [Microsoft.Win32.RegistryValueKind]$originalKind)
    } else {
        $key.DeleteValue('Path', $false)
    }
    $env:Path = $originalSession
    if ($null -ne $originalNoModify) { $env:DOTVAULT_NO_MODIFY_PATH = $originalNoModify } else { Remove-Item Env:DOTVAULT_NO_MODIFY_PATH -ErrorAction SilentlyContinue }
    if ($null -ne $originalDir) { $env:DOTVAULT_INSTALL_DIR = $originalDir } else { Remove-Item Env:DOTVAULT_INSTALL_DIR -ErrorAction SilentlyContinue }
    if ($null -ne $originalBase) { $env:DVT_TEST_BASE = $originalBase } else { Remove-Item Env:DVT_TEST_BASE -ErrorAction SilentlyContinue }
    if ($dir) { Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue }
    $key.Close()
}

if ($failures -gt 0) {
    throw "$failures check(s) failed"
}
Write-Host 'install.ps1 PATH checks passed'
