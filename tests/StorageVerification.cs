using System.IO;
using BetterApex.Ocr;
using BetterApex.Storage;
using Microsoft.Data.Sqlite;

internal static class StorageVerification
{
    public static void Run(string rosterPath, string screenshotPath, MatchRecognition match, RosterRecognition roster)
    {
        var directory = NewDirectory("repository");
        var store = new MatchRepository(directory);
        Check(store.List().Count == 0 && !File.Exists(store.DatabasePath), "Empty history does not manufacture records");
        var evidence = ImageEvidence.Capture(screenshotPath);
        var rosterEvidence = ImageEvidence.Capture(rosterPath);
        var plain = new MatchDraft(match, evidence, null, null, DateTimeOffset.UtcNow);
        var linked = plain with { Association = RosterMatcher.Match(roster, match), RosterScreenshot = rosterEvidence };

        var first = store.Save(plain);
        Check(first.Created && store.List().Count == 1, "First save creates one record");
        var savedAt = store.Get(first.Id)!.SavedAtUtc;
        var duplicate = store.Save(plain);
        Check(duplicate.Id == first.Id && !duplicate.Created && !duplicate.Updated && store.List().Count == 1, "Duplicate screenshot is not appended");
        var upgraded = store.Save(linked);
        Check(upgraded.Id == first.Id && upgraded.Updated, "Association upgrades the same record");
        var reopened = new MatchRepository(directory);
        var restored = reopened.Get(first.Id)!;
        Check(restored.SavedAtUtc == savedAt && restored.Players.Count == 3, "Reopening preserves original save time and all players");
        Check(restored.Players.Select(p => p.Nickname).SequenceEqual(linked.Association!.Players.Select(p => p.DisplayName)), "Resolved identities persist without spelling changes");
        Check(restored.Players.Select(p => p.Damage).SequenceEqual(new int?[] { 1323, 1971, 371 }), "Damage survives reopening");
        Check(restored.Players[1].IsLocalPlayer == true && restored.Players[0].IsLocalPlayer == false, "Local-player attribution survives reopening");
        Check(File.ReadAllBytes(reopened.GetImagePath(restored.ScreenshotFile)).SequenceEqual(evidence.Bytes.ToArray()), "Stored evidence is an independent exact copy");
        Check(restored.RosterScreenshotFile is not null && File.Exists(reopened.GetImagePath(restored.RosterScreenshotFile)), "Roster evidence is retained");
        var downgrade = reopened.Save(plain);
        Check(!downgrade.Updated && reopened.Get(first.Id)!.Players.All(p => p.NameConfirmed), "Unassociated save does not erase resolved names");

        var unknownMatch = match with
        {
            Players = match.Players.Select((p, i) => i == 0
                ? p with { Damage = p.Damage with { Value = null }, Kills = p.Kills with { Value = 0 } } : p).ToArray()
        };
        var copy = Path.Combine(directory, "temporary-source.jpg");
        File.WriteAllBytes(copy, File.ReadAllBytes(Path.Combine(Path.GetDirectoryName(rosterPath)!, "03-游戏结束结算界面-主要用小队排名跟小队总击杀进行定位.jpg")));
        var temporaryEvidence = ImageEvidence.Capture(copy);
        File.Delete(copy); // Only this test-created copy is deleted, never a supplied sample.
        var unknown = reopened.Save(new(unknownMatch, temporaryEvidence, null, null, DateTimeOffset.UtcNow));
        var unknownRecord = new MatchRepository(directory).Get(unknown.Id)!;
        Check(unknownRecord.Players[0].Kills == 0 && unknownRecord.Players[0].Damage is null, "Zero and unknown remain distinct");
        Check(File.Exists(reopened.GetImagePath(unknownRecord.ScreenshotFile)), "Original source may disappear before saving a captured draft");
        ExpectFailure(() => reopened.GetImagePath("../../outside.jpg"), "Evidence paths cannot escape the data directory");

        var rollbackStore = new MatchRepository(NewDirectory("rollback"));
        var original = rollbackStore.Save(plain);
        using (var connection = new SqliteConnection($"Data Source={rollbackStore.DatabasePath};Pooling=False"))
        {
            connection.Open();
            using var command = connection.CreateCommand();
            command.CommandText = "CREATE TRIGGER fail_player BEFORE INSERT ON players WHEN NEW.slot=1 BEGIN SELECT RAISE(ABORT,'injected test failure'); END";
            command.ExecuteNonQuery();
        }
        ExpectFailure(() => rollbackStore.Save(linked), "Mid-save failure is reported");
        var rolledBack = rollbackStore.Get(original.Id)!;
        Check(rolledBack.Players.Count == 3 && rolledBack.Players.All(p => !p.NameConfirmed) && rollbackStore.List().Count == 1,
            "A failed update rolls back deleted/inserted player rows");

        var imageFailureStore = new MatchRepository(NewDirectory("image-failure"));
        var intact = imageFailureStore.Save(plain);
        var intactRecord = imageFailureStore.Get(intact.Id)!;
        File.WriteAllText(imageFailureStore.GetImagePath(intactRecord.ScreenshotFile), "corrupted test-owned evidence");
        ExpectFailure(() => imageFailureStore.GetVerifiedImagePath(intactRecord.ScreenshotFile), "History refuses altered evidence");
        ExpectFailure(() => imageFailureStore.Save(linked), "Corrupt stored evidence is reported");
        Check(imageFailureStore.Get(intact.Id)!.Players.All(p => !p.NameConfirmed), "Image write/check failure leaves the prior record untouched");

        Parallel.For(0, 4, _ => new MatchRepository(directory).Save(linked));
        Check(new MatchRepository(directory).List().Count == 2, "Concurrent duplicate saves do not append matches");
        var firstRun = NewDirectory("concurrent-first-save");
        Parallel.For(0, 2, _ => new MatchRepository(firstRun).Save(linked));
        Check(new MatchRepository(firstRun).List().Count == 1, "Concurrent first saves initialize one database atomically");
        var future = new MatchRepository(NewDirectory("future-version"));
        future.Save(plain);
        using (var connection = new SqliteConnection($"Data Source={future.DatabasePath};Pooling=False"))
        {
            connection.Open();
            using var command = connection.CreateCommand();
            command.CommandText = "PRAGMA user_version=99";
            command.ExecuteNonQuery();
        }
        var databaseBytes = File.ReadAllBytes(future.DatabasePath);
        ExpectFailure(() => future.List(), "Unknown database version is not read as empty history");
        ExpectFailure(() => future.Save(linked), "Unknown database version is not overwritten");
        Check(databaseBytes.SequenceEqual(File.ReadAllBytes(future.DatabasePath)), "Unknown-version database remains intact");
        Console.WriteLine("STORAGE PASS: persistence, identity upgrade, deduplication, null/zero, evidence copies, rollback, corrupt image, concurrent saves.");
    }

    public static string NewDirectory(string name)
    {
        var directory = Path.Combine(Environment.CurrentDirectory, ".local", "storage-tests", name + "-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(directory);
        return directory;
    }

    private static void Check(bool condition, string message)
    { if (!condition) throw new InvalidOperationException("FAILED: " + message); }
    private static void ExpectFailure(Action action, string message)
    {
        try { action(); }
        catch (Exception e) when (e is InvalidDataException or SqliteException or IOException) { return; }
        throw new InvalidOperationException("FAILED: " + message);
    }
}
