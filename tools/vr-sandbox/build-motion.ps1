$ErrorActionPreference='Stop'
$vs=& "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath
if(!$vs){throw 'Visual Studio C++ tools not found'}
$out=Join-Path $env:USERPROFILE '.orbiters-build/vr-sandbox'
$command=Join-Path $out 'compile-motion.cmd'
@"
@call "$vs\VC\Auxiliary\Build\vcvars64.bat" >nul
@cl /nologo /std:c++17 /EHsc /MD /O2 "$PSScriptRoot\motion_probe.cpp" /Fo"$out\motion_probe.obj" /link "$out\openvr_api.lib" d3d11.lib ole32.lib windowscodecs.lib /OUT:"$out\motion-probe.exe"
"@ | Set-Content -LiteralPath $command
& $env:ComSpec /c $command
if($LASTEXITCODE -ne 0){throw "Motion probe build failed: $LASTEXITCODE"}
