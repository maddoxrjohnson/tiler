# Records the demo: starts tiler, switches to an empty workspace (your own
# windows stay parked on workspace 1), opens colored demo windows over a dark
# backdrop, drives tiler through its CLI, and logs when each command ran so the
# video can show the matching shortcut. Afterwards it closes the demo windows and
# quits tiler, which puts your windows back.
#
# Requires: a release build (cargo build --release) and FFmpeg
# (winget install Gyan.FFmpeg). Don't touch the mouse or keyboard while it runs.
# Output: $env:TEMP\tiler-demo\raw.mp4 and steps.csv.

$ErrorActionPreference = 'Continue'
$env:PATH = [Environment]::GetEnvironmentVariable('PATH','User') + ';' + $env:LOCALAPPDATA + '\Microsoft\WinGet\Links;' + $env:PATH
$t = Join-Path $PSScriptRoot '..\..\target\release\tiler.exe'
$d = "$env:TEMP\tiler-demo"
New-Item -ItemType Directory -Force $d | Out-Null
$log = "$d\steps.csv"
'step,keys,seconds' | Set-Content $log
$script:pids = @()

$backdrop = Start-Process -PassThru -WindowStyle Hidden powershell.exe -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',"$PSScriptRoot\backdrop.ps1"
Start-Sleep 3
$srv = Start-Process -PassThru -NoNewWindow -FilePath $t -ArgumentList 'server' -RedirectStandardError "$d\server.log"
Start-Sleep 2
& $t workspace 5 | Out-Null
Start-Sleep 1

$rec = Start-Process -PassThru -NoNewWindow -FilePath ffmpeg -RedirectStandardError "$d\ffmpeg.log" -ArgumentList @(
  '-hide_banner','-y','-f','gdigrab','-draw_mouse','0','-framerate','30','-offset_x','0','-offset_y','0',
  '-video_size','2240x1328','-i','desktop','-c:v','libx264','-preset','ultrafast','-crf','18',
  '-pix_fmt','yuv420p','-t','82',"$d\raw.mp4")
$sw = [Diagnostics.Stopwatch]::StartNew()
function Step($name, $keys) { "$name,$keys,$([math]::Round($sw.Elapsed.TotalSeconds,2))" | Add-Content $log }
function Run($name, $keys, [string[]]$cmd, $pause = 2.5) { Step $name $keys; & $t @cmd | Out-Null; Start-Sleep -Milliseconds ([int]($pause*1000)) }
function Open($label, $color, $lines) {
  Step "open $label" ''
  $body = "function prompt { '> ' }; Set-Location C:\; `$Host.UI.RawUI.WindowTitle='$label'; `$Host.UI.RawUI.BackgroundColor='$color'; Clear-Host; Write-Host ''; Write-Host '   $label' -ForegroundColor White; Write-Host ''; " + (($lines | ForEach-Object { "Write-Host '   $_' -ForegroundColor Gray;" }) -join ' ')
  $p = Start-Process -PassThru conhost.exe -ArgumentList "powershell.exe -NoLogo -NoProfile -NoExit -Command `"$body`""
  $script:pids += $p.Id
  Start-Sleep -Milliseconds 2600
}

Start-Sleep 2
Open 'editor' 'DarkBlue'    @('fn main() {', '    tiler::run();', '}')
Open 'server' 'DarkMagenta' @('listening on :8080', 'GET /health 200')
Open 'tests'  'DarkGreen'   @('running 140 tests', 'test result: ok')
Open 'notes'  'DarkRed'     @('- ship v0.1.0', '- record demo')
Run 'move left'  'alt-shift-h' @('move','left')
Run 'move down'  'alt-shift-j' @('move','down') 3
Run 'grow'       'alt-equal'   @('resize','smart','+250')
Run 'focus up'   'alt-k'       @('focus','up') 1.5
Run 'accordion'  'alt-comma'   @('layout','accordion','horizontal','vertical') 3
Run 'tiles'      'alt-slash'   @('layout','tiles','horizontal','vertical') 2.5
Step 'open calculator' ''
Start-Process calc
Start-Sleep -Milliseconds 3500
Run 'float'            'alt-shift-space' @('layout','floating','tiling') 3
Run 'to workspace 6'   'alt-shift-6'     @('move-node-to-workspace','6') 2.5
Run 'workspace 6'      'alt-6'           @('workspace','6') 2.5
Run 'workspace 5'      'alt-5'           @('workspace','5') 2.5
Run 'fullscreen'       'alt-f'           @('fullscreen') 2.5
Run 'fullscreen off'   'alt-f'           @('fullscreen') 2
Run 'flatten'          'alt-shift-semicolon r' @('flatten-workspace-tree') 2.5
Run 'balance'          'alt-shift-semicolon b' @('balance-sizes') 3
Step 'end' ''

$rec.WaitForExit()
# Clean up: demo windows (and the shells inside them), calculator, then tiler.
$children = Get-CimInstance Win32_Process | Where-Object { $script:pids -contains $_.ParentProcessId } | ForEach-Object { $_.ProcessId }
$script:pids + $children | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
Get-Process CalculatorApp -ErrorAction SilentlyContinue | Stop-Process -Force
Stop-Process -Id $backdrop.Id -Force -ErrorAction SilentlyContinue
Start-Sleep 1
& $t workspace 1 | Out-Null
Start-Sleep 1
& $t quit
Start-Sleep 1
"server exited: " + $srv.HasExited
& $t rescue
ffprobe -v error -show_entries format=duration:stream=width,height -of csv=p=0 "$d\raw.mp4"
