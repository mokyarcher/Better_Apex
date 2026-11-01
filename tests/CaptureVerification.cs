using System.IO;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using BetterApex.Capture;

internal static class CaptureVerification
{
    public static void Run(string outputDirectory)
    {
        Directory.CreateDirectory(outputDirectory);
        var app = Application.Current ?? new Application();
        app.ShutdownMode = ShutdownMode.OnExplicitShutdown;
        app.Resources["TextPrimary"] = Brushes.White;
        app.Resources["TextSecondary"] = Brushes.Gray;
        var content = new Grid { Background = Brushes.CornflowerBlue };
        content.Children.Add(new Border { Background = Brushes.Red, Width = 64, Height = 64,
            HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Top });
        content.Children.Add(new Border { Background = Brushes.Lime, Width = 64, Height = 64,
            HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Bottom });
        var counter = new TextBlock { Text = "Capture test", FontSize = 28, HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center };
        content.Children.Add(counter);
        var fixture = new Window { Title = "Better Apex capture test fixture", Left = 40, Top = 40,
            Width = 640, Height = 400, Content = content, ShowInTaskbar = false };
        var ticks = 0;
        var animation = new DispatcherTimer(TimeSpan.FromMilliseconds(150), DispatcherPriority.Normal,
            (_, _) => counter.Text = $"Capture test {++ticks}", Dispatcher.CurrentDispatcher);
        fixture.Show();
        var target = WindowCatalog.Describe(new WindowInteropHelper(fixture).Handle)!;
        try
        {
            RunTask(async () =>
            {
                Check(GameWindowCapture.IsSupported, "Windows.Graphics.Capture supported on this test desktop");
                Check(WindowCatalog.List().All(w => w.ProcessId != Environment.ProcessId), "Own preview/history windows are excluded from picker");
                var borderAccess = await CaptureBorder.RequestAsync();
                Console.WriteLine($"BORDER ACCESS: supported={CaptureBorder.IsSupported}, allowed={borderAccess.Allowed}; {borderAccess.Message}");
                await using var capture = new GameWindowCapture(target, hideBorder: borderAccess.Allowed);
                if (borderAccess.Allowed)
                {
                    Check(capture.BorderHiddenRequested, "Border-required property disabled after system allows access");
                    Check(capture.SetBorderHidden(false) && !capture.BorderHiddenRequested, "Border can be shown again");
                    Check(capture.SetBorderHidden(true) && capture.BorderHiddenRequested, "Border can be hidden again");
                    Console.WriteLine("PASS native border hide/show/hide round trip");
                }
                await Until(() => capture.Latest is not null, "first captured frame", capture);
                var first = capture.Latest!;
                Check(!first.IsBlack && first.Image.IsFrozen, "Full-color immutable frame available off UI thread");
                Check(first.Image.PixelWidth == first.Geometry.Client.Width && first.Image.PixelHeight == first.Geometry.Client.Height,
                    "Screenshot dimensions equal physical client area");
                CheckCorner(first.Image, 10, 10, true);
                CheckCorner(first.Image, first.Image.PixelWidth - 10, first.Image.PixelHeight - 10, false);
                await Until(() => capture.Latest!.SourceTime > first.SourceTime, "fresh capture timestamps", capture);

                fixture.Width = 800;
                fixture.Height = 500;
                fixture.Left = 100;
                await Until(() => capture.Latest is { } latest && latest.Image.PixelWidth > first.Image.PixelWidth
                    && latest.Image.PixelHeight > first.Image.PixelHeight, "resize and frame pool recreation", capture);
                var resized = capture.Latest!;
                CheckCorner(resized.Image, 10, 10, true);
                CheckCorner(resized.Image, resized.Image.PixelWidth - 10, resized.Image.PixelHeight - 10, false);
                var pngPath = Path.Combine(outputDirectory, "captured-client.png");
                CaptureScreenshot.Save(resized, pngPath);
                using (var saved = File.OpenRead(pngPath))
                {
                    var decoded = BitmapFrame.Create(saved, BitmapCreateOptions.None, BitmapCacheOption.OnLoad);
                    Check(decoded.PixelWidth == resized.Image.PixelWidth && decoded.PixelHeight == resized.Image.PixelHeight,
                        "PNG export preserves original pixels instead of preview dimensions");
                    CheckCorner(decoded, 10, 10, true);
                }
                Check(first.Image.PixelWidth != resized.Image.PixelWidth, "Previously captured frame remains unchanged");

                var dpi = VisualTreeHelper.GetDpi(fixture);
                fixture.ResizeMode = ResizeMode.NoResize;
                fixture.WindowStyle = WindowStyle.None;
                fixture.Left = fixture.Top = 0;
                fixture.Width = 2560 / dpi.DpiScaleX;
                fixture.Height = 1440 / dpi.DpiScaleY;
                await Until(() => capture.Latest is { } twoK && twoK.Image.PixelWidth == 2560 && twoK.Image.PixelHeight == 1440,
                    "native 2560x1440 client capture", capture);
                var fullResolution = capture.Latest!;
                CheckCorner(fullResolution.Image, 10, 10, true);
                CheckCorner(fullResolution.Image, 2550, 1430, false);
                CaptureScreenshot.Save(fullResolution, Path.Combine(outputDirectory, "captured-2k.png"));
                fixture.WindowStyle = WindowStyle.SingleBorderWindow;
                fixture.ResizeMode = ResizeMode.CanResize;
                fixture.Width = 800;
                fixture.Height = 500;
                fixture.Left = fixture.Top = 40;
                await Until(() => capture.Latest is { } normal && normal.Image.PixelWidth < 2560,
                    "return from 2K to windowed capture", capture);

                fixture.WindowState = WindowState.Minimized;
                await Until(() => capture.IsMinimized, "minimized window detection", capture);
                var sequence = capture.Latest!.Sequence;
                await Task.Delay(800);
                Check(capture.Latest!.Sequence == sequence, "Minimized window does not publish stale frames as fresh");
                fixture.WindowState = WindowState.Normal;
                await Until(() => capture.Latest is { } latest && latest.Sequence > sequence, "capture resumes on restore", capture);
                await capture.DisposeAsync();
                Check(capture.Latest is null && capture.IsStopping, "Stop releases capture and clears latest frame");
                // Exercise actual preview controls against our fixture only.
                var preview = new BetterApex.CaptureWindow { Owner = fixture };
                ((CheckBox)preview.FindName("WindowCompatibility")).IsChecked = true;
                var targets = (ComboBox)preview.FindName("TargetWindows");
                targets.ItemsSource = new[] { target };
                targets.SelectedItem = target;
                preview.Show();
                var start = (Button)preview.FindName("StartCaptureButton");
                var stop = (Button)preview.FindName("StopCaptureButton");
                var image = (Image)preview.FindName("CapturePreview");
                Check(start.IsEnabled && !stop.IsEnabled, "Preview controls start idle");
                start.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                await Until(() => image.Source is not null, "preview image refresh", capture);
                Check(!targets.IsEnabled && stop.IsEnabled && !start.IsEnabled, "Target selection locked during capture");
                stop.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                await Until(() => start.IsEnabled && !stop.IsEnabled, "preview stop controls", capture);
                Check(((Button)preview.FindName("SaveCaptureButton")).IsEnabled, "Stopped preview allows saving the displayed last frame");
                start.RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                await Until(() => image.Source is not null, "preview restart", capture);
                preview.Close();
                await Until(() => !preview.IsVisible, "preview close releases active session", capture);
                await using var restarted = new GameWindowCapture(target);
                await Until(() => restarted.Latest is not null, "capture restart", restarted);
                var ownedPreview = new BetterApex.CaptureWindow { Owner = fixture };
                ((CheckBox)ownedPreview.FindName("WindowCompatibility")).IsChecked = true;
                var ownedTargets = (ComboBox)ownedPreview.FindName("TargetWindows");
                ownedTargets.ItemsSource = new[] { target };
                ownedTargets.SelectedItem = target;
                ownedPreview.Show();
                ((Button)ownedPreview.FindName("StartCaptureButton")).RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
                await Until(() => ((Image)ownedPreview.FindName("CapturePreview")).Source is not null,
                    "owned preview active before owner closes", restarted);
                fixture.Close();
                await Until(() => restarted.TargetClosed, "target close detected", restarted);
                await Until(() => !ownedPreview.IsVisible, "owner close cleans up preview window", restarted);
            });
            Console.WriteLine("CAPTURE PASS: actual WGC frames, client corners and size, fresh timestamps, move/resize, minimize/restore, stop/restart, target closed.");
        }
        finally { animation.Stop(); fixture.Close(); }
    }

    private static async Task Until(Func<bool> condition, string step, GameWindowCapture capture)
    {
        var timeout = System.Diagnostics.Stopwatch.StartNew();
        while (!condition())
        {
            if (capture.Error is { } error) throw new InvalidOperationException(step + ": " + error);
            if (timeout.Elapsed > TimeSpan.FromSeconds(12))
                throw new TimeoutException($"Capture test: {step}; pool={capture.FrameSize.Width}x{capture.FrameSize.Height}; "
                    + $"geometry={NativeWindow.Geometry(capture.Target.Handle)}; latest={capture.Latest?.Image.PixelWidth}x{capture.Latest?.Image.PixelHeight}");
            await Task.Delay(100);
        }
        Console.WriteLine("PASS " + step);
    }

    private static void CheckCorner(BitmapSource image, int x, int y, bool red)
    {
        var pixel = new byte[4];
        image.CopyPixels(new Int32Rect(x, y, 1, 1), pixel, 4, 0);
        Check(red ? pixel[2] > 220 && pixel[1] < 30 : pixel[1] > 220 && pixel[2] < 30,
            red ? "Top-left client marker; title bar excluded" : "Bottom-right client marker; edges preserved");
    }

    private static void Check(bool success, string message)
    { if (!success) throw new InvalidOperationException("FAILED: " + message); }

    private static void RunTask(Func<Task> work)
    {
        var dispatcher = Dispatcher.CurrentDispatcher;
        SynchronizationContext.SetSynchronizationContext(new DispatcherSynchronizationContext(dispatcher));
        var task = work();
        if (!task.IsCompleted)
        {
            var frame = new DispatcherFrame();
            _ = task.ContinueWith(_ => dispatcher.BeginInvoke(new Action(() => frame.Continue = false)), TaskScheduler.Default);
            Dispatcher.PushFrame(frame);
        }
        task.GetAwaiter().GetResult();
    }
}
