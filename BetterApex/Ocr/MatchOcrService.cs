using System.IO;
using System.Windows;
using System.Windows.Media.Imaging;
using Tesseract;
using static BetterApex.Ocr.ScreenshotOcr;

namespace BetterApex.Ocr;

internal static class MatchOcrService
{
    public static Task<MatchRecognition> RecognizeAsync(string path) => RunAsync(() => Recognize(path));

    // First template: the full 16:9, three-player summary supplied for this prototype.
    // Positions are relative to the image, not to a particular pixel resolution.
    public static MatchRecognition Recognize(string path)
        => Recognize(LoadOriginal(path));

    internal static MatchRecognition Recognize(BitmapSource source)
    {
        if (Math.Abs((double)source.PixelWidth / source.PixelHeight - 16d / 9) > 0.025)
            throw new InvalidDataException("当前仅测试完整的 16:9 三人结算截图，请选择未裁剪的结算图。");

        var dataPath = Path.Combine(AppContext.BaseDirectory, "Ocr", "tessdata");
        using var numbers = new TesseractEngine(dataPath, "eng", EngineMode.LstmOnly);
        using var names = new TesseractEngine(dataPath, "chi_sim+eng", EngineMode.LstmOnly);
        var windowsNames = WindowsTextOcr.CreateChineseEngine();
        numbers.SetVariable("tessedit_char_whitelist", "0123456789/,");
        var rankRaw = ReadHeaderNumber(numbers, source, Region(source, .740, .140, .092, .065),
            hasRankPrefix: true, fallback: Region(source, .784, .151, .024, .055));
        var rank = OcrValueParser.Number(rankRaw.Text, rankRaw.Confidence, 60);
        if (rank.Value == 0)
            rank = rank with { Value = null };
        var squadRaw = ReadHeaderNumber(numbers, source, Region(source, .890, .145, .048, .060), hasRankPrefix: false);
        var squad = OcrValueParser.Number(squadRaw.Text, squadRaw.Confidence);
        var players = new List<PlayerRecognition>();
        var positions = new[] { "左侧玩家", "中间玩家", "右侧玩家" };
        var columns = new[] { .118, .394, .670 };
        for (var i = 0; i < columns.Length; i++)
        {
            var x = columns[i];
            var name = Read(names, source, x, .274, .080, .039);
            var nameCandidates = new List<string> { name.Text.Trim() };
            if (windowsNames is not null)
            {
                var bounds = Region(source, x - .012, .255, .160, .070);
                var native = WindowsTextOcr.Read(windowsNames, source, bounds);
                var line = native.Lines.Where(l => l.Words.Count > 0).OrderBy(l => Math.Abs(
                    l.Words.Average(w => w.BoundingRect.Y + w.BoundingRect.Height / 2) - (.290 * source.PixelHeight - bounds.Y) * 3)).FirstOrDefault();
                if (line is not null)
                {
                    var words = line.Words.OrderBy(w => w.BoundingRect.X).ToArray();
                    var height = words.Max(w => w.BoundingRect.Height);
                    for (var start = 0; start < words.Length; start++)
                    {
                        if (start > 0 && words.Take(start).Any(w => w.BoundingRect.Height >= height * .5)) break;
                        for (var count = 1; count <= words.Length - start; count++)
                            nameCandidates.Add(WindowsTextOcr.JoinWords(words.Skip(start).Take(count)));
                    }
                }
            }
            var kdaRaw = Read(numbers, source, x, .375, .066, .038);
            var damageRaw = Read(numbers, source, x, .440, .050, .038);
            var kda = OcrValueParser.Kda(kdaRaw.Text, kdaRaw.Confidence);
            // A parseable line can still contain a slash misread as a digit (1/0/71).
            // Validate against the actual image groups even when the regex succeeds.
            {
                var row = Region(source, x, .375, .066, .038);
                var groups = KdaImageSplitter.TrySplit(source, row);
                if (groups is not null)
                {
                    numbers.SetVariable("tessedit_char_whitelist", "0123456789");
                    try
                    {
                        var readings = groups.Select(region => ReadNumberGroup(numbers, source, region)).ToArray();
                        var parsed = readings.Select(reading => OcrValueParser.Number(reading.Text, reading.Confidence, 999)).ToArray();
                        kda = parsed;
                        kdaRaw = ($"{string.Join(" / ", readings.Select(r => r.Text))}（分组识别；整行：{kdaRaw.Text}）", parsed.Min(f => f.Confidence));
                    }
                    finally { numbers.SetVariable("tessedit_char_whitelist", "0123456789/,"); }
                }
            }
            var damage = OcrValueParser.Number(damageRaw.Text, damageRaw.Confidence);
            if (damage.Confidence < .70f)
            {
                var word = Read(numbers, source, x, .440, .050, .038, PageSegMode.SingleWord);
                var raw = Read(numbers, source, x, .440, .050, .038, PageSegMode.RawLine);
                var wordValue = OcrValueParser.Number(word.Text, word.Confidence);
                var rawValue = OcrValueParser.Number(raw.Text, raw.Confidence);
                damage = wordValue.Value is not null && wordValue.Value == rawValue.Value
                    ? wordValue with { Confidence = Math.Min(word.Confidence, raw.Confidence) }
                    : damage with { Value = null };
            }
            players.Add(new PlayerRecognition(positions[i], name.Text.Trim(), kda[0], kda[1], kda[2], damage, kdaRaw.Text)
            {
                NameCandidates = nameCandidates.Where(n => n.Length > 0).Distinct().ToArray()
            });
        }

        var warnings = new List<string> { "当前只测试这一种结算布局；昵称与本人归属请人工核对。" };
        var fields = players.SelectMany(p => new[] { p.Kills, p.Assists, p.Knocks, p.Damage }).Append(rank).Append(squad).ToArray();
        if (fields.Any(f => f.Value is null))
            warnings.Add("部分数字未能完整识别，已标记为待核对，没有补成 0。");
        if (fields.Any(f => f.Value is not null && f.Confidence < .70f))
            warnings.Add("部分数字的识别可信度较低，请对照原图检查。");
        if (squad.Value is not null && players.All(p => p.Kills.Value is not null)
            && players.Sum(p => p.Kills.Value!.Value) != squad.Value)
            warnings.Add("三人击杀之和与小队总击杀不一致，请核对。");
        if (fields.All(f => f.Value is null))
            throw new InvalidDataException("未识别到此模板的有效战绩数字，请检查是否为完整的三人结算页。");
        return new MatchRecognition(rank, squad, players, warnings);
    }

    private static (string Text, float Confidence) ReadNumberGroup(TesseractEngine engine, BitmapSource source, KdaNumberRegion group)
    {
        var reading = Read(engine, source, group.Bounds, PageSegMode.SingleWord);
        if (reading.Text.Length == group.Digits.Count && OcrValueParser.Number(reading.Text, reading.Confidence, 999).Value is not null)
            return reading;
        var digits = group.Digits.Select(rect => Read(engine, source, rect, PageSegMode.SingleChar)).ToArray();
        if (digits.All(d => d.Text.Length == 1 && d.Text[0] is >= '0' and <= '9'))
            return (string.Concat(digits.Select(d => d.Text)), digits.Min(d => d.Confidence));
        // Do not accept the original word if its digit count contradicts the image.
        return (reading.Text.Length == group.Digits.Count ? reading.Text : "", 0);
    }

    private static (string Text, float Confidence) ReadHeaderNumber(TesseractEngine engine, BitmapSource source,
        Int32Rect row, bool hasRankPrefix, Int32Rect? fallback = null)
    {
        var group = KdaImageSplitter.TryLocateHeaderNumber(source, row, hasRankPrefix);
        if (group is null)
            return Read(engine, source, fallback ?? row, PageSegMode.RawLine);
        engine.SetVariable("tessedit_char_whitelist", "0123456789");
        try { return ReadNumberGroup(engine, source, group); }
        finally { engine.SetVariable("tessedit_char_whitelist", "0123456789/,"); }
    }

}
