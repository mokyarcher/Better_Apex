using System.IO;
using System.Windows.Media.Imaging;

namespace BetterApex.Capture;

internal static class CaptureScreenshot
{
    public static void Save(CapturedFrame frame, string path)
    {
        var destination = Path.GetFullPath(path);
        var temporary = destination + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try
        {
            var encoder = new PngBitmapEncoder();
            encoder.Frames.Add(BitmapFrame.Create(frame.Image));
            using (var file = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write)) encoder.Save(file);
            File.Move(temporary, destination, overwrite: true);
        }
        finally { if (File.Exists(temporary)) File.Delete(temporary); }
    }
}
