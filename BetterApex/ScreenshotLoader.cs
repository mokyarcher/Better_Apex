using System.IO;
using System.Windows.Media.Imaging;

namespace BetterApex;

internal sealed record Screenshot(string FilePath, int Width, int Height, long FileSize, BitmapSource Preview);

internal static class ScreenshotLoader
{
    // Bound decoding work for an accidental huge image; this is not an OCR limit.
    private const long MaximumFileSize = 64L * 1024 * 1024;
    private const long MaximumPixels = 50_000_000;
    private static readonly HashSet<string> SupportedExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".png", ".jpg", ".jpeg", ".bmp"
    };

    public static Screenshot Load(string path)
    {
        if (!SupportedExtensions.Contains(Path.GetExtension(path)))
            throw new InvalidDataException("请选择 PNG、JPG 或 BMP 格式的截图。");

        using var stream = File.OpenRead(path);
        if (stream.Length > MaximumFileSize)
            throw new InvalidDataException("图片超过 64 MB，请选择较小的截图。");

        var decoder = BitmapDecoder.Create(stream, BitmapCreateOptions.DelayCreation, BitmapCacheOption.None);
        var frame = decoder.Frames[0];
        var width = frame.PixelWidth;
        var height = frame.PixelHeight;
        if (width <= 0 || height <= 0 || (long)width * height > MaximumPixels)
            throw new InvalidDataException("图片尺寸无效或超过 5000 万像素，请选择普通结算截图。");

        stream.Position = 0;
        var preview = new BitmapImage();
        preview.BeginInit();
        preview.CacheOption = BitmapCacheOption.OnLoad;
        preview.StreamSource = stream;
        if (width > 1920)
            preview.DecodePixelWidth = 1920;
        preview.EndInit();
        preview.Freeze();

        return new Screenshot(Path.GetFullPath(path), width, height, stream.Length, preview);
    }
}
