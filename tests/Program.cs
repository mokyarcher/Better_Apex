using System.Security.Cryptography;
using System.Text.Json;
using System.Windows;
using System.Windows.Media.Imaging;
using BetterApex.Ocr;
using BetterApex.Storage;

internal static class Program
{
    private sealed record ExpectedMatch(int Rank, int SquadKills, int[][] Players);

    private static readonly Dictionary<string, ExpectedMatch> Samples = new(StringComparer.OrdinalIgnoreCase)
    {
        ["1.jpg"] = new(8, 4, [[1, 0, 1, 380], [3, 0, 3, 480], [0, 0, 0, 60]]),
        ["2.jpg"] = new(5, 11, [[4, 2, 4, 1323], [7, 3, 10, 1971], [0, 2, 1, 371]]),
        ["3.jpg"] = new(2, 19, [[5, 3, 7, 1520], [10, 5, 10, 1938], [4, 1, 4, 1351]]),
        ["结算图-01.jpg"] = new(1, 35, [[11, 9, 14, 3317], [15, 6, 20, 4062], [9, 9, 13, 3110]])
    };

    [STAThread]
    private static int Main(string[] args)
    {
        if (args.Length == 0)
            throw new ArgumentException("Pass a screenshot path or the sample directory.");
        if (args.Contains("--dxgi"))
        {
            DxgiCaptureVerification.Run(args[0]);
            return 0;
        }
        if (args.Contains("--auto-live"))
        {
            AutomationLiveVerification.Run(args[0]);
            return 0;
        }
        if (args.Contains("--automation"))
        {
            AutomationVerification.Run(args[0]);
            return 0;
        }
        if (args.Contains("--detect-auto"))
        {
            ScreenshotOcr.RunAsync(() =>
            {
                using var detector = new BetterApex.Capture.AutoPageDetector();
                foreach (var path in System.IO.Directory.GetFiles(args[0], "*.jpg"))
                {
                    Console.WriteLine(System.IO.Path.GetFileName(path) + ": " + JsonSerializer.Serialize(detector.Observe(ScreenshotOcr.LoadOriginal(path), true)));
                    Console.WriteLine(detector.MarkerDiagnostic);
                }
                return true;
            }).GetAwaiter().GetResult();
            return 0;
        }
        if (args.Contains("--capture"))
        {
            CaptureVerification.Run(args[0]);
            return 0;
        }
        if (args.Contains("--render-capture"))
        {
            RenderPreview.Capture(args[0], args[1]);
            return 0;
        }
        if (args.Contains("--render"))
        {
            RenderPreview.Run(args[0], args[1]);
            return 0;
        }
        if (args.Contains("--diagnose-roster"))
        {
            Console.WriteLine(JsonSerializer.Serialize(RosterOcrService.RecognizeAsync(args[0]).GetAwaiter().GetResult()));
            return 0;
        }
        if (args.Contains("--roster"))
        {
            VerifyRoster(args[0]);
            return 0;
        }
        var paths = System.IO.Directory.Exists(args[0])
            ? System.IO.Directory.GetFiles(args[0], "*.jpg").Order().ToArray() : [args[0]];
        Check(paths.Length > 0, "At least one real screenshot is present");
        foreach (var path in paths)
        {
            var filename = System.IO.Path.GetFileName(path);
            if (filename.Equals("5-id.jpg", StringComparison.OrdinalIgnoreCase))
            {
                VerifyRoster(path);
                continue;
            }
            if (filename.Equals("4-id.jpg", StringComparison.OrdinalIgnoreCase))
            {
                Console.WriteLine("4-id.jpg: HUD fixture for 2.jpg pairing; exact I/l spelling remains unverified.");
                continue;
            }
            if (!Samples.ContainsKey(filename) && !args.Contains("--diagnose"))
            {
                Console.WriteLine($"SKIP {filename}: no manually verified settlement baseline.");
                continue;
            }
            var before = SHA256.HashData(System.IO.File.ReadAllBytes(path));
            var result = MatchOcrService.RecognizeAsync(path).GetAwaiter().GetResult();
            if (args.Contains("--diagnose"))
            {
                Console.WriteLine(JsonSerializer.Serialize(result, new JsonSerializerOptions { WriteIndented = true }));
                continue;
            }
            if (!Samples.TryGetValue(filename, out var expected))
                throw new ArgumentException($"No manually verified baseline for {filename}.");
            Check(result.Rank.Value == expected.Rank && result.SquadKills.Value == expected.SquadKills, $"{filename}: squad summary");
            for (var i = 0; i < 3; i++)
            {
                var player = result.Players[i];
                int?[] actual = [player.Kills.Value, player.Assists.Value, player.Knocks.Value, player.Damage.Value];
                Check(actual.SequenceEqual(expected.Players[i].Select(v => (int?)v)), $"{filename}: player {i + 1} core fields");
                Console.WriteLine($"{filename}, player {i + 1}: {string.Join('/', actual.Take(3))}; damage={player.Damage.Value}");
            }
            using var stream = System.IO.File.OpenRead(path);
            var source = BitmapFrame.Create(stream, BitmapCreateOptions.None, BitmapCacheOption.OnLoad);
            var row = new Int32Rect((int)Math.Round(.118 * source.PixelWidth), (int)Math.Round(.375 * source.PixelHeight),
                (int)Math.Round(.066 * source.PixelWidth), (int)Math.Round(.038 * source.PixelHeight));
            var groups = KdaImageSplitter.TrySplit(source, row);
            Check(groups is not null, $"{filename}: two visible separators are located");
            var secondGroup = groups![1].Bounds;
            var missingSlash = new Int32Rect(row.X, row.Y, secondGroup.X + secondGroup.Width - row.X, row.Height);
            Check(KdaImageSplitter.TrySplit(source, missingSlash) is null, $"{filename}: missing separator is not guessed");
            Check(before.SequenceEqual(SHA256.HashData(System.IO.File.ReadAllBytes(path))), $"{filename}: source image unchanged");
            if (filename.Equals("2.jpg", StringComparison.OrdinalIgnoreCase))
            {
                var rosterPath = System.IO.Path.Combine(System.IO.Path.GetDirectoryName(path)!, "4-id.jpg");
                if (System.IO.File.Exists(rosterPath)) VerifyAssociation(rosterPath, path, result);
            }
        }
        Check(OcrValueParser.Number("4，062", 1).Value == 4062, "Thousands separator");
        Check(OcrValueParser.Number("", 1).Value is null, "Missing does not become zero");
        Check(OcrValueParser.Number("0", 1).Value == 0, "A recognized zero is retained");
        Check(OcrValueParser.Number("4,D62", 1).Value is null, "Invalid number is not guessed");
        Check(OcrValueParser.Kda("15/6/", 1).All(f => f.Value is null), "Incomplete KDA is rejected");
        if (!args.Contains("--diagnose"))
            Console.WriteLine("PASS: available labeled screenshots; settlement KDA/damage, rank, squad kills, roster nicknames, separator guard, async workers, parser, source integrity.");
        return 0;
    }

    private static void Check(bool condition, string name)
    {
        if (!condition)
            throw new InvalidOperationException($"FAILED: {name}");
    }

    private static void VerifyRoster(string path, string? settlementPath = null)
    {
        var before = SHA256.HashData(System.IO.File.ReadAllBytes(path));
        var result = RosterOcrService.RecognizeAsync(path).GetAwaiter().GetResult();
        if (System.IO.Path.GetFileName(path).Equals("5-id.jpg", StringComparison.OrdinalIgnoreCase))
        {
            Check(result.Members.Select(m => m.Nickname).SequenceEqual(new[] { "摸摸我的狗吧", "流川枫", "Archer" }),
                "5-id: exact HUD names without portrait border slash or voice icon");
            Check(result.Members[1].NeedsReview, "Filtered voice icon keeps review status");
        }
        else
            Check(result.Members.All(m => !string.IsNullOrWhiteSpace(m.Nickname)), "Paired HUD supplies three candidates; exact spelling is tested only on labeled HUDs");
        Check(before.SequenceEqual(SHA256.HashData(System.IO.File.ReadAllBytes(path))), "Roster source image unchanged");
        Console.WriteLine($"{System.IO.Path.GetFileName(path)}: {string.Join(" / ", result.Members.Select(m => m.Nickname))}");

        // Constructor/selection smoke check: catches initialization event-order
        // errors in the new mode selector without opening or controlling a window.
        // A plain test Application avoids App.xaml's StartupUri opening a second
        // production window when this test pumps the dispatcher for async work.
        var app = System.Windows.Application.Current ?? new System.Windows.Application();
        app.ShutdownMode = ShutdownMode.OnExplicitShutdown;
        app.Resources["TextPrimary"] = System.Windows.Media.Brushes.White;
        app.Resources["TextSecondary"] = System.Windows.Media.Brushes.Gray;
        System.Diagnostics.Trace.Listeners.Add(new System.Diagnostics.TextWriterTraceListener(Console.Out));
        var repository = new MatchRepository(StorageVerification.NewDirectory("ui"));
        var window = new BetterApex.MainWindow(repository);
        Check(!((System.Windows.Controls.Button)window.FindName("SaveMatchButton")).IsEnabled, "No save before recognition");
        var rosterMode = (System.Windows.Controls.RadioButton)window.FindName("RosterMode");
        rosterMode.IsChecked = true;
        Check(((System.Windows.Controls.Button)window.FindName("OcrButton")).Content.ToString() == "读取昵称", "Roster mode can be selected after initialization");
        ((System.Windows.Controls.RadioButton)window.FindName("SummaryMode")).IsChecked = true;
        Check(((System.Windows.Controls.Button)window.FindName("OcrButton")).Content.ToString() == "识别战绩", "Settlement mode remains available");
        var associationButton = (System.Windows.Controls.Button)window.FindName("AssociateButton");
        Check(!associationButton.IsEnabled, "No cached roster at startup");
        rosterMode.IsChecked = true;
        RunUiTask(() => window.LoadScreenshotAsync(path));
        RunUiTask(() => window.RecognizeCurrentAsync(associate: false));
        Check(associationButton.IsEnabled, $"Reading a roster enables explicit same-round pairing; mode={rosterMode.IsChecked}; error={((System.Windows.Controls.TextBlock)window.FindName("ErrorText")).Text}");
        var clear = (System.Windows.Controls.Button)window.FindName("ClearButton");
        clear.RaiseEvent(new RoutedEventArgs(System.Windows.Controls.Button.ClickEvent));
        Check(associationButton.IsEnabled, "Clearing the preview preserves the visibly cached roster");
        var expectedSavedCount = 0;
        if (settlementPath is not null)
        {
            RunUiTask(() => window.AssociateSettlementAsync(settlementPath));
            var linked = ((System.Windows.Controls.ItemsControl)window.FindName("PlayerResults")).ItemsSource.Cast<AssociatedPlayer>().ToArray();
            Check(linked.Length == 3 && linked.All(p => p.IsLinked), "UI pairs the actual selected settlement with its cached roster");
            SaveMatchResult? saved = null;
            RunUiTask(async () => saved = await window.SaveCurrentAsync());
            Check(saved is not null && repository.List().Count == 1, "UI saves the recognized linked match");
            RunUiTask(async () => await window.SaveCurrentAsync());
            Check(repository.List().Count == 1, "UI repeat save does not duplicate a match");
            var history = new BetterApex.HistoryWindow(new MatchRepository(repository.DataDirectory));
            RunUiTask(() => history.RefreshAsync(saved!.Id));
            Check(((System.Windows.Controls.DataGrid)history.FindName("SavedPlayers")).Items.Count == 3,
                "History window reloads the three stored players");
            Check(((System.Windows.Controls.Image)history.FindName("SavedScreenshotPreview")).Source is not null,
                "History reloads its saved screenshot");
            history.Close();
            expectedSavedCount = 1;
        }
        ((System.Windows.Controls.Button)window.FindName("NewRoundButton")).RaiseEvent(new RoutedEventArgs(System.Windows.Controls.Button.ClickEvent));
        Check(!associationButton.IsEnabled && ((System.Windows.Controls.Image)window.FindName("ScreenshotPreview")).Source is null,
            "New round clears roster and preview");
        Check(!((System.Windows.Controls.Button)window.FindName("SaveMatchButton")).IsEnabled && repository.List().Count == expectedSavedCount,
            "Clearing a round removes the draft but preserves saved history");
        window.Close();
    }

    private static void RunUiTask(Func<Task> work)
    {
        var dispatcher = System.Windows.Threading.Dispatcher.CurrentDispatcher;
        var previous = SynchronizationContext.Current;
        SynchronizationContext.SetSynchronizationContext(new System.Windows.Threading.DispatcherSynchronizationContext(dispatcher));
        try
        {
            var task = work();
            if (!task.IsCompleted)
            {
                var frame = new System.Windows.Threading.DispatcherFrame();
                _ = task.ContinueWith(_ => dispatcher.BeginInvoke(new Action(() => frame.Continue = false)), TaskScheduler.Default);
                System.Windows.Threading.Dispatcher.PushFrame(frame);
            }
            task.GetAwaiter().GetResult();
        }
        finally { SynchronizationContext.SetSynchronizationContext(previous); }
    }

    private static void VerifyAssociation(string rosterPath, string settlementPath, MatchRecognition settlement)
    {
        var observedRoster = RosterOcrService.RecognizeAsync(rosterPath).GetAwaiter().GetResult();
        var result = RosterMatcher.Match(observedRoster, settlement);
        Check(result.MatchedCount == 3, "All three real players link despite different HUD/summary order");
        Check(result.Players.Select(p => p.Member).SequenceEqual(new[]
        {
            observedRoster.Members[1], observedRoster.Members[2], observedRoster.Members[0]
        }), "4-id/2: each actual HUD slot links to its manually verified settlement card");
        Check(result.Players.Select(p => p.Result.Damage.Value).SequenceEqual(new int?[] { 1323, 1971, 371 }), "Pairing preserves source damage");
        Check(result.Players.Select(p => p.Member).Distinct().Count() == 3, "One-to-one mapping");

        VerifyRoster(rosterPath, settlementPath);
        StorageVerification.Run(rosterPath, settlementPath, settlement, observedRoster);

        // Synthetic identity scenarios are independent of files the user removed.
        // They test matching rules; they are not claimed as real screenshot OCR.
        var roster = new RosterRecognition([
            new("队友 1（上方）", "我上早八", false), new("队友 2（下方）", "焚梦", false), new("本人", "Archer", false)
        ]);

        MatchRecognition Named(params string[] names) => settlement with
        {
            Players = names.Select((name, i) => settlement.Players[i] with { Name = name, NameCandidates = [] }).ToArray()
        };
        Check(RosterMatcher.Match(roster, Named("我上早八", "焚梦", "Archer")).Players.Select(p => p.DisplayName)
            .SequenceEqual(new[] { "我上早八", "焚梦", "Archer" }), "Pairing does not depend on card order");
        Check(RosterMatcher.Match(roster, Named("焚梦", "Arche", "我上早八")).MatchedCount == 3, "Unique partial nickname is matched");
        var hidden = RosterMatcher.Match(roster, Named("", "Archer", "我上早八"));
        Check(hidden.MatchedCount == 2 && hidden.Players[0].Member is null, "Fully hidden nickname is not assigned by elimination");
        Check(RosterMatcher.Match(roster, Named("StrangerOne", "Archer", "StrangerTwo")).MatchedCount == 0, "Own name alone does not confirm a round");
        var duplicate = new RosterRecognition([new("队友 1", "Archer", false), new("队友 2", "Archer", false), new("本人", "我上早八", false)]);
        Check(RosterMatcher.Match(duplicate, Named("Archer", "", "我上早八")).MatchedCount == 0, "Duplicate nickname ambiguity is not forced");
        Console.WriteLine("PAIR PASS: real 4-id/2 pairing and storage; synthetic shuffled, partial/hidden/duplicate names, and wrong squad.");
    }
}
