#Requires -Version 5.1
<#
    Extract ASCII and UTF-16LE strings from a binary for read-only analysis.

    Usage:
      .\extract_strings.ps1 -Path ..\..\AnyDesk.exe -OutDir ..

    Outputs strings_ascii.txt and strings_utf16.txt next to -OutDir.
    This never executes the target.
#>
param(
    [Parameter(Mandatory = $true)][string]$Path,
    [string]$OutDir = '.',
    [int]$MinLength = 6
)

$ErrorActionPreference = 'Stop'
$bytes = [IO.File]::ReadAllBytes((Resolve-Path $Path))
New-Item -ItemType Directory -Force $OutDir | Out-Null

$ascii = New-Object System.Collections.Generic.List[string]
$cur = New-Object System.Text.StringBuilder
foreach ($b in $bytes) {
    if ($b -ge 32 -and $b -lt 127) {
        [void]$cur.Append([char]$b)
    } else {
        if ($cur.Length -ge $MinLength) { $ascii.Add($cur.ToString()) }
        [void]$cur.Clear()
    }
}
if ($cur.Length -ge $MinLength) { $ascii.Add($cur.ToString()) }

$utf16 = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $bytes.Length - 1; $i++) {
    if ([char]$bytes[$i] -ge ' ' -and [char]$bytes[$i] -lt 127 -and $bytes[$i + 1] -eq 0) {
        $sb = New-Object System.Text.StringBuilder
        $j = $i
        while ($j -lt $bytes.Length - 1 -and
               [char]$bytes[$j] -ge ' ' -and [char]$bytes[$j] -lt 127 -and
               $bytes[$j + 1] -eq 0) {
            [void]$sb.Append([char]$bytes[$j])
            $j += 2
        }
        if ($sb.Length -ge $MinLength) { $utf16.Add($sb.ToString()); $i = $j }
    }
}

$ascii | Set-Content -Encoding utf8 (Join-Path $OutDir 'strings_ascii.txt')
$utf16 | Set-Content -Encoding utf8 (Join-Path $OutDir 'strings_utf16.txt')

Write-Host "ASCII strings >= ${MinLength}: $($ascii.Count)"
Write-Host "UTF-16 strings >= ${MinLength}: $($utf16.Count)"
