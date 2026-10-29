# Install the Bynk toolchain (bynkc, bynk, bynkc-lsp) from a GitHub Release,
# without needing Rust or cargo.
#
#   irm https://github.com/bynk-lang/bynk/releases/latest/download/install.ps1 | iex
#
# Options (environment variables):
#   BYNK_VERSION      release tag to install (e.g. v0.16.0); default: the latest
#   BYNK_INSTALL_DIR  where to put the binaries; default: %LOCALAPPDATA%\Programs\bynk\bin
#   BYNK_NO_MODIFY_PATH  set to any value to leave the user's PATH alone
#
# The archive is verified against the release's SHA256SUMS before anything is
# installed. The install directory is added to the user's PATH.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# Windows PowerShell 5.1 does not negotiate TLS 1.2 by default; GitHub needs it.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = 'bynk-lang/bynk'
$base = "https://github.com/$repo/releases"
$target = 'x86_64-pc-windows-msvc'
if (-not [Environment]::Is64BitOperatingSystem) {
    throw "No prebuilt Bynk binaries for 32-bit Windows; use 'cargo install bynkc bynk' instead."
}
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    Write-Warning 'No native ARM64 build; installing the x64 binaries, which run under emulation.'
}

$dest = if ($env:BYNK_INSTALL_DIR) { $env:BYNK_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\bynk\bin' }

$tag = $env:BYNK_VERSION
if (-not $tag) {
    # /releases/latest redirects to /releases/tag/<tag>.
    $req = [System.Net.WebRequest]::Create("$base/latest")
    $req.AllowAutoRedirect = $false
    $res = $req.GetResponse()
    $loc = $res.Headers['Location']
    $res.Close()
    if ($loc -notmatch '/tag/') { throw 'Could not determine the latest release.' }
    $tag = ($loc -split '/')[-1]
}
if (-not $tag.StartsWith('v')) { $tag = "v$tag" }

$stage = "bynk-$tag-$target"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Installing Bynk $tag ($target) to $dest"
    Invoke-WebRequest "$base/download/$tag/$stage.zip" -OutFile (Join-Path $tmp "$stage.zip")
    Invoke-WebRequest "$base/download/$tag/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS')

    $line = Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { $_ -match "\s$([regex]::Escape("$stage.zip"))$" } | Select-Object -First 1
    if (-not $line) { throw "$stage.zip is not listed in SHA256SUMS." }
    $want = ($line -split '\s+')[0]
    $got = (Get-FileHash (Join-Path $tmp "$stage.zip") -Algorithm SHA256).Hash
    if ($want -ne $got) { throw "Checksum mismatch for $stage.zip (expected $want, got $got)." }

    Expand-Archive (Join-Path $tmp "$stage.zip") -DestinationPath $tmp
    New-Item -ItemType Directory -Path $dest -Force | Out-Null
    foreach ($bin in 'bynkc', 'bynk', 'bynkc-lsp') {
        Copy-Item (Join-Path $tmp "$stage\$bin.exe") (Join-Path $dest "$bin.exe") -Force
    }
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# Read the raw registry value so %VAR% entries survive, and write it back as
# REG_EXPAND_SZ (what [Environment]::SetEnvironmentVariable would flatten).
$key = (Get-Item 'HKCU:\Environment')
$userPath = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
if ($env:BYNK_NO_MODIFY_PATH) {
    Write-Host "Skipped the PATH update (BYNK_NO_MODIFY_PATH is set); add $dest to your PATH yourself."
} elseif (($userPath -split ';') -notcontains $dest) {
    $newPath = if ($userPath) { "$userPath;$dest" } else { $dest }
    Set-ItemProperty -Path 'HKCU:\Environment' -Name Path -Value $newPath -Type ExpandString
    $env:Path = "$env:Path;$dest"
    Write-Host "Added $dest to your PATH (open a new terminal to pick it up)."
}
Write-Host "Installed bynkc, bynk and bynkc-lsp. Run 'bynk doctor' to check your setup."
