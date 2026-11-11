$ErrorActionPreference='Stop'
$root='__ROOT__'
$owned='__PATH__'
function SafeFolder($p) {
    $full=[IO.Path]::GetFullPath($p)
    if ([IO.Path]::GetDirectoryName($full) -ne $root -or [IO.Path]::GetFileName($full) -notmatch '^session-[A-Za-z0-9]+$') { throw 'Unowned profile' }
    if ((Get-Item -LiteralPath $full).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Linked profile' }
}
function Clean($p) {
    SafeFolder $p
    $escaped=[regex]::Escape($p)
    foreach ($browser in (Get-CimInstance Win32_Process -Filter "name='msedgewebview2.exe'")) {
        if ($browser.CommandLine -match ('--user-data-dir="?' + $escaped + '(?:[\\"]|\s|$)')) {
            Stop-Process -Id $browser.ProcessId -Force -ErrorAction SilentlyContinue
        }
    }
    foreach ($browser in (Get-CimInstance Win32_Process -Filter "name='msedgewebview2.exe'")) {
        if ($browser.CommandLine -match ('--user-data-dir="?' + $escaped + '(?:[\\"]|\s|$)')) { throw 'Browser exit pending' }
    }
    if (Get-ChildItem -LiteralPath $p -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint } | Select-Object -First 1) { throw 'Linked profile content' }
    Remove-Item -LiteralPath $p -Recurse -Force
}
SafeFolder $owned
$owner=Get-Process -Id __PID__
$null=$owner.Handle
$retry=New-Object 'System.Collections.Generic.HashSet[string]'
function RetryCleanup {
    foreach ($p in @($retry)) {
        try {
            if ($p -ne $owned -and (Test-Path -LiteralPath $p)) {
                $record=Join-Path $p 'owner.lock'
                if (Test-Path -LiteralPath $record) {
                    try {
                        $probe=[IO.File]::Open($record,'Open','ReadWrite','None')
                        $probe.Dispose()
                    } catch { $null=$retry.Remove($p); continue }
                } elseif ((Get-Item -LiteralPath $p).LastWriteTimeUtc -gt [DateTime]::UtcNow.AddSeconds(-30)) { continue }
            }
            if (Test-Path -LiteralPath $p) { Clean $p }
            $null=$retry.Remove($p)
        } catch { }
    }
}
Get-ChildItem -LiteralPath $root -Directory | ForEach-Object {
    if ($_.FullName -ne $owned) {
        try {
            SafeFolder $_.FullName
            $record=Join-Path $_.FullName 'owner.lock'
            if (Test-Path -LiteralPath $record) {
                $probe=[IO.File]::Open($record,'Open','ReadWrite','None')
                $probe.Dispose()
            }
            $null=$retry.Add($_.FullName)
        } catch { }
    }
}
RetryCleanup
[IO.File]::WriteAllText((Join-Path $owned 'ready'),'')
while (!$owner.HasExited -and !(Test-Path -LiteralPath (Join-Path $owned 'done'))) {
    RetryCleanup
    Start-Sleep -Milliseconds 100
}
$null=$retry.Add($owned)
while ($retry.Count -gt 0) { RetryCleanup; if ($retry.Count -gt 0) { Start-Sleep -Milliseconds 250 } }
