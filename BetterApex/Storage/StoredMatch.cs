using BetterApex.Ocr;

namespace BetterApex.Storage;

internal sealed record MatchDraft(MatchRecognition Recognition, ImageEvidence Screenshot,
    RosterAssociation? Association, ImageEvidence? RosterScreenshot, DateTimeOffset RecognizedAtUtc,
    string? CaptureSessionId = null);

internal sealed record StoredPlayer(int Slot, string SourcePosition, string? Nickname, bool NameConfirmed,
    bool? IsLocalPlayer, int? Kills, int? Assists, int? Knocks, int? Damage)
{
    public string Position => SourcePosition + (IsLocalPlayer == true ? " · 本人" : "");
    public string DisplayName => string.IsNullOrWhiteSpace(Nickname) ? "昵称待核对"
        : NameConfirmed ? Nickname : $"{Nickname}（待核对）";
    public string Stats => $"击杀 {Value(Kills)}  /  助攻 {Value(Assists)}  /  击倒 {Value(Knocks)}";
    public string DamageLabel => $"伤害  {Value(Damage)}";
    private static string Value(int? value) => value?.ToString() ?? "待核对";
}

internal sealed record MatchSummary(string Id, DateTimeOffset SavedAtUtc, int? Rank, int? SquadKills, string Names)
{
    public string Title => $"{DisplayTime(SavedAtUtc):MM-dd HH:mm:ss}  ·  第 {Rank?.ToString() ?? "?"} 名";
    public string Subtitle => $"小队击杀 {SquadKills?.ToString() ?? "待核对"}  ·  {Names}";
    public static DateTimeOffset DisplayTime(DateTimeOffset utc)
        => TimeZoneInfo.ConvertTime(utc, TimeZoneInfo.FindSystemTimeZoneById("China Standard Time"));
}

internal sealed record StoredMatch(string Id, DateTimeOffset SavedAtUtc, DateTimeOffset UpdatedAtUtc,
    DateTimeOffset RecognizedAtUtc, int? Rank, int? SquadKills, string ScreenshotFile, string? RosterScreenshotFile,
    IReadOnlyList<StoredPlayer> Players)
{
    public string Title => $"第 {Rank?.ToString() ?? "?"} 名  ·  小队总击杀 {SquadKills?.ToString() ?? "待核对"}";
    public string TimeLabel => $"保存时间：{MatchSummary.DisplayTime(SavedAtUtc):yyyy-MM-dd HH:mm:ss}（北京时间）";
    public string Status => $"{Players.Count(p => p.NameConfirmed)}/3 名昵称已关联；缺失字段保留待核对。";
}

internal sealed record SaveMatchResult(string Id, bool Created, bool Updated, string Message);
