using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;
using BetterApex.Capture;
using BetterApex.Ocr;
using BetterApex.Storage;

internal static class AutomationLiveVerification
{
    public static void Run(string samples)
    {
        var app = Application.Current ?? new Application();
        app.ShutdownMode = ShutdownMode.OnExplicitShutdown;
        app.Resources["TextPrimary"] = Brushes.White;
        app.Resources["TextSecondary"] = Brushes.Gray;
        var source = new Image { Source = ScreenshotOcr.LoadOriginal(System.IO.Path.Combine(samples, "4-id.jpg")), Stretch = Stretch.Fill };
        RenderOptions.SetBitmapScalingMode(source, BitmapScalingMode.NearestNeighbor);
        var pulse = new Border { Width = 16, Height = 16, Background = Brushes.Red,
            HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Top };
        var grid = new Grid(); grid.Children.Add(source); grid.Children.Add(pulse);
        var fixture = new Window { Title = "Automatic recording test fixture", WindowStyle = WindowStyle.None,
            ResizeMode = ResizeMode.NoResize, Left = 0, Top = 0, Width = 800, Height = 500, Content = grid, ShowInTaskbar = false };
        fixture.Show();
        var dpi = VisualTreeHelper.GetDpi(fixture);
        fixture.Width = 2560 / dpi.DpiScaleX; fixture.Height = 1440 / dpi.DpiScaleY;
        var timer = new DispatcherTimer(TimeSpan.FromMilliseconds(150), DispatcherPriority.Normal,
            (_, _) => pulse.Background = pulse.Background == Brushes.Red ? Brushes.Lime : Brushes.Red, Dispatcher.CurrentDispatcher);
        var store = new MatchRepository(StorageVerification.NewDirectory("auto-live"));
        var window = new BetterApex.CaptureWindow(store);
        var picker = (ComboBox)window.FindName("TargetWindows");
        picker.ItemsSource = new[] { WindowCatalog.Describe(new WindowInteropHelper(fixture).Handle)! };
        picker.SelectedIndex = 0;
        window.Show();
        var task = RunAsync();
        var frame = new DispatcherFrame();
        var dispatcher = Dispatcher.CurrentDispatcher;
        _ = task.ContinueWith(_ => dispatcher.BeginInvoke(new Action(() => frame.Continue = false)), TaskScheduler.Default);
        try { Dispatcher.PushFrame(frame); task.GetAwaiter().GetResult(); }
        finally { timer.Stop(); window.Close(); fixture.Close(); }

        async Task RunAsync()
        {
            SynchronizationContext.SetSynchronizationContext(new DispatcherSynchronizationContext(Dispatcher.CurrentDispatcher));
            ((Button)window.FindName("AutoRecordButton")).RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
            fixture.Activate();
            await Until(() => ((TextBlock)window.FindName("AutoRosterText")).Text.Contains("Archer"), "automatic nickname capture");
            source.Source = ScreenshotOcr.LoadOriginal(System.IO.Path.Combine(samples, "2.jpg"));
            await Until(() => store.List().Count == 1, "captured GPU frames automatically reach SQLite");
            await Task.Delay(1500);
            Check(store.List().Count == 1, "Repeated live summary remains one record");
            var record = store.Get(store.List()[0].Id)!;
            Check(record.Players.Select(p => p.Damage).SequenceEqual(new int?[] { 1323, 1971, 371 }), "Live capture -> OCR -> saved damage");
            Check(record.RosterScreenshotFile is not null, "Both original live capture images are retained");

            ((Button)window.FindName("StopCaptureButton")).RaiseEvent(new RoutedEventArgs(Button.ClickEvent));
            await Until(() => ((Button)window.FindName("AutoRecordButton")).IsEnabled, "automatic worker stops cleanly");
            Console.WriteLine("AUTO LIVE PASS: low-rate DXGI -> automatic page detection -> nicknames -> settlement -> original evidence and SQLite, through actual UI buttons.");
        }

        async Task Until(Func<bool> ready, string step)
        {
            var elapsed = System.Diagnostics.Stopwatch.StartNew();
            while (!ready())
            {
                if (elapsed.Elapsed.TotalSeconds > 25)
                    throw new TimeoutException(step + ": " + ((TextBlock)window.FindName("AutoPhaseText")).Text
                        + " / " + ((TextBlock)window.FindName("AutoErrorText")).Text);
                await Task.Delay(100);
            }
            Console.WriteLine("PASS " + step);
        }
    }

    private static void Check(bool value, string description)
    { if (!value) throw new InvalidOperationException("FAILED: " + description); }
}
