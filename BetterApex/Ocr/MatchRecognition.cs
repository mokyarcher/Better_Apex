using System.Text.RegularExpressions;

namespace BetterApex.Ocr;

internal sealed record RecognizedField(string RawText, int? Value, float Confidence)
{
    public string Display => Value?.ToString() ?? "待核对";
}

internal sealed record PlayerRecognition(string Position, string Name, RecognizedField Kills,
    RecognizedField Assists, RecognizedField Knocks, RecognizedField Damage, string RawKda)
{
    public IReadOnlyList<string> NameCandidates { get; init; } = [];
    public string DisplayName => string.IsNullOrWhiteSpace(Name) ? "昵称未识别" : $"{Name}（待核对）";
    public string Stats => $"击杀 {Kills.Display}  /  助攻 {Assists.Display}  /  击倒 {Knocks.Display}";
    public string DamageLabel => $"伤害  {Damage.Display}";
}

internal sealed record MatchRecognition(RecognizedField Rank, RecognizedField SquadKills,
    IReadOnlyList<PlayerRecognition> Players, IReadOnlyList<string> Warnings);

internal static class OcrValueParser
{
    public static RecognizedField Number(string raw, float confidence, int maximum = int.MaxValue)
    {
        var text = Regex.Replace(raw, @"\s", "").Replace('，', ',');
        if (Regex.IsMatch(text, @"^(?:[0-9]+|[0-9]{1,3}(?:,[0-9]{3})+)$")
            && int.TryParse(text.Replace(",", ""), out var value) && value <= maximum)
            return new(raw, value, confidence);
        return new(raw, null, confidence);
    }

    public static RecognizedField[] Kda(string raw, float confidence)
    {
        var text = Regex.Replace(raw, @"\s", "").Replace('／', '/');
        var match = Regex.Match(text, @"^([0-9]{1,3})/([0-9]{1,3})/([0-9]{1,3})$");
        return Enumerable.Range(1, 3).Select(i => new RecognizedField(raw,
            match.Success ? int.Parse(match.Groups[i].Value) : null, confidence)).ToArray();
    }
}
