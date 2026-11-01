using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Vortice.Direct3D;
using Vortice.Direct3D11;
using Vortice.DXGI;
using static Vortice.Direct3D11.D3D11;
using Box = Vortice.Mathematics.Box;
using MapFlags = Vortice.Direct3D11.MapFlags;

namespace BetterApex.Capture;

internal sealed record DxgiCaptureMetrics(long Attempts, long Frames, double AverageCopyMs, double MaxCopyMs);

internal sealed class DxgiWindowCapture : IWindowCapture
{
    private readonly CapturePacing _pacing = new();
    private readonly AutoResetEvent _wake = new(false);
    private readonly TaskCompletionSource _finished = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private volatile CapturedFrame? _latest;
    private volatile string? _pause = "等待游戏切到前台…";
    private int _stopping;
    private int _wakeDisposed;
    private long _attempts, _frames;
    private double _totalCopyMs, _maxCopyMs;
    public WindowTarget Target { get; }
    public CapturedFrame? Latest => _latest;
    public string? Error => null; // Recoverable display errors are pause reasons.
    public string? PauseReason => _pause;
    public bool TargetClosed => !NativeWindow.IsSameWindow(Target);
    public bool IsMinimized => NativeWindow.IsIconic(Target.Handle);
    public bool IsStopping => Volatile.Read(ref _stopping) != 0;
    public bool CanAnalyze => !IsStopping && _pause is null && !TargetClosed && !IsMinimized;
    public DxgiCaptureMetrics Metrics => new(Interlocked.Read(ref _attempts), Interlocked.Read(ref _frames),
        _frames == 0 ? 0 : Volatile.Read(ref _totalCopyMs) / Interlocked.Read(ref _frames), Volatile.Read(ref _maxCopyMs));

    public DxgiWindowCapture(WindowTarget target)
    {
        if (!NativeWindow.IsSameWindow(target)) throw new InvalidOperationException("目标窗口已关闭，请刷新列表。");
        Target = target;
        new Thread(Run) { IsBackground = true, Name = "Low-frequency DXGI capture", Priority = ThreadPriority.BelowNormal }.Start();
    }

    public void RequestVerificationFrame()
    {
        if (IsStopping) return;
        _pacing.RequestVerification(Stopwatch.GetTimestamp());
        try { _wake.Set(); } catch (ObjectDisposedException) { }
    }

    private void Run()
    {
        DesktopReader? reader = null;
        try
        {
            while (!IsStopping)
            {
                var reason = NativeWindow.CapturePauseReason(Target, out var geometry);
                if (reason is not null)
                {
                    _pause = reason; _latest = null;
                    // Release the duplication while tabbed out: no background
                    // desktop updates, GPU readbacks, or desktop images queued.
                    reader?.Dispose(); reader = null;
                    _wake.WaitOne(100);
                    continue;
                }
                if (!_pacing.TryBegin(Stopwatch.GetTimestamp())) { _wake.WaitOne(50); continue; }
                Interlocked.Increment(ref _attempts);
                try
                {
                    var monitor = NativeWindow.MonitorFromWindow(Target.Handle, 2);
                    if (reader is null || reader.Monitor != monitor)
                    { reader?.Dispose(); reader = new DesktopReader(monitor); }
                    var timer = Stopwatch.StartNew();
                    var image = reader.Take(geometry.Client, () => IsStopping
                        || NativeWindow.GetAncestor(NativeWindow.GetForegroundWindow(), 2) != Target.Handle);
                    if (image is not null && !IsStopping && NativeWindow.CapturePauseReason(Target, out var final) is null && final == geometry)
                    {
                        var duration = timer.Elapsed.TotalMilliseconds;
                        Volatile.Write(ref _totalCopyMs, _totalCopyMs + duration);
                        Volatile.Write(ref _maxCopyMs, Math.Max(_maxCopyMs, duration));
                        var sequence = Interlocked.Increment(ref _frames);
                        _latest = new(image, DateTimeOffset.UtcNow, sequence, Stopwatch.GetElapsedTime(0), geometry, false);
                    }
                    _pause = null;
                }
                catch (Exception exception)
                {
                    Trace.TraceError(exception.ToString());
                    _pause = exception is NotSupportedException ? exception.Message
                        : $"显示模式变化或 DXGI 暂不可用，正在重连（0x{exception.HResult:X8}）";
                    _latest = null;
                    reader?.Dispose(); reader = null;
                    _wake.WaitOne(2000); // Bound driver/device reinitialization.
                }
                finally { _pacing.CompleteAttempt(Stopwatch.GetTimestamp()); }
            }
        }
        catch (Exception exception) { _pause = "DXGI 采集已停止：" + exception.Message; Trace.TraceError(exception.ToString()); }
        finally
        {
            reader?.Dispose(); _latest = null;
            _finished.TrySetResult();
        }
    }

    public async ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _stopping, 1) == 0) _wake.Set();
        await _finished.Task.ConfigureAwait(false);
        if (Interlocked.Exchange(ref _wakeDisposed, 1) == 0) _wake.Dispose();
    }

    private sealed class DesktopReader : IDisposable
    {
        private ID3D11Device? _device;
        private ID3D11DeviceContext? _context;
        private IDXGIOutputDuplication? _duplication;
        private ID3D11Texture2D? _staging;
        private Int32Rect _screen;
        private bool _ownsFrame;
        public nint Monitor { get; }

        public DesktopReader(nint monitor)
        {
            Monitor = monitor;
            try
            {
                using var factory = DXGI.CreateDXGIFactory1<IDXGIFactory1>();
                for (uint a = 0; factory.EnumAdapters1(a, out var adapter).Success; a++)
                {
                    using (adapter)
                    for (uint o = 0; adapter.EnumOutputs(o, out var output).Success; o++)
                    {
                        using (output)
                        {
                            var description = output.Description;
                            if (description.Monitor != monitor || !description.AttachedToDesktop) continue;
                            if (description.Rotation is not (ModeRotation.Identity or ModeRotation.Unspecified))
                                throw new NotSupportedException("当前 DXGI 模式仅支持横向显示器，请将游戏放在横向屏幕。");
                            var bounds = description.DesktopCoordinates;
                            _screen = new(bounds.Left, bounds.Top, bounds.Right - bounds.Left, bounds.Bottom - bounds.Top);
                            D3D11CreateDevice(adapter, DriverType.Unknown, DeviceCreationFlags.BgraSupport,
                                [FeatureLevel.Level_11_0, FeatureLevel.Level_10_1, FeatureLevel.Level_10_0], out _device, out _context).CheckError();
                            using var output1 = output.QueryInterface<IDXGIOutput1>();
                            _duplication = output1.DuplicateOutput(_device);
                            return;
                        }
                    }
                }
                throw new NotSupportedException("未找到游戏所在显示器的 DXGI 输出，请检查显示器连接。");
            }
            catch { Dispose(); throw; }
        }

        public BitmapSource? Take(Int32Rect client, Func<bool> cancelled)
        {
            if (client.X < _screen.X || client.Y < _screen.Y || client.X + client.Width > _screen.X + _screen.Width
                || client.Y + client.Height > _screen.Y + _screen.Height)
                throw new NotSupportedException("请把完整游戏窗口放在同一块显示器内，当前暂停截图。");
            if ((long)client.Width * client.Height > 50_000_000) throw new NotSupportedException("游戏窗口尺寸过大。");
            // Microsoft's recommended ownership pattern: release immediately
            // before acquiring again, avoiding redundant desktop GPU updates.
            if (_ownsFrame) { _duplication!.ReleaseFrame().CheckError(); _ownsFrame = false; }
            var acquired = _duplication!.AcquireNextFrame(50, out _, out var resource);
            if (acquired.Code == unchecked((int)0x887A0027)) return null; // WAIT_TIMEOUT
            acquired.CheckError();
            _ownsFrame = true;
            using (resource)
            using (var texture = resource.QueryInterface<ID3D11Texture2D>())
            {
                if (_staging is null || _staging.Description.Width != client.Width || _staging.Description.Height != client.Height)
                {
                    _staging?.Dispose();
                    _staging = _device!.CreateTexture2D(new Texture2DDescription(Format.B8G8R8A8_UNorm,
                        (uint)client.Width, (uint)client.Height, mipLevels: 1, bindFlags: BindFlags.None,
                        usage: ResourceUsage.Staging, cpuAccessFlags: CpuAccessFlags.Read));
                }
                var left = client.X - _screen.X; var top = client.Y - _screen.Y;
                var region = new Box(left, top, 0, left + client.Width, top + client.Height, 1);
                var context = _context!;
                context.CopySubresourceRegion(_staging, 0, 0, 0, 0, texture, 0, region);
                context.Flush();
                var wait = Stopwatch.StartNew();
                while (!cancelled())
                {
                    var result = context.Map(_staging, 0, MapMode.Read, MapFlags.DoNotWait, out var mapped);
                    if (result.Code == unchecked((int)0x887A000A)) // WAS_STILL_DRAWING
                    {
                        if (wait.ElapsedMilliseconds > 500) throw new TimeoutException("GPU 回读超时，已降低重试频率。");
                        Thread.Sleep(5); continue;
                    }
                    result.CheckError();
                    try
                    {
                        if (cancelled()) return null;
                        var stride = checked((int)mapped.RowPitch);
                        var visible = 0;
                        for (var row = 0; row < 18 && visible < 8; row++)
                            for (var column = 0; column < 32 && visible < 8; column++)
                            {
                                var pixel = unchecked((uint)Marshal.ReadInt32(mapped.DataPointer,
                                    row * client.Height / 18 * stride + column * client.Width / 32 * 4));
                                if (Math.Max(pixel & 255, Math.Max((pixel >> 8) & 255, (pixel >> 16) & 255)) > 12) visible++;
                            }
                        if (visible < 8) return null; // Avoid allocating/OCRing a black loading frame.
                        var image = BitmapSource.Create(client.Width, client.Height, 96, 96, PixelFormats.Bgra32,
                            null, mapped.DataPointer, checked(stride * client.Height), stride);
                        image.Freeze();
                        return image;
                    }
                    finally { context.Unmap(_staging, 0); }
                }
                return null;
            }
        }

        public void Dispose()
        {
            if (_ownsFrame) { _duplication?.ReleaseFrame(); _ownsFrame = false; }
            _staging?.Dispose(); _staging = null;
            _duplication?.Dispose(); _duplication = null;
            _context?.Dispose(); _context = null;
            _device?.Dispose(); _device = null;
        }
    }
}
