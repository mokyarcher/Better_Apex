using System.IO;
using System.Windows;
using BetterApex.Ocr;
using BetterApex.Storage;
using Microsoft.Win32;

namespace BetterApex;

public partial class MainWindow : Window
{
    private string? _screenshotPath;
    private RosterRecognition? _roundRoster;
    private string? _rosterSourcePath;
    private bool _isBusy;
    private readonly MatchRepository _repository;
    private MatchDraft? _saveDraft;
    private ImageEvidence? _rosterEvidence;
    private HistoryWindow? _historyWindow;
    private CaptureWindow? _captureWindow;
    private bool IsRosterMode => RosterMode.IsChecked == true;

    public MainWindow() : this(new MatchRepository(MatchRepository.DefaultDataDirectory())) { }

    internal MainWindow(MatchRepository repository)
    {
        _repository = repository;
        InitializeComponent();
    }

    private async void ImportScreenshot_Click(object sender, RoutedEventArgs e)
    {
        var dialog = new OpenFileDialog
        {
            Title = IsRosterMode ? "选择包含左下角队伍栏的对局截图" : "选择 Apex 结算截图",
            Filter = "截图 (*.png;*.jpg;*.jpeg;*.bmp)|*.png;*.jpg;*.jpeg;*.bmp",
            Multiselect = false,
            CheckFileExists = true
        };
        if (dialog.ShowDialog(this) != true)
            return;
        await LoadScreenshotAsync(dialog.FileName);
    }

    internal async Task<bool> LoadScreenshotAsync(string path)
    {
        ErrorText.Visibility = Visibility.Collapsed;
        SetActionsEnabled(false);
        ImportButton.Content = "正在读取…";
        try
        {
            var screenshot = await Task.Run(() => ScreenshotLoader.Load(path));
            if (IsRosterMode) ClearRoundRoster();
            _screenshotPath = screenshot.FilePath;
            ResetRecognition();
            ScreenshotPreview.Source = screenshot.Preview;
            ScreenshotPreview.Visibility = Visibility.Visible;
            EmptyState.Visibility = Visibility.Collapsed;
            StatusText.Text = Path.GetFileName(screenshot.FilePath);
            StatusText.ToolTip = screenshot.FilePath;
            var size = screenshot.FileSize >= 1024 * 1024
                ? $"{screenshot.FileSize / (1024d * 1024):0.##} MB"
                : $"{screenshot.FileSize / 1024d:0.##} KB";
            FileDetailsText.Text = $"{screenshot.Width} × {screenshot.Height} 像素  ·  {size}  ·  仅本地预览";
            return true;
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException
            or NotSupportedException or System.Runtime.InteropServices.COMException
            or ArgumentException or InvalidOperationException)
        {
            ErrorText.Text = exception is InvalidDataException
                ? exception.Message
                : "无法读取这张图片。请确认文件完整、未被占用，并且是 PNG、JPG 或 BMP 图片。";
            ErrorText.Visibility = Visibility.Visible;
            return false;
        }
        finally
        {
            ImportButton.Content = "选择截图";
            SetActionsEnabled(true);
        }
    }

    private async void RecognizeScreenshot_Click(object sender, RoutedEventArgs e)
        => await RecognizeCurrentAsync(associate: false);

    internal async Task RecognizeCurrentAsync(bool associate)
    {
        if (_screenshotPath is null)
            return;
        if (IsRosterMode) ClearRoundRoster();
        SetActionsEnabled(false);
        ErrorText.Visibility = Visibility.Collapsed;
        ResetRecognition();
        OcrButton.Content = "正在识别…";
        ResultSummaryText.Text = "正在本机分析截图，请稍候…";
        try
        {
            var evidence = await Task.Run(() => ImageEvidence.Capture(_screenshotPath));
            if (IsRosterMode)
            {
                var result = await RosterOcrService.RecognizeAsync(_screenshotPath);
                await Task.Run(() => evidence.VerifyUnchanged(_screenshotPath));
                _rosterEvidence = evidence;
                _roundRoster = result;
                _rosterSourcePath = _screenshotPath;
                UpdateRoundRosterStatus();
                ResultSummaryText.Text = "来自对局左下角的队伍栏";
                RosterResults.ItemsSource = result.Members;
                RecognitionNotesText.Text = "核对三个昵称后，点击“选择同场结算并关联”。开始下一场时请点击“新对局”。";
            }
            else
            {
                var result = await MatchOcrService.RecognizeAsync(_screenshotPath);
                await Task.Run(() => evidence.VerifyUnchanged(_screenshotPath));
                ResultSummaryText.Text = $"小队排名：{result.Rank.Display}  ·  总击杀：{result.SquadKills.Display}";
                if (associate && _roundRoster is not null)
                {
                    var linked = RosterMatcher.Match(_roundRoster, result);
                    _saveDraft = new MatchDraft(result, evidence, linked, _rosterEvidence, DateTimeOffset.UtcNow);
                    PlayerResults.ItemsSource = linked.Players;
                    ResultSummaryText.Text += $"\n已关联 {linked.MatchedCount}/3 名玩家";
                    RecognitionNotesText.Text = linked.MatchedCount == 3 ? "已按本场名单对应玩家，请核对战绩。"
                        : "未能确认的昵称保留待核对；请检查名单和结算图是否来自同一局。";
                    RecognitionNotesText.Text += "\n" + string.Join("\n", result.Warnings.Skip(1));
                }
                else
                {
                    _saveDraft = new MatchDraft(result, evidence, null, null, DateTimeOffset.UtcNow);
                    PlayerResults.ItemsSource = result.Players;
                    RecognitionNotesText.Text = string.Join("\n", result.Warnings);
                }
            }
        }
        catch (Exception exception)
        {
            _saveDraft = null;
            System.Diagnostics.Trace.TraceError(exception.ToString());
            ResultSummaryText.Text = "本次识别未完成，可以重新选择图片后再试。";
            ErrorText.Text = exception is InvalidDataException ? exception.Message
                : "无法完成本地识别，请检查图片和程序目录中的 OCR 模型、运行依赖是否完整。";
            ErrorText.Visibility = Visibility.Visible;
        }
        finally
        {
            OcrButton.Content = IsRosterMode ? "读取昵称" : "识别战绩";
            SetActionsEnabled(true);
        }
    }

    private void ResetRecognition()
    {
        _saveDraft = null;
        SaveMatchButton.IsEnabled = false;
        SaveMatchButton.Visibility = IsRosterMode ? Visibility.Collapsed : Visibility.Visible;
        SaveMatchButton.Content = "保存战绩";
        SaveStatusText.Visibility = Visibility.Collapsed;
        PlayerResults.ItemsSource = null;
        RosterResults.ItemsSource = null;
        PlayerResults.Visibility = IsRosterMode ? Visibility.Collapsed : Visibility.Visible;
        RosterResults.Visibility = IsRosterMode ? Visibility.Visible : Visibility.Collapsed;
        OcrButton.Content = IsRosterMode ? "读取昵称" : "识别战绩";
        ResultsTitleText.Text = IsRosterMode ? "队伍名单" : "识别结果";
        ResultSummaryText.Text = IsRosterMode ? "选择对局截图后，点击“读取昵称”。" : "选择图片后，点击“识别战绩”。";
        RecognitionNotesText.Text = IsRosterMode ? "请使用完整保留左下角队伍栏的对局截图，读取名单后可选择同场结算图关联。"
            : "当前仅测试提供的三人结算页布局，昵称与本人归属请人工核对。";
    }

    private void ScreenshotModeChanged(object sender, RoutedEventArgs e)
    {
        if (!IsInitialized) return;
        ResetRecognition();
        ErrorText.Visibility = Visibility.Collapsed;
    }

    private void SetActionsEnabled(bool enabled)
    {
        _isBusy = !enabled;
        ImportButton.IsEnabled = SummaryMode.IsEnabled = RosterMode.IsEnabled = enabled;
        ClearButton.IsEnabled = enabled && ScreenshotPreview.Source is not null;
        OcrButton.IsEnabled = enabled && _screenshotPath is not null;
        SaveMatchButton.IsEnabled = enabled && _saveDraft is not null;
        HistoryButton.IsEnabled = enabled;
        UpdateRoundRosterStatus();
    }

    private async void AssociateSettlement_Click(object sender, RoutedEventArgs e)
    {
        if (_roundRoster is null || _isBusy) return;
        var dialog = new OpenFileDialog
        {
            Title = "选择与当前队伍名单同一局的结算截图",
            Filter = "截图 (*.png;*.jpg;*.jpeg;*.bmp)|*.png;*.jpg;*.jpeg;*.bmp",
            CheckFileExists = true
        };
        if (dialog.ShowDialog(this) != true) return;
        await AssociateSettlementAsync(dialog.FileName);
    }

    internal async Task AssociateSettlementAsync(string path)
    {
        if (_roundRoster is null || _isBusy) return;
        SummaryMode.IsChecked = true;
        if (await LoadScreenshotAsync(path))
            await RecognizeCurrentAsync(associate: true);
    }

    private void ClearRoundRoster()
    {
        _roundRoster = null;
        _rosterSourcePath = null;
        _rosterEvidence = null;
        UpdateRoundRosterStatus();
    }

    private void UpdateRoundRosterStatus()
    {
        RoundRosterStatusText.Text = _roundRoster is null ? "本场名单：未采集"
            : "本场名单（请核对）：" + string.Join(" / ", _roundRoster.Members.Select(m => m.Nickname));
        RoundRosterStatusText.ToolTip = _rosterSourcePath;
        AssociateButton.IsEnabled = !_isBusy && _roundRoster is not null;
        NewRoundButton.IsEnabled = !_isBusy && (_roundRoster is not null || _screenshotPath is not null);
    }

    private void NewRound_Click(object sender, RoutedEventArgs e)
    {
        ClearRoundRoster();
        ClearScreenshot_Click(sender, e);
    }

    private void ClearScreenshot_Click(object sender, RoutedEventArgs e)
    {
        _screenshotPath = null;
        ResetRecognition();
        OcrButton.IsEnabled = false;
        ScreenshotPreview.Source = null;
        ScreenshotPreview.Visibility = Visibility.Collapsed;
        EmptyState.Visibility = Visibility.Visible;
        StatusText.Text = "尚未选择截图";
        StatusText.ToolTip = null;
        FileDetailsText.Text = "图片仅在本机预览，不会上传或修改原文件。";
        ErrorText.Visibility = Visibility.Collapsed;
        ClearButton.IsEnabled = false;
        UpdateRoundRosterStatus();
    }

    private async void SaveMatch_Click(object sender, RoutedEventArgs e) => await SaveCurrentAsync();

    internal async Task<SaveMatchResult?> SaveCurrentAsync()
    {
        if (_saveDraft is null || _isBusy) return null;
        var draft = _saveDraft;
        SetActionsEnabled(false);
        SaveMatchButton.Content = "正在保存…";
        ErrorText.Visibility = Visibility.Collapsed;
        try
        {
            var saved = await Task.Run(() => _repository.Save(draft));
            SaveStatusText.Text = saved.Message;
            SaveStatusText.Visibility = Visibility.Visible;
            if (_historyWindow?.IsVisible == true) await _historyWindow.RefreshAsync(saved.Id);
            return saved;
        }
        catch (Exception exception)
        {
            System.Diagnostics.Trace.TraceError(exception.ToString());
            ErrorText.Text = exception is InvalidDataException ? exception.Message : "保存失败，请检查数据目录的写入权限和磁盘空间；请勿将本次结果视为已保存。";
            ErrorText.Visibility = Visibility.Visible;
            SaveStatusText.Visibility = Visibility.Collapsed;
            return null;
        }
        finally { SaveMatchButton.Content = "保存战绩"; SetActionsEnabled(true); }
    }

    private void History_Click(object sender, RoutedEventArgs e)
    {
        if (_historyWindow is not null) { _historyWindow.Activate(); return; }
        _historyWindow = new HistoryWindow(_repository) { Owner = this };
        _historyWindow.Closed += (_, _) => _historyWindow = null;
        _historyWindow.Show();
    }

    private void Capture_Click(object sender, RoutedEventArgs e)
    {
        if (_captureWindow is not null) { _captureWindow.Activate(); return; }
        _captureWindow = new CaptureWindow(_repository) { Owner = this };
        _captureWindow.Closed += (_, _) => _captureWindow = null;
        _captureWindow.Show();
    }
}
