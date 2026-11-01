using System.Runtime.InteropServices.WindowsRuntime;
using System.Text;
using System.Text.RegularExpressions;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;

namespace BetterApex.Ocr;

internal static class WindowsTextOcr
{
    public static OcrEngine? CreateChineseEngine()
    {
        var language = OcrEngine.AvailableRecognizerLanguages.FirstOrDefault(l => l.LanguageTag.StartsWith("zh-Hans", StringComparison.OrdinalIgnoreCase));
        return language is null ? null : OcrEngine.TryCreateFromLanguage(language);
    }

    public static OcrResult Read(OcrEngine engine, BitmapSource source, Int32Rect bounds)
    {
        var scale = Math.Min(3d, (OcrEngine.MaxImageDimension - 2d) / Math.Max(bounds.Width, bounds.Height));
        var scaled = new TransformedBitmap(new CroppedBitmap(source, bounds), new ScaleTransform(scale, scale));
        var color = new FormatConvertedBitmap(scaled, PixelFormats.Bgra32, null, 0);
        var pixels = new byte[color.PixelWidth * color.PixelHeight * 4];
        color.CopyPixels(pixels, color.PixelWidth * 4, 0);
        using var bitmap = SoftwareBitmap.CreateCopyFromBuffer(pixels.AsBuffer(), BitmapPixelFormat.Bgra8,
            color.PixelWidth, color.PixelHeight, BitmapAlphaMode.Ignore);
        return engine.RecognizeAsync(bitmap).AsTask().GetAwaiter().GetResult();
    }

    public static string JoinWords(IEnumerable<OcrWord> words)
    {
        var text = new StringBuilder();
        OcrWord? previous = null;
        foreach (var word in words.Where(w => !string.IsNullOrWhiteSpace(w.Text)).OrderBy(w => w.BoundingRect.X))
        {
            var gap = previous is null ? 0 : word.BoundingRect.X - (previous.BoundingRect.X + previous.BoundingRect.Width);
            if (text.Length > 0 && previous is not null && gap > previous.BoundingRect.Height * .25
                && !Regex.IsMatch($"{text[^1]}{word.Text[0]}", @"^[\p{IsCJKUnifiedIdeographs}]{2}$"))
                text.Append(' ');
            text.Append(word.Text);
            previous = word;
        }
        return text.ToString().Trim();
    }
}
