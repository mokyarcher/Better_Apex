using System.IO;
using System.Text;
using System.Text.RegularExpressions;
using Windows.Media.Ocr;
using System.Windows.Media.Imaging;
using static BetterApex.Ocr.ScreenshotOcr;

namespace BetterApex.Ocr;

internal sealed record RosterMember(string Position, string Nickname, bool NeedsReview)
{
    public string DisplayName => string.IsNullOrWhiteSpace(Nickname) ? "待核对"
        : NeedsReview ? $"{Nickname}（待核对）" : Nickname;
}

internal sealed record RosterRecognition(IReadOnlyList<RosterMember> Members);

internal static class RosterOcrService
{
    public static Task<RosterRecognition> RecognizeAsync(string path) => RunAsync(() => Recognize(path));

    private static RosterRecognition Recognize(string path)
        => Recognize(LoadOriginal(path));

    internal static RosterRecognition Recognize(BitmapSource source)
    {
        if (Math.Abs((double)source.PixelWidth / source.PixelHeight - 16d / 9) > .025)
            throw new InvalidDataException("请使用未裁剪的 16:9 对局截图，完整保留左下角队伍栏。");
        var engine = WindowsTextOcr.CreateChineseEngine();
        if (engine is null)
            throw new InvalidDataException("未检测到本机简体中文 OCR 支持，暂时无法读取中文队伍昵称；结算战绩功能仍可使用。");
        var slots = new[]
        {
            // Names begin to the right of the portrait's diagonal frame. The
            // wider old crop included that frame, which OCR can read as '/'.
            // Keep the right edge; do not strip punctuation from player names.
            (Position: "队友 1（上方）", X: .070, Y: .715, Width: .090, Height: .045, CenterY: .738),
            (Position: "队友 2（下方）", X: .070, Y: .775, Width: .090, Height: .045, CenterY: .798),
            (Position: "本人", X: .085, Y: .880, Width: .130, Height: .050, CenterY: .904)
        };
        var members = slots.Select(slot =>
        {
            var bounds = Region(source, slot.X, slot.Y, slot.Width, slot.Height);
            var result = WindowsTextOcr.Read(engine, source, bounds);
            var line = result.Lines.Where(l => l.Words.Count > 0).OrderBy(l => Math.Abs(l.Words.Average(w => w.BoundingRect.Y + w.BoundingRect.Height / 2)
                - (slot.CenterY * source.PixelHeight - bounds.Y) * 3)).FirstOrDefault();
            var reading = line is null ? (Name: "", Trimmed: false) : ReadNickname(line.Words);
            return new RosterMember(slot.Position, reading.Name, reading.Trimmed || reading.Name.Length == 0);
        }).ToArray();
        if (members.All(m => m.Nickname.Length == 0))
            throw new InvalidDataException("未读取到队伍昵称，请检查是否完整显示了对局左下角队伍栏。");
        return new RosterRecognition(members);
    }

    private static (string Name, bool Trimmed) ReadNickname(IReadOnlyList<OcrWord> words)
    {
        var ordered = words.Where(w => !string.IsNullOrWhiteSpace(w.Text)).OrderBy(w => w.BoundingRect.X).ToArray();
        if (ordered.Length == 0) return ("", false);
        var text = new StringBuilder();
        var first = ordered[0].BoundingRect;
        OcrWord? previous = null;
        foreach (var word in ordered)
        {
            var rect = word.BoundingRect;
            var gap = previous is null ? 0 : rect.X - (previous.BoundingRect.X + previous.BoundingRect.Width);
            // A separated, lower/smaller symbol can be a voice icon. Mark a
            // trimmed candidate for review rather than silently trusting it.
            if (previous is not null && gap > first.Height * .4
                && (rect.Y > first.Y + first.Height * .12 || rect.Height < first.Height * .75))
                return (text.ToString().Trim(), true);
            if (text.Length > 0 && gap > first.Height * .25
                && !Regex.IsMatch($"{text[^1]}{word.Text[0]}", @"^[\p{IsCJKUnifiedIdeographs}]{2}$"))
                text.Append(' ');
            text.Append(word.Text);
            previous = word;
        }
        return (text.ToString().Trim(), false);
    }
}
