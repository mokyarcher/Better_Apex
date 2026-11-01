using System.IO;
using System.Text.Json;
using System.Windows.Media.Imaging;
using BetterApex.Ocr;
using BetterApex.Storage;

namespace BetterApex.Capture;

internal sealed record AutoJob(string Id, string? Token, DateTimeOffset CapturedAt,
    RosterRecognition? Roster, MatchRecognition? Recognition = null, bool ReviewOnly = false,
    string? SourceHash = null, string? RosterHash = null);
internal sealed record AutoCheckpoint(string SessionId, string? Token, DateTimeOffset CapturedAt = default);

internal sealed class AutoJournal(string dataDirectory)
{
    private string PendingRoot => Path.Combine(dataDirectory, "pending");
    private string CheckpointPath => Path.Combine(dataDirectory, "automation-state.json");
    public string JobDirectory(string id)
    {
        if (!Guid.TryParseExact(id, "N", out _)) throw new InvalidDataException("待保存任务编号无效。");
        return Path.Combine(PendingRoot, id);
    }

    public AutoCheckpoint? ReadCheckpoint()
    {
        if (!File.Exists(CheckpointPath)) return null;
        return Read<AutoCheckpoint>(CheckpointPath);
    }

    public IReadOnlyList<AutoJob> Pending()
    {
        if (!Directory.Exists(PendingRoot)) return [];
        var jobs = new List<AutoJob>();
        foreach (var directory in Directory.EnumerateDirectories(PendingRoot))
        {
            if ((File.GetAttributes(directory) & FileAttributes.ReparsePoint) != 0) continue;
            var id = Path.GetFileName(directory);
            if (!Guid.TryParseExact(id, "N", out _)) continue;
            var path = Path.Combine(directory, "job.json");
            if (!File.Exists(path)) continue;
            var job = Read<AutoJob>(path);
            if (job.Id != id) throw new InvalidDataException("待保存任务的编号与目录不一致。");
            jobs.Add(job);
        }
        return jobs.OrderBy(j => j.CapturedAt).ToArray();
    }

    public AutoJob Capture(AutoJob job, CapturedFrame settlement, CapturedFrame? roster)
    {
        if (Pending().Count >= 20 && !File.Exists(Path.Combine(JobDirectory(job.Id), "job.json")))
            throw new IOException("待保存战绩已达到 20 场，请先处理保存失败记录。");
        var directory = JobDirectory(job.Id);
        Directory.CreateDirectory(directory);
        CaptureScreenshot.Save(settlement, Path.Combine(directory, "settlement.png"));
        if (roster is not null) CaptureScreenshot.Save(roster, Path.Combine(directory, "roster.png"));
        job = job with { SourceHash = ImageEvidence.Capture(Path.Combine(directory, "settlement.png")).Hash,
            RosterHash = roster is null ? null : ImageEvidence.Capture(Path.Combine(directory, "roster.png")).Hash };
        WriteJob(job); // Manifest appears only after evidence is complete.
        return job;
    }

    public void WriteJob(AutoJob job) => Write(Path.Combine(JobDirectory(job.Id), "job.json"), job);

    public SaveMatchResult Save(AutoJob job, MatchRepository repository)
    {
        if (job.Recognition is null || job.ReviewOnly) throw new InvalidDataException("待保存战绩尚未完成稳定识别。");
        var directory = JobDirectory(job.Id);
        var rosterPath = Path.Combine(directory, "roster.png");
        var hasRoster = job.Roster is not null && File.Exists(rosterPath);
        var evidence = ImageEvidence.Capture(Path.Combine(directory, "settlement.png"));
        var rosterEvidence = hasRoster ? ImageEvidence.Capture(rosterPath) : null;
        if (evidence.Hash != job.SourceHash || rosterEvidence?.Hash != job.RosterHash)
            throw new InvalidDataException("待保存原图已改变，未使用旧识别结果保存。");
        var draft = new MatchDraft(job.Recognition, evidence,
            hasRoster ? RosterMatcher.Match(job.Roster!, job.Recognition) : null,
            rosterEvidence, job.CapturedAt, job.Id);
        var result = repository.Save(draft);
        if (ReadCheckpoint() is not { } checkpoint || job.CapturedAt >= checkpoint.CapturedAt)
            Write(CheckpointPath, new AutoCheckpoint(job.Id, job.Token, job.CapturedAt));
        // Delete only our own task files after the database and checkpoint commit.
        try
        {
            foreach (var name in new[] { "job.json", "settlement.png", "roster.png" })
            {
                var file = Path.Combine(directory, name);
                if (File.Exists(file)) File.Delete(file);
            }
            Directory.Delete(directory); // Nonrecursive: leave any unknown files alone.
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            // Database and checkpoint are already durable. A transient file
            // lock during cleanup must not report a successful save as failed.
            System.Diagnostics.Trace.TraceWarning("Saved match pending-file cleanup: " + exception.Message);
        }
        return result;
    }

    public void VerifyEvidence(AutoJob job)
    {
        var directory = JobDirectory(job.Id);
        if (job.SourceHash is null || ImageEvidence.Capture(Path.Combine(directory, "settlement.png")).Hash != job.SourceHash
            || (job.RosterHash is not null && ImageEvidence.Capture(Path.Combine(directory, "roster.png")).Hash != job.RosterHash))
            throw new InvalidDataException("待保存原图已改变，未使用旧识别结果保存。");
    }

    private static T Read<T>(string path)
    {
        if (new FileInfo(path).Length > 1024 * 1024) throw new InvalidDataException("自动记录状态文件过大。");
        return JsonSerializer.Deserialize<T>(File.ReadAllText(path)) ?? throw new InvalidDataException("自动记录状态文件无效。");
    }

    private static void Write<T>(string path, T data)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        var temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try
        {
            File.WriteAllText(temporary, JsonSerializer.Serialize(data));
            File.Move(temporary, path, overwrite: true);
        }
        finally { if (File.Exists(temporary)) File.Delete(temporary); }
    }
}
