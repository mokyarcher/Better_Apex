param([string]$RepoRoot = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationFramework,PresentationCore,WindowsBase
$repoPath = [IO.Path]::GetFullPath($RepoRoot)
$testPath = Join-Path $repoPath ('.local\rust-capture-tests\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testPath -Force | Out-Null
$cli = Join-Path $repoPath '.local\rust-target\debug\compatibility-cli.exe'
if (!(Test-Path -LiteralPath $cli)) { throw 'Build the compatibility-cli first.' }
$app = New-Object Windows.Application
$app.ShutdownMode = 'OnExplicitShutdown'
$grid = New-Object Windows.Controls.Grid
$grid.Background = [Windows.Media.Brushes]::CornflowerBlue
foreach ($corner in @(@('Red','Left','Top'),@('Lime','Right','Bottom'))) {
    $block = New-Object Windows.Controls.Border
    $block.Background = (New-Object Windows.Media.BrushConverter).ConvertFromString($corner[0]); $block.Width = 64; $block.Height = 64
    $block.HorizontalAlignment = $corner[1]; $block.VerticalAlignment = $corner[2]
    [void]$grid.Children.Add($block)
}
$counter = New-Object Windows.Controls.TextBlock
$counter.HorizontalAlignment='Center';$counter.VerticalAlignment='Center';$counter.FontSize=24
[void]$grid.Children.Add($counter)
$fixture = New-Object Windows.Window
$fixture.Title='Better Apex Rust Capture Fixture';$fixture.WindowStyle='None';$fixture.ResizeMode='NoResize'
$fixture.Left=0;$fixture.Top=0;$fixture.Content=$grid
$fixture.Show()
$dpi = [Windows.Media.VisualTreeHelper]::GetDpi($fixture)
$fixture.Width=2560/$dpi.DpiScaleX;$fixture.Height=1440/$dpi.DpiScaleY
$handle = (New-Object Windows.Interop.WindowInteropHelper($fixture)).Handle
$cover = New-Object Windows.Window
$cover.Title='Rust capture pause fixture';$cover.Width=320;$cover.Height=180
$script:stage=0;$script:count=0;$script:failure=$null
$process = Start-Process -FilePath $cli -ArgumentList @('--capture-window',$handle.ToInt64(),$PID,('"'+$testPath+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $testPath 'stdout.log') -RedirectStandardError (Join-Path $testPath 'stderr.log')
$null = $process.Handle
[void]$fixture.Activate()
$timer = New-Object Windows.Threading.DispatcherTimer
$timer.Interval=[TimeSpan]::FromMilliseconds(100)
$timer.Add_Tick({
    $script:count++;$counter.Text='Own fixture frame '+$script:count
    if ($script:stage -eq 0 -and (Test-Path -LiteralPath (Join-Path $testPath 'frames-ready'))) { $script:stage=1;$cover.Show();[void]$cover.Activate() }
    if ($script:stage -eq 1 -and (Test-Path -LiteralPath (Join-Path $testPath 'pause-verified'))) { $script:stage=2;$cover.Close();[void]$fixture.Activate() }
    $process.Refresh()
    if ($process.HasExited) { $timer.Stop();$fixture.Close();$app.Shutdown() }
    elseif ($script:count -gt 450) { $script:failure='Fixture timeout';$timer.Stop();Stop-Process -Id $process.Id -Force;$fixture.Close();$cover.Close();$app.Shutdown() }
})
$timer.Start()
try { [void]$app.Run() } finally { $timer.Stop();if (!$process.HasExited) {Stop-Process -Id $process.Id -Force} }
$process.WaitForExit()
if ($script:failure -or $process.ExitCode -ne 0) {Get-Content (Join-Path $testPath 'stderr.log');throw 'Rust capture fixture failed.'}
Get-Content (Join-Path $testPath 'report.json')
Write-Output ('RUST CAPTURE PASS: '+$testPath)
