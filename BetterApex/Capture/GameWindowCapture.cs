using System.Diagnostics;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Windows.Graphics;
using Windows.Graphics.Capture;
using Windows.Graphics.DirectX;
using Windows.Graphics.DirectX.Direct3D11;
using Windows.Graphics.Imaging;

namespace BetterApex.Capture;

internal sealed record CapturedFrame(BitmapSource Image, DateTimeOffset CapturedAt, long Sequence,
    TimeSpan SourceTime, WindowGeometry Geometry, bool IsBlack)
{
    public long ReceivedTimestamp { get; } = Stopwatch.GetTimestamp();
}

internal sealed class GameWindowCapture : IWindowCapture
{
    private readonly SemaphoreSlim _gate = new(1, 1);
    private readonly IDirect3DDevice _device;
    private readonly GraphicsCaptureItem _item;
    private readonly Direct3D11CaptureFramePool _pool;
    private readonly GraphicsCaptureSession _session;
    private SizeInt32 _size;
    private int _stopping;
    private bool _disposed;
    private long _lastCopy, _sequence;
    private long _lastSourceTicks = -1;
    private readonly double _intervalMilliseconds;
    private volatile CapturedFrame? _latest;
    private volatile string? _error;
    private volatile bool _closed;

    public WindowTarget Target { get; }
    public CapturedFrame? Latest => _latest;
    public string? Error => _error;
    public bool TargetClosed => _closed || !NativeWindow.IsSameWindow(Target);
    public bool IsMinimized => NativeWindow.IsIconic(Target.Handle);
    public bool IsStopping => Volatile.Read(ref _stopping) != 0;
    internal SizeInt32 FrameSize => _size;
    public bool BorderHiddenRequested { get; private set; }
    public string? PauseReason => null;
    public bool CanAnalyze => !IsStopping && !IsMinimized && !TargetClosed && Error is null;
    public void RequestVerificationFrame() { }

    public static bool IsSupported => GraphicsCaptureSession.IsSupported();

    public GameWindowCapture(WindowTarget target, TimeSpan? sampleInterval = null, bool hideBorder = false)
    {
        if (!IsSupported) throw new NotSupportedException("当前 Windows 或显卡不支持窗口采集。");
        if (!NativeWindow.IsSameWindow(target)) throw new InvalidOperationException("目标窗口已关闭，请刷新窗口列表。");
        Target = target;
        _intervalMilliseconds = (sampleInterval ?? TimeSpan.FromMilliseconds(500)).TotalMilliseconds;
        _device = CaptureInterop.CreateDevice();
        try
        {
            _item = CaptureInterop.CreateItem(target.Handle);
            _size = _item.Size;
            if (_size.Width <= 0 || _size.Height <= 0) throw new InvalidOperationException("目标窗口尺寸无效，请恢复窗口后再试。");
            _pool = Direct3D11CaptureFramePool.CreateFreeThreaded(_device, DirectXPixelFormat.B8G8R8A8UIntNormalized, 2, _size);
            try
            {
                _session = _pool.CreateCaptureSession(_item);
                _session.IsCursorCaptureEnabled = false;
                if (hideBorder) BorderHiddenRequested = CaptureBorder.TrySet(_session, required: false);
                _item.Closed += OnClosed;
                _pool.FrameArrived += OnFrameArrived;
                _session.StartCapture();
            }
            catch
            {
                _item.Closed -= OnClosed;
                _pool.FrameArrived -= OnFrameArrived;
                _session?.Dispose();
                _pool.Dispose();
                throw;
            }
        }
        catch { _device.Dispose(); throw; }
    }

    private void OnClosed(GraphicsCaptureItem sender, object args) => _closed = true;

    public bool SetBorderHidden(bool hidden)
    {
        if (IsStopping) return false;
        var applied = CaptureBorder.TrySet(_session, required: !hidden);
        if (applied) BorderHiddenRequested = hidden;
        return applied;
    }

    private async void OnFrameArrived(Direct3D11CaptureFramePool sender, object args)
    {
        if (IsStopping || !_gate.Wait(0)) return;
        try
        {
            if (IsStopping || TargetClosed) return;
            SizeInt32? resize = null;
            using (var frame = sender.TryGetNextFrame())
            {
                if (frame is null) return;
                var size = frame.ContentSize;
                if (size.Width <= 0 || size.Height <= 0) return;
                if (size.Width != _size.Width || size.Height != _size.Height)
                    resize = size;
                else if (!IsMinimized && Stopwatch.GetElapsedTime(_lastCopy).TotalMilliseconds >= _intervalMilliseconds
                    && frame.SystemRelativeTime.Ticks > _lastSourceTicks)
                {
                    _lastCopy = Stopwatch.GetTimestamp();
                    var geometry = NativeWindow.Geometry(Target.Handle);
                    var crop = geometry?.Crop(size.Width, size.Height);
                    if (crop is null) return;
                    using var bitmap = await SoftwareBitmap.CreateCopyFromSurfaceAsync(frame.Surface, BitmapAlphaMode.Ignore);
                    var stride = checked(bitmap.PixelWidth * 4);
                    var pixels = new byte[checked(stride * bitmap.PixelHeight)];
                    bitmap.CopyToBuffer(pixels.AsBuffer());
                    // Only publish a frame with a matching geometry before/after
                    // the GPU copy. A moving/resizing window gets a new attempt.
                    if (IsStopping || IsMinimized || TargetClosed || geometry != NativeWindow.Geometry(Target.Handle)) return;
                    var image = BitmapSource.Create(bitmap.PixelWidth, bitmap.PixelHeight, 96, 96,
                        PixelFormats.Bgra32, null, pixels, stride);
                    var client = new CroppedBitmap(image, crop.Value);
                    client.Freeze();
                    _lastSourceTicks = frame.SystemRelativeTime.Ticks;
                    _latest = new(client, DateTimeOffset.UtcNow, ++_sequence, frame.SystemRelativeTime, geometry!.Value,
                        IsBlack(pixels, stride, crop.Value));
                }
            }
            // Return the old surface before recreating the frame pool.
            if (resize is { } next && !IsStopping)
            {
                _latest = null;
                _size = next;
                _lastSourceTicks = -1;
                sender.Recreate(_device, DirectXPixelFormat.B8G8R8A8UIntNormalized, 2, next);
            }
        }
        catch (Exception e)
        {
            if (!IsStopping)
            {
                Trace.TraceError(e.ToString());
                _error = "采集已中断，请停止后重新选择窗口。";
            }
        }
        finally { _gate.Release(); }
    }

    private static bool IsBlack(byte[] pixels, int stride, System.Windows.Int32Rect crop)
    {
        // Sparse check of the client only: borders do not make a black game
        // surface appear usable. A visible HUD normally supplies bright pixels.
        var bright = 0;
        for (var row = 0; row < 36; row++)
            for (var column = 0; column < 64; column++)
            {
                var offset = (crop.Y + row * crop.Height / 36) * stride + (crop.X + column * crop.Width / 64) * 4;
                if (Math.Max(pixels[offset], Math.Max(pixels[offset + 1], pixels[offset + 2])) > 12 && ++bright >= 12)
                    return false;
            }
        return true;
    }

    public async ValueTask DisposeAsync()
    {
        Interlocked.Exchange(ref _stopping, 1);
        await _gate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (_disposed) return;
            _disposed = true;
            _pool.FrameArrived -= OnFrameArrived;
            _item.Closed -= OnClosed;
            try { _session.Dispose(); }
            finally
            {
                try { _pool.Dispose(); }
                finally { _device.Dispose(); _latest = null; }
            }
        }
        finally { _gate.Release(); }
    }
}
