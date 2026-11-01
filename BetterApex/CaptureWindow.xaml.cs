using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Threading;
using BetterApex.Capture;
using Microsoft.Win32;
using BetterApex.Storage;

namespace BetterApex;

public partial class CaptureWindow : Window
{
    private readonly DispatcherTimer _timer;
    private IWindowCapture? _capture;
    private CapturedFrame? _displayed;
    private bool _transitioning, _closing;
    private string? _savedMessage;
    private Task? _stopTask;
    private readonly MatchRepository _repository;
    private volatile AutoRecorder? _autoRecorder;
    private CancellationTokenSource? _autoCancellation;
    private Task? _autoTask;
    private HistoryWindow? _autoHistory;

    public CaptureWindow() : this(new MatchRepository(MatchRepository.DefaultDataDirectory())) { }

    internal CaptureWindow(MatchRepository repository)
    {
        _repository = repository;
        InitializeComponent();
        _timer = new DispatcherTimer(TimeSpan.FromMilliseconds(250), DispatcherPriority.Background,
            (_, _) => UpdatePreview(), Dispatcher);
        _timer.Stop();
        RefreshWindows();
    }

    private void RefreshWindows_Click(object sender, RoutedEventArgs e) => RefreshWindows();

    private void RefreshWindows()
    {
        try
        {
            var previous = (TargetWindows.SelectedItem as WindowTarget)?.Handle;
            var windows = WindowCatalog.List();
            TargetWindows.ItemsSource = windows;
            TargetWindows.SelectedItem = windows.FirstOrDefault(w => w.Handle == previous)
                ?? windows.FirstOrDefault(w => w.ProcessName.StartsWith("r5apex", StringComparison.OrdinalIgnoreCase));
            CaptureStatusText.Text = windows.Count == 0 ? "未发现可采集窗口，请先启动游戏后刷新。" : "请选择 Apex 窗口，再点击“开始自动记录”。";
            CaptureErrorText.Text = "";
        }
        catch (Exception e)
        {
            System.Diagnostics.Trace.TraceError(e.ToString());
            CaptureErrorText.Text = "无法读取窗口列表，请稍后刷新。";
        }
        UpdateButtons();
    }

    private void TargetChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!IsInitialized) return;
        _displayed = null;
        _savedMessage = null;
        CapturePreview.Source = null;
        ShowWaiting("尚未开始采集");
        CaptureDetailsText.Text = "自动记录会在局内采集昵称，到结算时识别、关联并保存。";
        UpdateButtons();
    }

    private void StartCapture_Click(object sender, RoutedEventArgs e)
        => StartCapture(automatic: false);

    private void AutoRecord_Click(object sender, RoutedEventArgs e) => StartCapture(automatic: true);

    private async void StartCapture(bool automatic)
    {
        if (_capture is not null || _transitioning || TargetWindows.SelectedItem is not WindowTarget target) return;
        CaptureErrorText.Text = "";
        _displayed = null;
        _savedMessage = null;
        CapturePreview.Source = null;
        ShowWaiting("正在等待游戏画面…");
        _stopTask = null;
        _transitioning = true;
        UpdateButtons();
        try
        {
            if (WindowCompatibility.IsChecked == true)
            {
                var border = HideCaptureBorder.IsChecked == true ? await CaptureBorder.RequestAsync() : new BorderAccess(false, "显示采集黄框");
                if (_closing) return;
                var windowCapture = new GameWindowCapture(target, TimeSpan.FromSeconds(2), border.Allowed);
                _capture = windowCapture;
                BorderStatusText.Text = BorderMessage(windowCapture, HideCaptureBorder.IsChecked == true, border);
            }
            else
            {
                _capture = new DxgiWindowCapture(target);
                BorderStatusText.Text = "DXGI 无黄框 · 每 2 秒最多 1 次常规截屏，结算候选仅补 1 帧校验";
            }
            _autoRecorder = null; _autoTask = null;
            AutoErrorText.Text = ""; AutoSavedText.Text = ""; AutoRosterText.Text = "本场名单：未采集";
            if (automatic)
            {
                var capture = _capture;
                _autoCancellation = new CancellationTokenSource();
                _autoTask = AutoRecorder.RunAsync(_repository,
                    () => capture.CanAnalyze ? capture.Latest : null,
                    recorder => _autoRecorder = recorder, _autoCancellation.Token, capture.RequestVerificationFrame);
                AutoPhaseText.Text = "自动记录：正在启动…";
            }
            else AutoPhaseText.Text = "仅预览：不自动识别或保存战绩";
            CaptureStatusText.Text = $"正在采集：{target.Title}";
            CaptureDetailsText.Text = "等待第一张有效画面；游戏最小化时请先恢复窗口。";
            _timer.Start();
        }
        catch (Exception exception)
        {
            System.Diagnostics.Trace.TraceError(exception.ToString());
            ShowWaiting("尚未取得画面");
            CaptureStatusText.Text = "未能开始采集。";
            CaptureErrorText.Text = exception switch
            {
                NotSupportedException or InvalidOperationException => exception.Message,
                UnauthorizedAccessException => "Windows 未允许采集该窗口，请检查系统录屏限制并尝试无边框模式。",
                _ => "无法采集这个窗口，请恢复游戏窗口并使用无边框窗口模式重试。"
            } + $"（错误代码：0x{exception.HResult:X8}）";
        }
        finally { _transitioning = false; UpdateButtons(); }
    }

    private async void StopCapture_Click(object sender, RoutedEventArgs e) => await StopCaptureAsync();

    private Task StopCaptureAsync()
    {
        var capture = _capture;
        return capture is null ? Task.CompletedTask : _stopTask ??= StopCaptureCoreAsync(capture);
    }

    private async Task StopCaptureCoreAsync(IWindowCapture capture)
    {
        _transitioning = true;
        _timer.Stop();
        UpdateButtons();
        try
        {
            _autoCancellation?.Cancel();
            AutoPhaseText.Text = "正在停止；已捕获的结算将完成保存…";
            await Task.WhenAll(capture.DisposeAsync().AsTask(), _autoTask ?? Task.CompletedTask);
        }
        catch (Exception e) { System.Diagnostics.Trace.TraceError(e.ToString()); }
        finally
        {
            _capture = null;
            _transitioning = false;
            UpdateAutomaticStatus();
            _autoCancellation?.Dispose(); _autoCancellation = null;
            AutoPhaseText.Text = "自动记录已停止";
            CaptureStatusText.Text = _displayed is null ? "采集已停止。" : "采集已停止，当前显示最后一张截图。";
            if (_displayed is null) ShowWaiting("采集已停止");
            else WaitingOverlay.Visibility = Visibility.Collapsed;
            UpdateButtons();
        }
    }

    private void UpdatePreview()
    {
        UpdateAutomaticStatus();
        if (_capture is not { } capture) return;
        if (capture.PauseReason is { } pause)
        {
            ShowWaiting(pause);
            CaptureStatusText.Text = pause;
            SaveCaptureButton.IsEnabled = false;
            return;
        }
        if (capture.TargetClosed || capture.Error is not null)
        {
            ShowWaiting(capture.TargetClosed ? "游戏窗口已关闭" : "采集已中断");
            CaptureErrorText.Text = capture.Error ?? "目标窗口已关闭，请停止采集后刷新窗口列表。";
            SaveCaptureButton.IsEnabled = false;
            return;
        }
        if (capture.IsMinimized)
        {
            ShowWaiting("游戏窗口已最小化，等待恢复…");
            CaptureStatusText.Text = "等待游戏恢复；背景为上一次画面。";
            SaveCaptureButton.IsEnabled = false;
            return;
        }
        var frame = capture.Latest;
        if (frame is null || frame.IsBlack || System.Diagnostics.Stopwatch.GetElapsedTime(frame.ReceivedTimestamp) > TimeSpan.FromSeconds(3))
        {
            ShowWaiting(frame?.IsBlack == true ? "当前为黑屏，等待有效画面…" : "等待新的游戏画面…");
            CaptureStatusText.Text = "尚未取得可用的新画面；如已进入游戏，请检查窗口模式。";
            SaveCaptureButton.IsEnabled = false;
            return;
        }
        if (_displayed?.Sequence != frame.Sequence)
        {
            _displayed = frame;
            CapturePreview.Source = frame.Image;
        }
        WaitingOverlay.Visibility = Visibility.Collapsed;
        CaptureStatusText.Text = $"正在采集：{capture.Target.Title}";
        CaptureDetailsText.Text = $"{frame.Image.PixelWidth} × {frame.Image.PixelHeight} 像素 · 最近采集 {frame.CapturedAt.ToLocalTime():HH:mm:ss}"
            + (_savedMessage is null ? "" : $"\n{_savedMessage}");
        if (capture is DxgiWindowCapture dxgi)
        {
            var metrics = dxgi.Metrics;
            CaptureDetailsText.Text += $" · 已取 {metrics.Frames} 帧 · 平均采样 {metrics.AverageCopyMs:0.0} ms";
        }
        SaveCaptureButton.IsEnabled = true;
    }

    private void ShowWaiting(string message)
    {
        PreviewPlaceholder.Text = message;
        WaitingOverlay.Visibility = Visibility.Visible;
    }

    private void UpdateButtons()
    {
        var running = _capture is not null;
        TargetWindows.IsEnabled = RefreshWindowsButton.IsEnabled = !running && !_transitioning && !_closing;
        StartCaptureButton.IsEnabled = !running && !_transitioning && !_closing && TargetWindows.SelectedItem is WindowTarget;
        AutoRecordButton.IsEnabled = StartCaptureButton.IsEnabled;
        HideCaptureBorder.IsEnabled = !_transitioning && !_closing;
        WindowCompatibility.IsEnabled = !running && !_transitioning && !_closing;
        NextRoundButton.IsEnabled = running && _autoTask is not null && !_transitioning && !_closing;
        StopCaptureButton.IsEnabled = running && !_transitioning && !_closing;
        SaveCaptureButton.IsEnabled = !running && !_transitioning && !_closing && _displayed is not null;
    }

    private void CaptureMode_Changed(object sender, RoutedEventArgs e)
    {
        if (!IsInitialized) return;
        var compatible = WindowCompatibility.IsChecked == true;
        HideCaptureBorder.Visibility = compatible ? Visibility.Visible : Visibility.Collapsed;
        BorderStatusText.Text = compatible ? "窗口兼容采集每 2 秒最多 1 帧；Win10 仍会显示黄框。"
            : "默认 DXGI 无黄框 · 每 2 秒最多 1 次常规截屏 · 切出游戏暂停";
    }

    private static string BorderMessage(GameWindowCapture capture, bool hide, BorderAccess access)
        => !hide ? "显示采集黄框" : capture.BorderHiddenRequested
            ? "已请求隐藏黄框；其他录屏程序或系统设置仍可能要求显示。"
            : access.Allowed ? "系统未接受隐藏请求，本次保留黄框。" : access.Message;

    private async void HideBorder_Changed(object sender, RoutedEventArgs e)
    {
        if (!IsInitialized || _transitioning || _closing) return;
        if (_capture is not GameWindowCapture capture)
        {
            BorderStatusText.Text = HideCaptureBorder.IsChecked == true ? "开始采集时申请隐藏黄框。" : "显示采集黄框";
            return;
        }
        _transitioning = true;
        UpdateButtons();
        try
        {
            var hide = HideCaptureBorder.IsChecked == true;
            var access = hide ? await CaptureBorder.RequestAsync() : new BorderAccess(false, "显示采集黄框");
            if (_closing || _capture != capture) return;
            capture.SetBorderHidden(hide && access.Allowed);
            BorderStatusText.Text = BorderMessage(capture, hide, access);
        }
        finally { _transitioning = false; UpdateButtons(); }
    }

    private void UpdateAutomaticStatus()
    {
        if (_autoRecorder is { } recorder)
        {
            var status = recorder.Status;
            AutoPhaseText.Text = "自动记录：" + status.Phase;
            AutoRosterText.Text = status.Roster;
            AutoSavedText.Text = status.LastSaved + $"  本次新增 {status.SavedCount} 场";
            AutoErrorText.Text = status.Error;
        }
        if (_autoTask?.IsFaulted == true)
        {
            AutoPhaseText.Text = "自动记录未能继续，请停止后重试";
            AutoErrorText.Text = _autoTask.Exception!.GetBaseException().Message;
        }
    }

    private void NextRound_Click(object sender, RoutedEventArgs e) => _autoRecorder?.RequestNextRound();

    private void AutoHistory_Click(object sender, RoutedEventArgs e)
    {
        if (_autoHistory is not null) { _autoHistory.Activate(); return; }
        _autoHistory = new HistoryWindow(_repository) { Owner = this };
        _autoHistory.Closed += (_, _) => _autoHistory = null;
        _autoHistory.Show();
    }

    private async void SaveCapture_Click(object sender, RoutedEventArgs e)
    {
        // Freeze this exact displayed frame before the dialog opens; later frames
        // cannot replace the evidence the user chose to save.
        var frame = _displayed;
        if (frame is null) return;
        var dialog = new SaveFileDialog
        {
            Title = "保存当前游戏截图", Filter = "PNG 图片 (*.png)|*.png", DefaultExt = ".png", AddExtension = true,
            FileName = $"Apex-{frame.CapturedAt.ToLocalTime():yyyyMMdd-HHmmss}.png"
        };
        if (dialog.ShowDialog(this) != true) return;
        try
        {
            await Task.Run(() => CaptureScreenshot.Save(frame, dialog.FileName));
            _savedMessage = $"截图已保存：{dialog.FileName}";
            CaptureDetailsText.Text = _savedMessage;
        }
        catch (Exception exception)
        {
            System.Diagnostics.Trace.TraceError(exception.ToString());
            CaptureErrorText.Text = "截图保存失败，请检查保存位置和磁盘空间。";
        }
    }

    protected override async void OnClosing(CancelEventArgs e)
    {
        if (_capture is not null)
        {
            e.Cancel = true;
            if (_closing) return;
            _closing = true;
            await StopCaptureAsync();
            // Dispose can complete synchronously. Do not re-enter Close while
            // WPF is still dispatching the original Closing event.
            _ = Dispatcher.BeginInvoke(new Action(() => { if (IsVisible) Close(); }), DispatcherPriority.Background);
            return;
        }
        _timer.Stop();
        _autoCancellation?.Cancel();
        base.OnClosing(e);
    }

    protected override void OnClosed(EventArgs e)
    {
        _closing = true;
        _timer.Stop();
        _autoCancellation?.Cancel();
        // Closing an owner can bypass its owned window's Closing event.
        if (_capture is { } capture)
        {
            _capture = null;
            _ = ReleaseClosedCaptureAsync(capture, _autoTask, _autoCancellation);
        }
        base.OnClosed(e);
    }

    private static async Task ReleaseClosedCaptureAsync(IWindowCapture capture, Task? autoTask, CancellationTokenSource? cancellation)
    {
        try { await Task.WhenAll(capture.DisposeAsync().AsTask(), autoTask ?? Task.CompletedTask).ConfigureAwait(false); }
        catch (Exception exception) { System.Diagnostics.Trace.TraceError(exception.ToString()); }
        finally { cancellation?.Dispose(); }
    }
}
