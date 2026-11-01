using System.Text;
using System.Text.RegularExpressions;

namespace BetterApex.Ocr;

internal sealed record AssociatedPlayer(PlayerRecognition Result, RosterMember? Member)
{
    public bool IsLinked => Member is not null;
    public string Position => Result.Position + (Member?.Position == "本人" ? " · 本人" : "");
    public string DisplayName => Member?.Nickname ?? "昵称待核对";
    public string Stats => Result.Stats;
    public string DamageLabel => Result.DamageLabel;
}

internal sealed record RosterAssociation(IReadOnlyList<AssociatedPlayer> Players)
{
    public int MatchedCount => Players.Count(p => p.IsLinked);
}

internal static class RosterMatcher
{
    private const double MinimumScore = .82;
    private const double MinimumMargin = .15;

    public static RosterAssociation Match(RosterRecognition roster, MatchRecognition settlement)
    {
        if (roster.Members.Count != 3 || settlement.Players.Count != 3)
            throw new ArgumentException("当前仅支持三人名单与三人结算的关联。");
        var scores = new double[3, 3];
        for (var i = 0; i < 3; i++)
            for (var j = 0; j < 3; j++)
                scores[i, j] = settlement.Players[i].NameCandidates.Append(settlement.Players[i].Name)
                    .Select(name => Similarity(name, roster.Members[j].Nickname)).DefaultIfEmpty(0).Max();

        var assignments = new List<(int[] Members, double Score)>();
        void Search(int row, int used, int[] members, double score)
        {
            if (row == 3) { assignments.Add(((int[])members.Clone(), score)); return; }
            members[row] = -1;
            Search(row + 1, used, members, score);
            for (var member = 0; member < 3; member++)
            {
                if ((used & (1 << member)) != 0 || scores[row, member] < MinimumScore) continue;
                members[row] = member;
                Search(row + 1, used | (1 << member), members, score + scores[row, member]);
            }
        }
        Search(0, 0, new int[3], 0);
        var best = assignments.OrderByDescending(a => a.Score).First();
        var linked = Enumerable.Range(0, 3).Select(row =>
        {
            var member = best.Members[row];
            var alternative = assignments.Where(a => a.Members[row] != member).Select(a => a.Score).DefaultIfEmpty(0).Max();
            return new AssociatedPlayer(settlement.Players[row], member >= 0 && best.Score - alternative >= MinimumMargin
                ? roster.Members[member] : null);
        }).ToArray();

        // The user's own name can recur across unrelated rounds. One shared name
        // alone is not enough evidence to label a settlement with a cached squad.
        if (linked.Count(p => p.IsLinked) < 2)
            linked = settlement.Players.Select(p => new AssociatedPlayer(p, null)).ToArray();
        return new RosterAssociation(linked);
    }

    private static double Similarity(string observed, string target)
    {
        var left = Canonical(observed);
        var right = Canonical(target);
        if (left.EnumerateRunes().Count() < 2 || right.EnumerateRunes().Count() < 2) return 0;
        if (left == right) return 1;
        var a = left.EnumerateRunes().ToArray();
        var b = right.EnumerateRunes().ToArray();
        // A one-character overlap in a short Chinese nickname is too ambiguous.
        if (a.Length < 4 || b.Length < 4 || a.Length > 64 || b.Length > 64) return 0;
        var distance = new int[a.Length + 1, b.Length + 1];
        for (var i = 0; i <= a.Length; i++) distance[i, 0] = i;
        for (var j = 0; j <= b.Length; j++) distance[0, j] = j;
        for (var i = 1; i <= a.Length; i++)
            for (var j = 1; j <= b.Length; j++)
            {
                distance[i, j] = Math.Min(Math.Min(distance[i - 1, j] + 1, distance[i, j - 1] + 1),
                    distance[i - 1, j - 1] + (a[i - 1] == b[j - 1] ? 0 : 1));
                if (i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1])
                    distance[i, j] = Math.Min(distance[i, j], distance[i - 2, j - 2] + 1);
            }
        var score = 1d - (double)distance[a.Length, b.Length] / Math.Max(a.Length, b.Length);
        var coverage = (double)a.Length / b.Length;
        if (coverage is >= .65 and < 1 && (right.StartsWith(left, StringComparison.Ordinal) || right.EndsWith(left, StringComparison.Ordinal)))
            score = Math.Max(score, .82 + .1 * coverage);
        return score;
    }

    private static string Canonical(string name)
        => Regex.Replace(name.Normalize(NormalizationForm.FormKC).ToUpperInvariant(), @"\s", "");
}
