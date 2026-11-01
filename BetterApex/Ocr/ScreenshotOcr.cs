using System.IO;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Tesseract;

namespace BetterApex.Ocr;

internal static class ScreenshotOcr
{
    public static Task<T> RunAsync<T>(Func<T> work, bool lowPriority = false)
    {
        var completion = new TaskCompletionSource<T>(TaskCreationOptions.RunContinuationsAsynchronously);
        var thread = new Thread(() =>
        {
            try { completion.SetResult(work()); }
            catch (Exception exception) { completion.SetException(exception); }
        }) { IsBackground = true, Name = "Local screenshot OCR", Priority = lowPriority ? ThreadPriority.BelowNormal : ThreadPriority.Normal };
        thread.SetApartmentState(ApartmentState.STA);
        thread.Start();
        return completion.Task;
    }

    public static BitmapSource LoadOriginal(string path)
    {
        _ = ScreenshotLoader.Load(path);
        using var file = File.OpenRead(path);
        var bitmap = new BitmapImage();
        bitmap.BeginInit();
        bitmap.CacheOption = BitmapCacheOption.OnLoad;
        bitmap.StreamSource = file;
        bitmap.EndInit();
        bitmap.Freeze();
        return bitmap;
    }

    public static Int32Rect Region(BitmapSource source, double x, double y, double width, double height)
        => new((int)Math.Round(x * source.PixelWidth), (int)Math.Round(y * source.PixelHeight),
            (int)Math.Round(width * source.PixelWidth), (int)Math.Round(height * source.PixelHeight));

    public static (string Text, float Confidence) Read(TesseractEngine engine, BitmapSource source,
        double x, double y, double width, double height, PageSegMode mode = PageSegMode.SingleLine)
        => Read(engine, source, Region(source, x, y, width, height), mode);

    public static (string Text, float Confidence) Read(TesseractEngine engine, BitmapSource source,
        Int32Rect rect, PageSegMode mode)
    {
        var crop = new CroppedBitmap(source, rect);
        var color = new FormatConvertedBitmap(crop, PixelFormats.Bgra32, null, 0);
        var pixels = new byte[rect.Width * rect.Height * 4];
        color.CopyPixels(pixels, rect.Width * 4, 0);
        var gray = new byte[rect.Width * rect.Height];
        for (var i = 0; i < gray.Length; i++)
        {
            var brightest = Math.Max(pixels[i * 4], Math.Max(pixels[i * 4 + 1], pixels[i * 4 + 2]));
            gray[i] = (byte)(255 - brightest);
        }
        var prepared = BitmapSource.Create(rect.Width, rect.Height, 96, 96, PixelFormats.Gray8, null, gray, rect.Width);
        var visual = new DrawingVisual();
        using (var drawing = visual.RenderOpen())
        {
            drawing.DrawRectangle(Brushes.White, null, new System.Windows.Rect(0, 0, rect.Width * 3 + 32, rect.Height * 3 + 32));
            drawing.DrawImage(prepared, new System.Windows.Rect(16, 16, rect.Width * 3, rect.Height * 3));
        }
        var rendered = new RenderTargetBitmap(rect.Width * 3 + 32, rect.Height * 3 + 32, 96, 96, PixelFormats.Pbgra32);
        rendered.Render(visual);
        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(rendered));
        using var memory = new MemoryStream();
        encoder.Save(memory);
        using var image = Pix.LoadFromMemory(memory.ToArray());
        using var page = engine.Process(image, mode);
        return (page.GetText().Trim(), page.GetMeanConfidence());
    }
}
