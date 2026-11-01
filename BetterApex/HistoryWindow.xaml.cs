using System.IO;
using System.Windows;
using System.Windows.Controls;
using BetterApex.Storage;

namespace BetterApex;

public partial class HistoryWindow : Window
{
    private readonly MatchRepository _repository;
    private StoredMatch? _selected;
    private int _selectionVersion;
    private int _imageVersion;
    private Task _selectionTask = Task.CompletedTask;

    internal HistoryWindow(MatchRepository repository)
    {
        _repository = repository;
        InitializeComponent();
        DataLocationText.Text = "本地数据：" + repository.DataDirectory;
        DataLocationText.ToolTip = repository.DataDirectory;
    }

    private async void Window_Loaded(object sender, RoutedEventArgs e) => await RefreshAsync();
    private async void Refresh_Click(object sender, RoutedEventArgs e) => await RefreshAsync(_selected?.Id);

    internal async Task RefreshAsync(string? selectedId = null)
    {
        RefreshButton.IsEnabled = false;
        HistoryErrorText.Text = "";
        try
        {
            var items = await Task.Run(_repository.List);
            HistoryList.ItemsSource = items;
            HistoryCountText.Text = $"共 {items.Count} 场 · 按保存时间排列";
            EmptyHistoryText.Text = items.Count == 0 ? "还没有保存的战绩。\n识别结算图后，点击“保存战绩”。" : "选择左侧记录查看详情。";
            HistoryList.SelectedItem = items.FirstOrDefault(i => i.Id == selectedId) ?? items.FirstOrDefault();
            await _selectionTask;
        }
        catch (Exception exception)
        {
            System.Diagnostics.Trace.TraceError(exception.ToString());
            HistoryCountText.Text = "暂时无法读取历史记录";
            HistoryErrorText.Text = exception is InvalidDataException ? exception.Message : "无法读取本地数据，请检查数据目录是否可访问；已有数据未被重建或覆盖。";
        }
        finally { RefreshButton.IsEnabled = true; }
    }

    private async void HistorySelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        _selectionTask = SelectRecordAsync(HistoryList.SelectedItem as MatchSummary);
        await _selectionTask;
    }

    private async Task SelectRecordAsync(MatchSummary? summary)
    {
        var version = ++_selectionVersion;
        _imageVersion++;
        _selected = null;
        SavedScreenshotPreview.Source = null;
        DetailsPanel.Visibility = Visibility.Collapsed;
        EmptyHistoryText.Visibility = Visibility.Visible;
        SummaryImageButton.IsEnabled = RosterImageButton.IsEnabled = false;
        if (summary is null) return;
        EmptyHistoryText.Text = "正在读取记录…";
        try
        {
            var record = await Task.Run(() => _repository.Get(summary.Id));
            if (version != _selectionVersion) return;
            if (record is null) throw new InvalidDataException("这条记录已不存在，请刷新列表。");
            _selected = record;
            MatchTitleText.Text = record.Title;
            SavedTimeText.Text = record.TimeLabel;
            RecordStatusText.Text = record.Status;
            SavedPlayers.ItemsSource = record.Players;
            EmptyHistoryText.Visibility = Visibility.Collapsed;
            DetailsPanel.Visibility = Visibility.Visible;
            SummaryImageButton.IsEnabled = true;
            RosterImageButton.IsEnabled = record.RosterScreenshotFile is not null;
            await ShowImageAsync(record.ScreenshotFile, "结算截图", version);
        }
        catch (Exception exception)
        {
            if (version != _selectionVersion) return;
            EmptyHistoryText.Text = "这条记录暂时无法打开。";
            HistoryErrorText.Text = exception is InvalidDataException ? exception.Message : "读取记录失败，请检查本地数据目录。";
        }
    }

    private async Task ShowImageAsync(string file, string caption, int selectionVersion)
    {
        var imageVersion = ++_imageVersion;
        HistoryErrorText.Text = "";
        SavedScreenshotPreview.Source = null;
        ImageCaptionText.Text = caption;
        try
        {
            var image = await Task.Run(() => ScreenshotLoader.Load(_repository.GetVerifiedImagePath(file)));
            if (selectionVersion == _selectionVersion && imageVersion == _imageVersion)
                SavedScreenshotPreview.Source = image.Preview;
        }
        catch (Exception exception)
        {
            if (selectionVersion != _selectionVersion || imageVersion != _imageVersion) return;
            HistoryErrorText.Text = exception is InvalidDataException ? exception.Message : "保存的截图无法读取，文字战绩仍可查看。";
        }
    }

    private async void SummaryImage_Click(object sender, RoutedEventArgs e)
    { if (_selected is not null) await ShowImageAsync(_selected.ScreenshotFile, "结算截图", _selectionVersion); }
    private async void RosterImage_Click(object sender, RoutedEventArgs e)
    { if (_selected?.RosterScreenshotFile is { } file) await ShowImageAsync(file, "局内昵称截图", _selectionVersion); }
}
