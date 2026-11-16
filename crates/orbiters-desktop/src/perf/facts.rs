//! What the Performance report asks Windows (`report.rs`): a PowerShell script and the names of programs to look for. Read by
//! the program, never shown.

/// Programs that matter beside VRChat in VR: overlays, VR tools, streamers, recorders, other game overlays.
pub const KNOWN_PROGRAMS: &[&str] = &[
    "OVR Toolkit", "XSOverlay", "AdvancedSettings", "OVRAdvancedSettings", "SpaceCalibrator", "fpsVR", "OVRdrop", "DesktopPlus", "VRCX", "VRCFaceTracking", "VRCOSC", "VirtualDesktop.Streamer",
    "VirtualDesktop.Service", "OVRServer_x64", "OculusClient", "SlimeVR", "slimevr", "Standable", "VRChatOSCRouter", "Wallpaper64", "wallpaper32", "obs64", "Discord", "MSIAfterburner", "RTSS",
    "NVIDIA Overlay", "NVIDIA Share", "GameBar", "XboxGameBarWidgets", "Medal", "Overwolf", "VRCVideoCacher", "ALVR", "PimaxClient", "Bigscreen", "Kinect", "Spout",
    "VoiceMeeter", "voicemeeter8x64", "NVBroadcast", "iCUE", "LGHUB", "SignalRgb", "ArmouryCrate", "lghub_agent", "Razer Synapse 3", "WMIRegistrationService",
];

/// PowerShell's arguments before the script.
pub const POWERSHELL_ARGS: [&str; 5] = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"];

/// The script: Windows' name and build, the antivirus, the power plan and battery, the OpenXR runtime, Game Mode and
/// hardware scheduling, and three seconds of the graphics card's counters for the processes `pids` (comma-separated).
pub fn script(pids: &str) -> String {
    format!(
        r#"$ErrorActionPreference='SilentlyContinue'
$o=[ordered]@{{}}
$os=Get-CimInstance Win32_OperatingSystem
$o.os="$($os.Caption) $($os.Version)"
$r=Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$o.build="$($r.DisplayVersion) $($r.CurrentBuild).$($r.UBR)"
$o.antivirus=@(Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntiVirusProduct | ForEach-Object {{ $_.displayName }})
$o.power_plan=((powercfg /getactivescheme) -replace '^.*\(','' -replace '\)\s*$','')
$b=Get-CimInstance Win32_Battery
$o.battery=[bool]$b
$o.on_battery=[bool]($b -and $b.BatteryStatus -eq 1)
$o.openxr_runtime=(Get-ItemProperty 'HKLM:\SOFTWARE\Khronos\OpenXR\1').ActiveRuntime -replace '^.*\\',''
$o.game_mode=(Get-ItemProperty 'HKCU:\Software\Microsoft\GameBar').AutoGameModeEnabled
$o.hags=(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers').HwSchMode
$pids=@({pids})
$c=Get-Counter -Counter '\GPU Engine(*)\Utilization Percentage','\GPU Process Memory(*)\Dedicated Usage','\GPU Adapter Memory(*)\Dedicated Usage' -SampleInterval 1 -MaxSamples 3
$g=@{{}}
foreach($s in $c.CounterSamples){{ if($s.InstanceName -match 'pid_(\d+)_'){{ $p=[int]$Matches[1]; if($pids -contains $p){{ $k="$p"; if(-not $g[$k]){{ $g[$k]=@{{gpu_percent=0.0;dedicated_mb=0.0}} }}; if($s.Path -like '*utilization percentage'){{ $g[$k].gpu_percent+=$s.CookedValue/3 }} else {{ $g[$k].dedicated_mb=[math]::Max($g[$k].dedicated_mb,$s.CookedValue/1MB) }} }} }} elseif($s.Path -like '*gpu adapter memory*'){{ $o.adapter_dedicated_used_mb=[math]::Max([double]$o.adapter_dedicated_used_mb,$s.CookedValue/1MB) }} }}
$o.gpu_by_pid=$g
$o | ConvertTo-Json -Depth 4 -Compress"#
    )
}
