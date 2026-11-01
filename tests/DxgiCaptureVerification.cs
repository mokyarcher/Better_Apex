using System.Diagnostics;
using System.IO;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;
using BetterApex.Capture;

internal static class DxgiCaptureVerification
{
    public static void Run(string output)
    {
        Directory.CreateDirectory(output);
        var app = Application.Current ?? new Application();
        app.ShutdownMode = ShutdownMode.OnExplicitShutdown;
        var grid = new Grid { Background = Brushes.CornflowerBlue };
        grid.Children.Add(new Border { Background = Brushes.Red, Width = 64, Height = 64,
            HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Top });
        grid.Children.Add(new Border { Background = Brushes.Lime, Width = 64, Height = 64,
            HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Bottom });
        var counter = new TextBlock { FontSize = 24, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center };
        grid.Children.Add(counter);
        var fixture = new Window { Title = "DXGI low frequency test", Left = 40, Top = 40, Width = 800, Height = 500,
            Content = grid, ShowInTaskbar = false };
        fixture.Show(); fixture.Activate();
        var ticks = 0;
        var animation = new DispatcherTimer(TimeSpan.FromMilliseconds(100), DispatcherPriority.Normal,
            (_, _) => counter.Text = "Desktop frame " + ++ticks, Dispatcher.CurrentDispatcher);
        var dispatcher = Dispatcher.CurrentDispatcher;
        SynchronizationContext.SetSynchronizationContext(new DispatcherSynchronizationContext(dispatcher));
        var task = Verify();
        var pump = new DispatcherFrame();
        _ = task.ContinueWith(_ => dispatcher.BeginInvoke(new Action(() => pump.Continue = false)), TaskScheduler.Default);
        try { Dispatcher.PushFrame(pump); task.GetAwaiter().GetResult(); }
        finally { animation.Stop(); fixture.Close(); }

        async Task Verify()
        {
            var target = WindowCatalog.Describe(new WindowInteropHelper(fixture).Handle)!;
            await using var source = new DxgiWindowCapture(target);
            await Until(() => source.Latest is { } ready && HasCorners(ready), "First fully rendered foreground DXGI frame", source);
            var first = source.Latest!;
            CaptureScreenshot.Save(first, Path.Combine(output, "dxgi-first.png"));
            Console.WriteLine($"DXGI first geometry: {first.Geometry}");
            Corner(first, 10, 10, true);
            Corner(first, first.Image.PixelWidth - 10, first.Image.PixelHeight - 10, false);
            var startCount = source.Metrics.Attempts;
            var sampling = Stopwatch.StartNew();
            var seen = new List<TimeSpan> { first.SourceTime };
            while (sampling.Elapsed.TotalSeconds < 5.2)
            {
                if (source.Latest is { } frame && frame.SourceTime != seen[^1]) seen.Add(frame.SourceTime);
                await Task.Delay(25);
            }
            Check(source.Metrics.Attempts - startCount <= 3, "Normal capture attempts bounded to one per two seconds without catch-up");
            Check(seen.Count >= 3 && seen.Zip(seen.Skip(1)).All(p => p.Second - p.First >= TimeSpan.FromMilliseconds(1980)),
                "Actual published frames obey two-second normal pacing");
            Console.WriteLine($"PASS Two-second pacing: {seen.Count} observed frames in {sampling.Elapsed.TotalSeconds:F1}s");
            var beforeVerification = source.Latest!.Sequence;
            source.RequestVerificationFrame();
            source.RequestVerificationFrame(); // Repeated requests are coalesced.
            await Until(() => source.Latest!.Sequence > beforeVerification, "Single verification frame", source);

            fixture.Left += 70; fixture.Width += 80;
            await Until(() => source.Latest is { } moved && moved.Image.PixelWidth != first.Image.PixelWidth && HasCorners(moved),
                "Window move/resize updates client crop", source);
            var resized = source.Latest!;
            Corner(resized, 10, 10, true);
            Corner(resized, resized.Image.PixelWidth - 10, resized.Image.PixelHeight - 10, false);
            Corner(first, 10, 10, true);
            CaptureScreenshot.Save(resized, Path.Combine(output, "dxgi-client.png"));

            var cover = new Window { Title = "DXGI pause test", Left = 60, Top = 60, Width = 350, Height = 220,
                Content = new TextBlock { Text = "Non-game window" }, ShowInTaskbar = false };
            cover.Show(); cover.Activate();
            await Until(() => source.PauseReason is not null && source.Latest is null, "Alt-tab pauses capture and clears latest frame", source);
            var pausedCount = source.Metrics.Attempts;
            await Task.Delay(1600);
            Check(source.Metrics.Attempts == pausedCount, "No DXGI acquisition/readback attempts while game is not foreground");
            cover.Close(); fixture.Activate();
            await Until(() => source.Latest is not null && source.PauseReason is null, "Capture resumes after foreground restore", source);

            var dpi = VisualTreeHelper.GetDpi(fixture);
            fixture.ResizeMode = ResizeMode.NoResize; fixture.WindowStyle = WindowStyle.None;
            fixture.Left = fixture.Top = 0;
            fixture.Width = 2560 / dpi.DpiScaleX; fixture.Height = 1440 / dpi.DpiScaleY;
            await Until(() => source.Latest is { } full && full.Image.PixelWidth == 2560 && full.Image.PixelHeight == 1440,
                "Native 2K monitor region", source);
            CaptureScreenshot.Save(source.Latest!, Path.Combine(output, "dxgi-2k.png"));
            var metrics = source.Metrics;
            Console.WriteLine($"DXGI METRICS: attempts={metrics.Attempts}; frames={metrics.Frames}; average copy={metrics.AverageCopyMs:F2}ms; max copy={metrics.MaxCopyMs:F2}ms");
            await source.DisposeAsync();
            var stopped = source.Metrics.Attempts;
            await Task.Delay(1200);
            Check(source.Latest is null && source.Metrics.Attempts == stopped, "Stop releases capture and never schedules more attempts");
            Console.WriteLine("DXGI PASS: actual frame pacing, no capture on alt-tab, client corners, move/resize, 2K PNG and stop.");
        }
    }

    private static async Task Until(Func<bool> ready, string label, DxgiWindowCapture source)
    {
        var timeout = Stopwatch.StartNew();
        while (!ready())
        {
            if (timeout.Elapsed.TotalSeconds > 15) throw new TimeoutException(label + ": " + source.PauseReason);
            await Task.Delay(50);
        }
        Console.WriteLine("PASS " + label);
    }

    private static void Corner(CapturedFrame frame, int x, int y, bool red)
    {
        var pixel = new byte[4];
        frame.Image.CopyPixels(new Int32Rect(x, y, 1, 1), pixel, 4, 0);
        Check(red ? pixel[2] > 220 && pixel[1] < 30 : pixel[1] > 220 && pixel[2] < 30,
            $"Client corner at {x},{y}, pixel BGR={pixel[0]},{pixel[1]},{pixel[2]}");
    }

    private static bool HasCorners(CapturedFrame frame)
    {
        var red = new byte[4]; var green = new byte[4];
        frame.Image.CopyPixels(new Int32Rect(10, 10, 1, 1), red, 4, 0);
        frame.Image.CopyPixels(new Int32Rect(frame.Image.PixelWidth - 20, frame.Image.PixelHeight - 20, 1, 1), green, 4, 0);
        return red[2] > 220 && red[1] < 30 && green[1] > 220 && green[2] < 30;
    }

    private static void Check(bool valid, string message)
    { if (!valid) throw new InvalidOperationException("FAILED: " + message); }
}
