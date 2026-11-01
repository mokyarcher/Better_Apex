using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace BetterApex.Ocr;

internal sealed record KdaNumberRegion(Int32Rect Bounds, IReadOnlyList<Int32Rect> Digits);

internal static class KdaImageSplitter
{
    // Split at actual orange slash glyphs, rather than guessing a missing separator
    // from an OCR string or deriving a player's kills from the squad total.
    public static KdaNumberRegion[]? TrySplit(BitmapSource source, Int32Rect row)
    {
        var (mask, glyphs) = FindGlyphs(source, row);

        bool IsSlash(Int32Rect glyph)
        {
            if (glyph.Width < glyph.Height * .2 || glyph.Width > glyph.Height * .65)
                return false;
            var band = Math.Max(1, glyph.Height / 4);
            var upper = new List<int>();
            var lower = new List<int>();
            for (var x = glyph.X; x < glyph.X + glyph.Width; x++)
                for (var y = glyph.Y; y < glyph.Y + glyph.Height; y++)
                {
                    if (!mask[y * row.Width + x]) continue;
                    if (y < glyph.Y + band) upper.Add(x);
                    if (y >= glyph.Y + glyph.Height - band) lower.Add(x);
                }
            return upper.Count > 0 && lower.Count > 0
                && upper.Average() - lower.Average() >= glyph.Width * .35
                && upper.Max() - upper.Min() + 1 <= glyph.Width * .7;
        }

        var slashes = glyphs.Where(IsSlash).ToArray();
        if (slashes.Length != 2)
            return null;

        var groups = new[]
        {
            glyphs.Where(g => g.X + g.Width <= slashes[0].X).ToArray(),
            glyphs.Where(g => g.X >= slashes[0].X + slashes[0].Width && g.X + g.Width <= slashes[1].X).ToArray(),
            glyphs.Where(g => g.X >= slashes[1].X + slashes[1].Width).ToArray()
        };
        if (groups.Any(g => g.Length is < 1 or > 3))
            return null;
        return groups.Select(group => NumberRegion(row, group)).ToArray();
    }

    public static KdaNumberRegion? TryLocateHeaderNumber(BitmapSource source, Int32Rect row, bool hasRankPrefix)
    {
        var (_, glyphs) = FindGlyphs(source, row);
        if (hasRankPrefix)
        {
            // The Chinese rank prefix is a wide, taller glyph. Never discard a digit
            // merely because it happens to be the leftmost component.
            if (glyphs.Count is < 2 or > 3 || glyphs[0].Width < glyphs[0].Height * .7
                || glyphs[0].Height <= glyphs.Skip(1).Max(g => g.Height) * 1.05)
                return null;
            glyphs = glyphs.Skip(1).ToList();
        }
        return glyphs.Count is >= 1 and <= 3 ? NumberRegion(row, glyphs) : null;
    }

    private static KdaNumberRegion NumberRegion(Int32Rect row, IReadOnlyList<Int32Rect> group)
    {
        Int32Rect Padded(Int32Rect bounds)
        {
            var left = Math.Max(0, bounds.X - 2);
            var right = Math.Min(row.Width, bounds.X + bounds.Width + 2);
            var top = Math.Max(0, bounds.Y - 2);
            var bottom = Math.Min(row.Height, bounds.Y + bounds.Height + 2);
            return new Int32Rect(row.X + left, row.Y + top, right - left, bottom - top);
        }
        var left = group.Min(g => g.X);
        var top = group.Min(g => g.Y);
        var right = group.Max(g => g.X + g.Width);
        var bottom = group.Max(g => g.Y + g.Height);
        return new KdaNumberRegion(Padded(new Int32Rect(left, top, right - left, bottom - top)),
            group.Select(Padded).ToArray());
    }

    private static (bool[] Mask, List<Int32Rect> Glyphs) FindGlyphs(BitmapSource source, Int32Rect row)
    {
        var crop = new FormatConvertedBitmap(new CroppedBitmap(source, row), PixelFormats.Bgra32, null, 0);
        var pixels = new byte[row.Width * row.Height * 4];
        crop.CopyPixels(pixels, row.Width * 4, 0);
        var mask = new bool[row.Width * row.Height];
        for (var i = 0; i < mask.Length; i++)
        {
            var blue = pixels[i * 4];
            var green = pixels[i * 4 + 1];
            var red = pixels[i * 4 + 2];
            mask[i] = red >= 130 && red - green >= 25 && green - blue >= 20;
        }
        bool HasInk(int x) => Enumerable.Range(0, row.Height).Any(y => mask[y * row.Width + x]);
        var glyphs = new List<Int32Rect>();
        for (var x = 0; x < row.Width; x++)
        {
            if (!HasInk(x)) continue;
            var left = x;
            while (x + 1 < row.Width && HasInk(x + 1)) x++;
            var top = row.Height;
            var bottom = -1;
            for (var xx = left; xx <= x; xx++)
                for (var y = 0; y < row.Height; y++)
                    if (mask[y * row.Width + xx]) { top = Math.Min(top, y); bottom = Math.Max(bottom, y); }
            if (bottom - top + 1 >= Math.Max(4, row.Height * .4))
                glyphs.Add(new Int32Rect(left, top, x - left + 1, bottom - top + 1));
        }
        return (mask, glyphs);
    }
}
