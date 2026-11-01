using System.IO;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using BetterApex.Storage;

internal static class RenderPreview
{
    public static void Capture(string imagePath, string outputDirectory)
    {
        Directory.CreateDirectory(outputDirectory);
        var app = new BetterApex.App { ShutdownMode = ShutdownMode.OnExplicitShutdown };
        app.InitializeComponent();
        var window = new BetterApex.CaptureWindow();
        var targets = (ComboBox)window.FindName("TargetWindows");
        targets.ItemsSource = new[] { new BetterApex.Capture.WindowTarget(0, 0, "窗口采集测试画面", "test-fixture") };
        targets.SelectedIndex = 0;
        ((TextBlock)window.FindName("CaptureStatusText")).Text = "请选择游戏窗口，再点击“开始自动记录”。";
        Render(window, Path.Combine(outputDirectory, "capture-empty.png"), 1068, 730);
        ((Image)window.FindName("CapturePreview")).Source = BetterApex.Ocr.ScreenshotOcr.LoadOriginal(imagePath);
        ((FrameworkElement)window.FindName("WaitingOverlay")).Visibility = Visibility.Collapsed;
        ((TextBlock)window.FindName("CaptureStatusText")).Text = "正在采集：窗口采集测试画面";
        ((TextBlock)window.FindName("CaptureDetailsText")).Text = "2560 × 1440 像素";
        ((TextBlock)window.FindName("AutoPhaseText")).Text = "自动记录：对局进行中，等待结算";
        ((TextBlock)window.FindName("AutoRosterText")).Text = "本场名单：队友一 / 队友二 / 本人";
        ((Button)window.FindName("SaveCaptureButton")).IsEnabled = true;
        ((Button)window.FindName("StopCaptureButton")).IsEnabled = true;
        ((Button)window.FindName("StartCaptureButton")).IsEnabled = false;
        ((Button)window.FindName("AutoRecordButton")).IsEnabled = false;
        targets.IsEnabled = false;
        ((Button)window.FindName("RefreshWindowsButton")).IsEnabled = false;
        Render(window, Path.Combine(outputDirectory, "capture-active.png"), 1068, 730);
        window.Close();
    }

    // Render the real XAML and application styles without showing a desktop window.
    // This is a visual review utility; functional workflow checks live in Program.
    public static void Run(string dataDirectory, string outputDirectory)
    {
        var app = new BetterApex.App { ShutdownMode = ShutdownMode.OnExplicitShutdown };
        app.InitializeComponent();
        var store = new MatchRepository(dataDirectory);
        var summaries = store.List();
        var record = store.Get(summaries.First().Id)!;
        Directory.CreateDirectory(outputDirectory);

        var history = new BetterApex.HistoryWindow(store);
        ((TextBlock)history.FindName("HistoryCountText")).Text = $"共 {summaries.Count} 场 · 按保存时间排列";
        ((ListBox)history.FindName("HistoryList")).ItemsSource = summaries;
        ((FrameworkElement)history.FindName("DetailsPanel")).Visibility = Visibility.Visible;
        ((FrameworkElement)history.FindName("EmptyHistoryText")).Visibility = Visibility.Collapsed;
        ((TextBlock)history.FindName("MatchTitleText")).Text = record.Title;
        ((TextBlock)history.FindName("SavedTimeText")).Text = record.TimeLabel;
        ((TextBlock)history.FindName("RecordStatusText")).Text = record.Status;
        ((DataGrid)history.FindName("SavedPlayers")).ItemsSource = record.Players;
        ((Image)history.FindName("SavedScreenshotPreview")).Source = BetterApex.ScreenshotLoader.Load(store.GetImagePath(record.ScreenshotFile)).Preview;
        ((Button)history.FindName("SummaryImageButton")).IsEnabled = true;
        ((Button)history.FindName("RosterImageButton")).IsEnabled = record.RosterScreenshotFile is not null;
        Render(history, Path.Combine(outputDirectory, "history.png"), 1180, 750);

        var main = new BetterApex.MainWindow(store);
        ((FrameworkElement)main.FindName("EmptyState")).Visibility = Visibility.Collapsed;
        ((Image)main.FindName("ScreenshotPreview")).Source = BetterApex.ScreenshotLoader.Load(store.GetImagePath(record.ScreenshotFile)).Preview;
        ((Image)main.FindName("ScreenshotPreview")).Visibility = Visibility.Visible;
        ((ItemsControl)main.FindName("PlayerResults")).ItemsSource = record.Players;
        ((TextBlock)main.FindName("ResultSummaryText")).Text = record.Title + "\n已关联 3/3 名玩家";
        ((FrameworkElement)main.FindName("SaveMatchButton")).Visibility = Visibility.Visible;
        ((Button)main.FindName("SaveMatchButton")).IsEnabled = true;
        ((TextBlock)main.FindName("SaveStatusText")).Text = "战绩已保存，可在历史记录中查看。";
        ((TextBlock)main.FindName("SaveStatusText")).Visibility = Visibility.Visible;
        ((TextBlock)main.FindName("RecognitionNotesText")).Text = "已按本场名单对应玩家，请核对战绩。";
        ((TextBlock)main.FindName("StatusText")).Text = "已保存的结算截图";
        ((TextBlock)main.FindName("RoundRosterStatusText")).Text = "本场名单（请核对）：" + string.Join(" / ", record.Players.Select(p => p.Nickname));
        Render(main, Path.Combine(outputDirectory, "recognition.png"), 1200, 750);
        Console.WriteLine(outputDirectory);
        history.Close();
        main.Close();
    }

    private static void Render(Window window, string file, int width, int height)
    {
        var root = (FrameworkElement)window.Content;
        root.Measure(new Size(width, height));
        root.Arrange(new Rect(0, 0, width, height));
        root.UpdateLayout();
        var image = new RenderTargetBitmap(width, height, 96, 96, PixelFormats.Pbgra32);
        image.Render(root);
        var background = new DrawingVisual();
        using (var drawing = background.RenderOpen())
        {
            drawing.DrawRectangle(window.Background, null, new Rect(0, 0, width, height));
            drawing.DrawImage(image, new Rect(0, 0, width, height));
        }
        var composite = new RenderTargetBitmap(width, height, 96, 96, PixelFormats.Pbgra32);
        composite.Render(background);
        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(composite));
        using var stream = File.Create(file);
        encoder.Save(stream);
    }
}
