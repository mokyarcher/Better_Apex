using System.IO;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using BetterApex.Capture;
using BetterApex.Ocr;
using BetterApex.Storage;

internal static class AutomationVerification
{
    public static void Run(string sampleDirectory)
    {
        ScreenshotOcr.RunAsync(() =>
        {
            using var detector = new AutoPageDetector();
            var images = Directory.GetFiles(sampleDirectory, "*.jpg").ToDictionary(path => Path.GetFileName(path)!, ScreenshotOcr.LoadOriginal);
            foreach (var pair in images)
            {
                var observed = detector.Observe(pair.Value, true);
                var expected = pair.Key is "4-id.jpg" or "5-id.jpg" ? AutoPage.InMatch : AutoPage.Settlement;
                Check(observed.Page == expected, pair.Key + " page classification");
                Console.WriteLine($"DETECT {pair.Key}: {observed.Page}, token={observed.Token}, squads={observed.Squads}");
            }
            var hud = images["4-id.jpg"];
            var summary = images["2.jpg"];
            var hudObservation = detector.Observe(hud, false);
            var summaryObservation = detector.Observe(summary, false);
            Check(summaryObservation.Token is not null && (hudObservation.Token is null || hudObservation.Token == summaryObservation.Token),
                "Readable markers agree; uncertain HUD marker remains unknown");
            var nextObservation = detector.Observe(images["5-id.jpg"], false);
            Check(nextObservation.Token is not null && nextObservation.Token != summaryObservation.Token,
                "5-id visible marker supplies distinct new-round evidence");

            var store = new MatchRepository(StorageVerification.NewDirectory("automation"));
            var rosterReads = 0;
            var engine = new AutoRecorder(store, detector.Observe, image => { rosterReads++; return RosterOcrService.Recognize(image); });
            long sequence = 0;
            CapturedFrame Frame(BitmapSource image) => new(image, DateTimeOffset.UtcNow.AddSeconds(++sequence), sequence,
                TimeSpan.FromSeconds(sequence), new WindowGeometry(new Int32Rect(0, 0, image.PixelWidth, image.PixelHeight),
                    new Int32Rect(0, 0, image.PixelWidth, image.PixelHeight)), false);
            void Feed(BitmapSource image, int count) { for (var i = 0; i < count; i++) engine.Process(Frame(image)); }
            Feed(hud, 4);
            Check(rosterReads == 2, "Roster freezes after two agreeing actual HUD frames");
            Feed(hud, 4);
            Check(rosterReads == 2, "No repeated nickname OCR after roster ready");
            Feed(summary, 2);
            Check(store.List().Count == 1, "Actual HUD -> summary automatically saves one match");
            var saved = store.Get(store.List()[0].Id)!;
            Check(saved.Players.Select(p => p.Damage).SequenceEqual(new int?[] { 1323, 1971, 371 })
                && saved.Players.Count(p => p.NameConfirmed) == 3, "Actual roster associates with correct damage and survives storage");
            Feed(summary, 5);
            Feed(hud, 4); // Returning to spectate the same match is not new.
            Feed(summary, 3);
            Check(store.List().Count == 1, "Repeated summary and same-match spectating do not duplicate");
            Feed(images["5-id.jpg"], 5);
            Check(engine.Status.Roster.Contains("摸摸我的狗吧") && !engine.Status.Roster.Contains("smallhundred"),
                "New stable marker clears old roster and collects current roster");
            Feed(summary, 2);
            Check(store.List().Count == 1, "Old summary is not associated with the next squad");

            // A genuine second timeline uses deterministic page observations;
            // this is state-machine testing, not claimed as a second real pair.
            var secondMode = AutoPage.InMatch;
            var synthetic = new AutoRecorder(store, (_, _) => new(secondMode, "bbbbbbbb", 20));
            for (var i = 0; i < 4; i++) synthetic.Process(Frame(images["5-id.jpg"]));
            secondMode = AutoPage.Settlement;
            for (var i = 0; i < 2; i++) synthetic.Process(Frame(images["3.jpg"]));
            Check(store.List().Count == 2, "A distinct admitted round creates a second record");
            var second = store.Get(store.List()[0].Id)!;
            Check(second.Players.All(p => !p.NameConfirmed), "Unrelated names are not forced into three-player association");
            var before = store.List().Count;
            var restarted = new AutoRecorder(store, detector.Observe);
            restarted.Process(Frame(summary)); restarted.Process(Frame(summary));
            Check(store.List().Count == before, "Starting/restarting on summary never appends an unobserved round");

            VerifyRecovery(summary, hud, detector, Frame);
            Console.WriteLine("AUTOMATION PASS: real page detection, HUD -> freeze -> summary -> associate -> save; duplicate lock, round isolation and persistent retry.");
            return true;
        }).GetAwaiter().GetResult();
    }

    private static void VerifyRecovery(BitmapSource summary, BitmapSource hud, AutoPageDetector detector,
        Func<BitmapSource, CapturedFrame> frame)
    {
        var store = new MatchRepository(StorageVerification.NewDirectory("auto-recovery"));
        var journal = new AutoJournal(store.DataDirectory);
        var job = new AutoJob(Guid.NewGuid().ToString("N"), "211347a4", DateTimeOffset.UtcNow,
            RosterOcrService.Recognize(hud), MatchOcrService.Recognize(summary));
        job = journal.Capture(job, frame(summary), frame(hud));
        var checkpoint = Path.Combine(store.DataDirectory, "automation-state.json");
        Directory.CreateDirectory(checkpoint); // Fail after the database commit.
        try { journal.Save(job, store); throw new InvalidOperationException("Expected checkpoint failure"); }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException) { }
        Check(store.List().Count == 1 && journal.Pending().Count == 1, "Failed checkpoint retains a recoverable committed job");
        Directory.Delete(checkpoint);
        var recovered = new AutoRecorder(store, detector.Observe);
        recovered.RecoverPending();
        Check(store.List().Count == 1 && journal.Pending().Count == 0, "Recovery retries same session id without duplicate");
        var lockedJob = journal.Capture(job with { Id = Guid.NewGuid().ToString("N") }, frame(summary), frame(hud));
        using (File.Open(Path.Combine(journal.JobDirectory(lockedJob.Id), "job.json"), FileMode.Open, FileAccess.Read, FileShare.Read))
        {
            journal.Save(lockedJob, store);
            Check(store.List().Count == 1 && journal.Pending().Count == 1,
                "Cleanup file lock does not fail committed save or duplicate record");
        }
        recovered.RecoverPending();
        Check(store.List().Count == 1 && journal.Pending().Count == 0, "Cleanup retry remains idempotent after file unlock");
        job = journal.Capture(job with { Id = Guid.NewGuid().ToString("N"), ReviewOnly = true }, frame(summary), frame(hud));
        recovered.RecoverPending();
        Check(store.List().Count == 1 && journal.Pending().Count == 1, "Unstable key fields remain review-only after restart");
        File.Copy(Path.Combine(journal.JobDirectory(job.Id), "roster.png"), Path.Combine(journal.JobDirectory(job.Id), "settlement.png"), true);
        try { journal.VerifyEvidence(job); throw new InvalidOperationException("Expected altered evidence rejection"); }
        catch (InvalidDataException) { }
    }

    private static void Check(bool success, string description)
    { if (!success) throw new InvalidOperationException("FAILED: " + description); }
}
