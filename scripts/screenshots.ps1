# Renders every view of the demo (invented data, generated pictures) into docs\screenshots.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\screenshots.ps1 [-Exe path\to\TwinForge.exe]
param([string]$Exe = "")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Exe) {
    $Exe = @($(if ($env:CARGO_TARGET_DIR) { "$env:CARGO_TARGET_DIR\release\TwinForge.exe" }), "$root\target\release\TwinForge.exe", "$root\dist\TwinForge.exe", "$root\target\debug\TwinForge.exe") |
        Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $Exe) { throw "Build TwinForge first (cargo build --release)." }
$out = Join-Path $root "docs\screenshots"
New-Item -ItemType Directory -Force $out | Out-Null

# name, view, pose, light
$shots = @(
    @("duplicates",        "duplicates",     "selected", $false),
    @("similar-pictures",  "pictures",       "compare",  $false),
    @("compare-large",     "pictures",       "compare-large", $false),
    @("similar-music",     "music",          "compare",  $false),
    @("same-name",         "names",          "selected", $false),
    @("empty-files",       "empty-files",    "selected", $false),
    @("empty-folders",     "empty-folders",  "selected", $false),
    @("broken-shortcuts",  "shortcuts",      "compare",  $false),
    @("biggest-files",     "big",            "",         $false),
    @("temporary-files",   "temp",           "selected", $false),
    @("scan-setup",        "setup",          "",         $false),
    @("scan-setup-pictures", "setup-pictures", "",       $false),
    @("scanning",          "scanning",       "",         $false),
    @("confirm",           "confirm",        "",         $false),
    @("report",            "report",         "",         $false),
    @("history",           "history",        "",         $false),
    @("settings",          "settings",       "",         $false),
    @("about",             "about",          "",         $false),
    @("shortcuts",         "keys",           "",         $false),
    @("light-duplicates",  "duplicates",     "selected", $true),
    @("light-pictures",    "pictures",       "compare",  $true),
    @("light-setup",       "setup",          "",         $true)
)
foreach ($s in $shots) {
    $png = Join-Path $out ($s[0] + ".png")
    $a = @("--demo", "--screenshot", $png, "--view", $s[1])
    if ($s[2]) { $a += @("--pose", $s[2]) }
    if ($s[3]) { $a += "--light" }
    $p = Start-Process -FilePath $Exe -ArgumentList $a -PassThru -Wait
    if (-not (Test-Path $png)) { throw "No screenshot for $($s[0])" }
    Write-Host ("{0,-22} {1,8:N0} bytes" -f $s[0], (Get-Item $png).Length)
}
