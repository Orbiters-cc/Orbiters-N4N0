param([switch]$ProbeOnly)
$ErrorActionPreference='Stop'
$root=$PSScriptRoot
$vs=& "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath
if (!$vs) { throw 'Visual Studio C++ tools not found' }
$out='C:\Users\Enzo\.orbiters-build\vr-sandbox\driver'
New-Item -ItemType Directory -Force "$out\bin\win64" | Out-Null
foreach ($asset in @(@('lib','lib/win64/openvr_api.lib'),@('dll','bin/win64/openvr_api.dll'))) {
    $destination=Join-Path (Split-Path $out) "openvr_api.$($asset[0])"
    if (!(Test-Path -LiteralPath $destination)) {
        Invoke-WebRequest "https://raw.githubusercontent.com/ValveSoftware/openvr/v2.12.14/$($asset[1])" -OutFile $destination
    }
}
Copy-Item -LiteralPath "$root\driver.vrdrivermanifest" -Destination "$out\driver.vrdrivermanifest"
$bat=Join-Path $out 'compile.cmd'
$driverCommand=if ($ProbeOnly) { 'rem driver unchanged' } else { "cl /nologo /std:c++17 /EHsc /MD /O2 /LD `"$root\driver.cpp`" /Fo`"$out\driver.obj`" /link /IMPLIB:`"$out\driver.lib`" /OUT:`"$out\bin\win64\driver_orbiters_sandbox.dll`"" }
@"
@set "PATH=${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer;%PATH%"
@call "$vs\VC\Auxiliary\Build\vcvars64.bat" >nul
@$driverCommand
@if errorlevel 1 exit /b %errorlevel%
@cl /nologo /std:c++17 /EHsc /MD /O2 "$root\probe.cpp" /Fo"$out\probe.obj" /link "$out\..\openvr_api.lib" d3d11.lib ole32.lib windowscodecs.lib /OUT:"$out\..\probe.exe"
"@ | Set-Content -LiteralPath $bat
& $env:ComSpec /d /c $bat
if ($LASTEXITCODE -ne 0) { throw "Driver build failed: $LASTEXITCODE" }

& "$out\..\probe.exe" png-self-test "$out\..\encoder-selftest.png"
if($LASTEXITCODE -ne 0){throw "Probe PNG channel self-test failed: $LASTEXITCODE"}
