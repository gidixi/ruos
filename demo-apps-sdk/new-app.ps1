<#
.SYNOPSIS
  Scaffold a new ruos GUI window app from templates/window-app.

.DESCRIPTION
  Copies templates/window-app to apps/<Name>, rewrites the package name and
  manifest (id/title/size). The workspace picks it up automatically (members glob
  apps/*), so no manifest edit is needed. Prompts for anything not passed.

.EXAMPLE
  .\new-app.ps1
  .\new-app.ps1 -Name browser -Title "Browser" -Width 900 -Height 640
#>
param(
  [string]$Name,
  [string]$Id,
  [string]$Title,
  [int]$Width  = 0,
  [int]$Height = 0,
  # Project root (where templates/ + apps/ live). Default = this script's folder.
  [string]$Path = $PSScriptRoot
)
$ErrorActionPreference = "Stop"
$proj = (Resolve-Path $Path).Path

if (-not $Name)  { $Name  = Read-Host "App name (kebab-case dir + crate, e.g. browser)" }
$Name = $Name.Trim().ToLower()
if ($Name -notmatch '^[a-z][a-z0-9-]*$') { throw "Name must be lowercase kebab-case (got '$Name')" }

if (-not $Id)    { $Id = Read-Host "Manifest id / .cwasm stem [$Name]"; if (-not $Id) { $Id = $Name } }
$Id = $Id.Trim().ToLower()
if ($Id -notmatch '^[a-z][a-z0-9-]*$') { throw "Id must be lowercase kebab-case (got '$Id')" }

if (-not $Title) {
  $def = (Get-Culture).TextInfo.ToTitleCase($Name -replace '-',' ')
  $Title = Read-Host "Launcher title [$def]"; if (-not $Title) { $Title = $def }
}
if ($Width  -le 0) { $w = Read-Host "Window width  [640]"; $Width  = if ($w) { [int]$w } else { 640 } }
if ($Height -le 0) { $h = Read-Host "Window height [480]"; $Height = if ($h) { [int]$h } else { 480 } }

$tpl = Join-Path $proj "templates\window-app"
if (-not (Test-Path $tpl)) { throw "templates\window-app missing under '$proj' - run .\bootstrap.ps1 -Path '$proj' first" }
$dst = Join-Path $proj "apps\$Name"
if (Test-Path $dst) { throw "apps\$Name already exists" }
Copy-Item $tpl $dst -Recurse

# Cargo.toml: package name
$ct = Join-Path $dst "Cargo.toml"
(Get-Content $ct -Raw).Replace('name = "window-app"', "name = `"$Name`"") | Set-Content $ct -NoNewline

# src/lib.rs: manifest + sizes + titles
$lib = Join-Path $dst "src\lib.rs"
$code = Get-Content $lib -Raw
$code = $code.Replace('declare_manifest!("window-app", "Window App", 480, 320);',
                      "declare_manifest!(`"$Id`", `"$Title`", $Width, $Height);")
$code = $code.Replace('const W: u32 = 480;', "const W: u32 = $Width;")
$code = $code.Replace('const H: u32 = 320;', "const H: u32 = $Height;")
$code = $code.Replace('frame_once(s, "Window App"', "frame_once(s, `"$Title`"")
$code = $code.Replace('ui.heading("Window App");', "ui.heading(`"$Title`");")
Set-Content $lib $code -NoNewline

Write-Host ""
Write-Host "Scaffolded apps/$Name  (id='$Id', title='$Title', ${Width}x${Height})" -ForegroundColor Green
Write-Host "Next:" -ForegroundColor Cyan
Write-Host "  .\build.ps1 -App $Name -Id $Id"
Write-Host "  .\deploy.ps1"
