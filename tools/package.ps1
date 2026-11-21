# 把 exe + 动画包打成可直接分发的 zip（Release 用，本地也能跑）：
#   powershell -ExecutionPolicy Bypass -File tools\package.ps1 -Version v0.1.0
# 产物：target\claude-pet-<版本>-win64.zip，解压后是一个 claude-pet\ 文件夹。
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Exe = "target\release\claude-pet.exe"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not (Test-Path $Exe)) { throw "找不到 $Exe，先运行 cargo build --release" }

$stage = Join-Path $root "target\package\claude-pet"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item $Exe (Join-Path $stage "claude-pet.exe")
Copy-Item -Recurse "dist\pets" (Join-Path $stage "pets")
Copy-Item "README.md", "LICENSE" $stage
# README 里链接了 docs/*.md，一起带上（预览动图不带）
New-Item -ItemType Directory -Force (Join-Path $stage "docs") | Out-Null
Copy-Item "docs\*.md" (Join-Path $stage "docs")

$zip = Join-Path $root "target\claude-pet-$Version-win64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
# 逐个文件写入并手动用正斜杠命名：Windows PowerShell 5.1 自带的打包方式会写成反斜杠，
# 别的解压工具（以及非 Windows 系统）会把它当成文件名的一部分
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$base = Split-Path -Parent $stage
$archive = [IO.Compression.ZipFile]::Open($zip, [IO.Compression.ZipArchiveMode]::Create)
try {
    Get-ChildItem -Recurse -File $stage | ForEach-Object {
        $name = $_.FullName.Substring($base.Length + 1).Replace('\', '/')
        [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $_.FullName, $name, [IO.Compression.CompressionLevel]::Optimal)
    }
} finally {
    $archive.Dispose()
}
Write-Host "已生成 $zip"
