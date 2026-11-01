using System.IO;
using System.Text.RegularExpressions;
using System.Windows.Media.Imaging;
using System.Windows.Media;
using BetterApex.Ocr;
using Tesseract;
using Windows.Media.Ocr;

namespace BetterApex.Capture;

internal enum AutoPage { Unknown, InMatch, Settlement }
internal sealed record PageObservation(AutoPage Page, string? Token = null, int? Squads = null, bool NewRoundIntro = false);

internal sealed class AutoPageDetector : IDisposable
{
    private readonly OcrEngine _text = WindowsTextOcr.CreateChineseEngine()
        ?? throw new InvalidDataException("自动记录需要本机 Windows 简体中文 OCR 支持，请先安装对应语言组件。");
    private readonly TesseractEngine _marker = new(Path.Combine(AppContext.BaseDirectory, "Ocr", "tessdata"), "eng", EngineMode.LstmOnly);
    internal string MarkerDiagnostic { get; private set; } = "";

    public PageObservation Observe(BitmapSource image, bool lookForIntro)
    {
        if (Math.Abs((double)image.PixelWidth / image.PixelHeight - 16d / 9) > .025)
            throw new InvalidDataException("自动记录当前支持完整的 16:9 游戏画面，请检查分辨率和黑边。");
        var header = Text(image, .690, .135, .255, .085);
        if (header.Contains("小队排名") && header.Contains("小队总击杀"))
            return new(AutoPage.Settlement, ReadToken(image, true));
        var hud = Text(image, .800, .035, .145, .060);
        var count = Regex.Match(hud, @"(\d{1,2})剩余小队");
        if (count.Success && int.TryParse(count.Groups[1].Value, out var squads) && squads is >= 1 and <= 20)
            return new(AutoPage.InMatch, ReadToken(image, false), squads);
        var intro = lookForIntro ? Text(image, .20, .04, .60, .18) : "";
        return new(AutoPage.Unknown, NewRoundIntro: intro is "你的小队" or "您的小队");
    }

    private string Text(BitmapSource source, double x, double y, double w, double h)
        => Regex.Replace(WindowsTextOcr.Read(_text, source, ScreenshotOcr.Region(source, x, y, w, h)).Text, @"\s", "");

    private string? ReadToken(BitmapSource source, bool settlement)
    {
        var y = settlement ? .925 : .978;
        var height = settlement ? .028 : .022;
        var native = Text(source, .0, y, .15, height);
        string? Extract(string text)
        {
            var found = Regex.Match(Regex.Replace(text, @"\s", ""), @"[0-9:]{12,}:([a-fA-F0-9]{8})(?![a-fA-F0-9])");
            return found.Success ? found.Groups[1].Value.ToLowerInvariant() : null;
        }
        var nativeToken = Extract(native);
        _marker.SetVariable("tessedit_char_whitelist", "0123456789abcdefABCDEF:");
        var original = ScreenshotOcr.Read(_marker, source, .0, y, .15, height, PageSegMode.SingleLine);
        var originalToken = Extract(original.Text);
        if (original.Confidence >= .65f && originalToken is not null)
        { MarkerDiagnostic = original.Text; return originalToken; }
        var crop = new CroppedBitmap(source, ScreenshotOcr.Region(source, .0, y, .15, height));
        var color = new FormatConvertedBitmap(crop, PixelFormats.Bgra32, null, 0);
        var pixels = new byte[color.PixelWidth * color.PixelHeight * 4];
        color.CopyPixels(pixels, color.PixelWidth * 4, 0);
        var white = new byte[color.PixelWidth * color.PixelHeight];
        for (var i = 0; i < white.Length; i++)
            white[i] = Math.Min(pixels[i * 4], Math.Min(pixels[i * 4 + 1], pixels[i * 4 + 2])) > 165 ? (byte)255 : (byte)0;
        var prepared = BitmapSource.Create(color.PixelWidth, color.PixelHeight, 96, 96, PixelFormats.Gray8, null, white, color.PixelWidth);
        var raw = ScreenshotOcr.Read(_marker, prepared, new System.Windows.Int32Rect(0, 0, prepared.PixelWidth, prepared.PixelHeight), PageSegMode.SingleLine);
        MarkerDiagnostic = $"{raw.Text} ({raw.Confidence})";
        // A visible screen marker is auxiliary evidence, not an official match ID.
        var filteredToken = Extract(raw.Text);
        if (filteredToken is not null && raw.Confidence >= .65f) return filteredToken;
        if (originalToken is not null && original.Confidence >= .35f
            && (originalToken == nativeToken || (originalToken == filteredToken && raw.Confidence >= .35f))) return originalToken;
        return filteredToken is not null && filteredToken == nativeToken && raw.Confidence >= .35f ? filteredToken : null;
    }

    public void Dispose() => _marker.Dispose();
}
