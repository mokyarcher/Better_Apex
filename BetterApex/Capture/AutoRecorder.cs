using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Windows.Media.Imaging;
using BetterApex.Ocr;
using BetterApex.Storage;

namespace BetterApex.Capture;

internal sealed record AutoRecordStatus(string Phase, string Roster = "本场名单：未采集", string LastSaved = "", int SavedCount = 0, string Error = "");

internal sealed class AutoRecorder
{
    private readonly MatchRepository _repository;
    private readonly AutoJournal _journal;
    private readonly Func<BitmapSource, bool, PageObservation> _observe;
    private readonly Func<BitmapSource, RosterRecognition> _readRoster;
    private readonly Func<BitmapSource, MatchRecognition> _readMatch;
    private readonly Action? _requestVerification;
    private volatile AutoRecordStatus _status = new("等待进入对局");
    private string? _sessionId, _sessionToken, _completedToken, _lastToken;
    private bool _locked, _introSeen;
    private int _hudVotes, _tokenVotes, _introVotes, _rosterAttempts;
    private long _lastSequence = -1;
    private int _resetRequested;
    private TimeSpan? _lastObservation;
    private TimeSpan _nextRoster, _rosterDeadline;
    private long _lastRecoveryTick;
    private DateTimeOffset _lastCompletedAt;
    private RosterRecognition? _roster;
    private CapturedFrame? _rosterFrame;
    private readonly List<(RosterRecognition Roster, CapturedFrame Frame)> _rosterVotes = [];
    private readonly List<CapturedFrame> _settlement = [];

    public AutoRecordStatus Status => _status;

    internal AutoRecorder(MatchRepository repository, Func<BitmapSource, bool, PageObservation> observe,
        Func<BitmapSource, RosterRecognition>? roster = null, Func<BitmapSource, MatchRecognition>? match = null,
        Action? requestVerification = null)
    {
        _repository = repository;
        _journal = new(repository.DataDirectory);
        _observe = observe;
        _readRoster = roster ?? RosterOcrService.Recognize;
        _readMatch = match ?? MatchOcrService.Recognize;
        _requestVerification = requestVerification;
        if (_journal.ReadCheckpoint() is { } checkpoint)
        { _locked = true; _completedToken = checkpoint.Token; _lastCompletedAt = checkpoint.CapturedAt; }
    }

    public static Task RunAsync(MatchRepository repository, Func<CapturedFrame?> latest,
        Action<AutoRecorder> started, CancellationToken cancellation, Action? requestVerification = null)
        => ScreenshotOcr.RunAsync(() =>
        {
            using var detector = new AutoPageDetector();
            var recorder = new AutoRecorder(repository, detector.Observe, requestVerification: requestVerification);
            var key = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(repository.DataDirectory.ToUpperInvariant())));
            using var mutex = new Mutex(false, @"Local\BetterApexAuto-" + key);
            var owns = false;
            try
            {
                try { owns = mutex.WaitOne(0); } catch (AbandonedMutexException) { owns = true; }
                if (!owns) throw new InvalidOperationException("此数据目录已有一个自动记录正在运行，请先停止另一个窗口。");
                started(recorder);
                recorder.RecoverPending();
                recorder._lastRecoveryTick = Stopwatch.GetTimestamp();
                var idle = Stopwatch.StartNew();
                while (!cancellation.IsCancellationRequested)
                {
                    try
                    {
                        var frame = latest();
                        if (frame is not null && !frame.IsBlack && Stopwatch.GetElapsedTime(frame.ReceivedTimestamp) < TimeSpan.FromSeconds(3))
                        { recorder.Process(frame); idle.Restart(); }
                        else if (idle.Elapsed > TimeSpan.FromSeconds(20))
                        { recorder.ForgetLiveRound(); idle.Restart(); }
                        if (Stopwatch.GetElapsedTime(recorder._lastRecoveryTick) >= TimeSpan.FromSeconds(10))
                        { recorder.RecoverPending(); recorder._lastRecoveryTick = Stopwatch.GetTimestamp(); }
                    }
                    catch (Exception exception)
                    {
                        Trace.TraceError(exception.ToString());
                        recorder._status = recorder.Status with { Error = "自动记录暂未完成：" + exception.Message };
                        cancellation.WaitHandle.WaitOne(1000);
                    }
                    cancellation.WaitHandle.WaitOne(100);
                }
                return true;
            }
            finally { if (owns) mutex.ReleaseMutex(); }
        }, lowPriority: true);

    internal void ForgetLiveRound()
    {
        _sessionId = null; _sessionToken = null; _roster = null; _rosterFrame = null;
        _rosterVotes.Clear(); _settlement.Clear(); _hudVotes = 0; _tokenVotes = 0;
        _status = Status with { Phase = "等待有效局内画面，恢复后重新采集名单", Roster = "本场名单：未采集" };
    }

    internal void Process(CapturedFrame frame)
    {
        if (Interlocked.Exchange(ref _resetRequested, 0) != 0) ResetForNextRound();
        if (frame.IsBlack || frame.Sequence == _lastSequence) return;
        _lastSequence = frame.Sequence;
        if (_lastObservation is { } previous && frame.SourceTime - previous < TimeSpan.FromMilliseconds(400)) return;
        _lastObservation = frame.SourceTime;
        var observation = _observe(frame.Image, _locked);
        if (observation.NewRoundIntro)
        {
            if (++_introVotes >= 2) _introSeen = true;
        }
        else _introVotes = 0;
        if (observation.Page == AutoPage.InMatch)
        {
            _settlement.Clear();
            if (observation.Token is { } token)
            {
                _tokenVotes = token == _lastToken ? _tokenVotes + 1 : 1;
                _lastToken = token;
            }
            else { _lastToken = null; _tokenVotes = 0; }
            if (_sessionId is not null && _sessionToken is not null && observation.Token is not null
                && observation.Token != _sessionToken && _tokenVotes >= 3)
                ForgetLiveRound();
            // If a previously unreadable marker first becomes reliable after a
            // roster was frozen, reacquire identity rather than assume continuity.
            if (_sessionId is not null && _sessionToken is null && _roster?.Members.Any(m => m.Nickname.Length > 0) == true
                && observation.Token is not null && _tokenVotes >= 2)
                ForgetLiveRound();
            if (_locked)
            {
                if (!_introSeen && !(observation.Token is not null && _completedToken is not null
                    && observation.Token != _completedToken && _tokenVotes >= 3))
                {
                    _status = Status with { Phase = "本局已处理，等待下一局（未确认时可点“新一局”）" };
                    return;
                }
                _locked = false;
                _hudVotes = 0;
            }
            if (_sessionId is null)
            {
                if (++_hudVotes < 2) { _status = Status with { Phase = "发现局内画面，正在确认…" }; return; }
                _sessionId = Guid.NewGuid().ToString("N");
                _sessionToken = _tokenVotes >= 2 ? observation.Token : null;
                _roster = null; _rosterFrame = null; _rosterVotes.Clear(); _rosterAttempts = 0;
                _rosterDeadline = frame.SourceTime.Add(TimeSpan.FromSeconds(20)); _nextRoster = frame.SourceTime;
                _introSeen = false;
            }
            if (_sessionToken is null && _tokenVotes >= 2) _sessionToken = observation.Token;
            if ((_roster is null || _roster.Members.Any(m => m.Nickname.Length == 0))
                && _rosterAttempts < 6 && frame.SourceTime <= _rosterDeadline && frame.SourceTime >= _nextRoster)
                CollectRoster(frame);
            else _status = Status with { Phase = "对局进行中，等待结算" };
            return;
        }
        _hudVotes = 0;
        if (observation.Page != AutoPage.Settlement) { _settlement.Clear(); return; }
        if (_sessionId is null || _locked)
        {
            _status = Status with { Phase = _locked ? "本局已处理，等待下一局" : "已看到结算，但未观察到本局入局；可手动识别保存" };
            return;
        }
        if (_sessionToken is not null && observation.Token is not null && _sessionToken != observation.Token)
        {
            _status = Status with { Phase = "结算与局内标记不一致，请核对当前画面", Error = "未使用旧名单关联不同对局。" };
            _settlement.Clear(); return;
        }
        _settlement.Add(frame);
        if (_settlement.Count > 3) _settlement.RemoveAt(0);
        _sessionToken ??= observation.Token;
        if (_settlement.Count < 2)
        {
            _requestVerification?.Invoke();
            var candidate = new AutoJob(_sessionId, _sessionToken, frame.CapturedAt, _roster, ReviewOnly: true);
            _journal.Capture(candidate, frame, _rosterFrame);
            _status = Status with { Phase = "发现结算，已保留截图，正在确认…" };
            return;
        }
        var official = _settlement[^1];
        var comparison = _settlement[^2];
        var job = new AutoJob(_sessionId, _sessionToken ?? observation.Token, official.CapturedAt, _roster);
        job = _journal.Capture(job, official, _rosterFrame);
        _locked = true; _completedToken = job.Token; _sessionId = null;
        _settlement.Clear();
        _status = Status with { Phase = "结算已捕获，正在解析并保存…", Error = "" };
        var result = _readMatch(official.Image);
        var corroboration = _readMatch(comparison.Image);
        result = Corroborate(result, corroboration);
        if (result.Rank.Value is not (>= 1 and <= 20) || result.SquadKills.Value is null)
        {
            _journal.WriteJob(job with { ReviewOnly = true });
            throw new InvalidDataException("结算排名或总击杀未能稳定读取，截图已保留为待保存任务。");
        }
        job = job with { Recognition = result };
        _journal.WriteJob(job);
        SaveJob(job);
    }

    private void CollectRoster(CapturedFrame frame)
    {
        _rosterAttempts++;
        _nextRoster = frame.SourceTime.Add(TimeSpan.FromSeconds(_rosterAttempts < 3 ? .6 : 3));
        _status = Status with { Phase = "正在采集本场昵称…" };
        RosterRecognition reading;
        try { reading = _readRoster(frame.Image); }
        catch (InvalidDataException) { return; }
        _rosterVotes.Add((reading, frame));
        if (_rosterVotes.Count > 3) _rosterVotes.RemoveAt(0);
        var members = new List<RosterMember>();
        for (var slot = 0; slot < 3; slot++)
        {
            var stable = _rosterVotes.Select(v => v.Roster.Members[slot]).Where(m => m.Nickname.Length > 0)
                .GroupBy(m => m.Nickname, StringComparer.Ordinal).Where(g => g.Count() >= 2)
                .OrderByDescending(g => g.Count()).FirstOrDefault();
            members.Add(_roster?.Members[slot] is { Nickname.Length: > 0 } frozen ? frozen
                : stable?.First() ?? new RosterMember(reading.Members[slot].Position, "", true));
        }
        if (members.Where(m => m.Nickname.Length > 0).GroupBy(m => m.Nickname).Any(g => g.Count() > 1)) return;
        _roster = new(members);
        _rosterFrame = _rosterVotes.OrderByDescending(v => v.Roster.Members.Zip(members)
            .Count(pair => pair.First.Nickname == pair.Second.Nickname && pair.Second.Nickname.Length > 0)).First().Frame;
        _status = Status with
        {
            Phase = "对局进行中，等待结算",
            Roster = "本场名单：" + string.Join(" / ", members.Select(m => m.Nickname.Length == 0 ? "待核对" : m.Nickname))
        };
        if (members.All(m => m.Nickname.Length > 0)) _rosterVotes.Clear();
    }

    internal void ResetForNextRound()
    {
        ForgetLiveRound(); _locked = false; _introSeen = false; _lastToken = null;
        _status = Status with { Phase = "已清空本场名单，等待局内画面" };
    }

    public void RequestNextRound() => Interlocked.Exchange(ref _resetRequested, 1);

    internal void RecoverPending()
    {
        foreach (var pending in _journal.Pending())
        {
            try
            {
                var job = pending;
                if (job.ReviewOnly) throw new InvalidDataException("有结算截图需要人工核对，已保留在 data/pending。");
                _journal.VerifyEvidence(job);
                if (job.Recognition is null)
                {
                    var image = ScreenshotOcr.LoadOriginal(Path.Combine(_journal.JobDirectory(job.Id), "settlement.png"));
                    var observation = _observe(image, false);
                    if (observation.Page != AutoPage.Settlement) throw new InvalidDataException("待保存截图不是可确认的结算页。");
                    var recognition = _readMatch(image);
                    if (recognition.Rank.Value is not (>= 1 and <= 20) || recognition.SquadKills.Value is null)
                        throw new InvalidDataException("待保存截图仍有关键字段缺失，请保留 pending 目录中的原图核对。");
                    job = job with { Recognition = recognition };
                    _journal.WriteJob(job);
                }
                SaveJob(job);
            }
            catch (Exception exception)
            { _status = Status with { Error = "有战绩待保存：" + exception.Message }; }
        }
    }

    private void SaveJob(AutoJob job)
    {
        var saved = _journal.Save(job, _repository);
        if (_sessionId is null && job.CapturedAt >= _lastCompletedAt)
        { _completedToken = job.Token; _locked = true; _lastCompletedAt = job.CapturedAt; }
        _status = Status with { Phase = _sessionId is null ? "战绩已保存，等待下一局" : "对局进行中，上一场战绩已保存", LastSaved = "最近保存：" + MatchSummary.DisplayTime(job.CapturedAt).ToString("HH:mm:ss")
            + (saved.Created ? " · 新增一场" : " · 已存在，未重复新增"), SavedCount = Status.SavedCount + (saved.Created ? 1 : 0), Error = "" };
    }

    private static MatchRecognition Corroborate(MatchRecognition chosen, MatchRecognition other)
    {
        RecognizedField Check(RecognizedField field, RecognizedField comparison)
            => comparison.Value is not null && field.Value != comparison.Value ? field with { Value = null } : field;
        return chosen with
        {
            Rank = Check(chosen.Rank, other.Rank), SquadKills = Check(chosen.SquadKills, other.SquadKills),
            Players = chosen.Players.Select((p, i) => p with
            {
                Kills = Check(p.Kills, other.Players[i].Kills), Assists = Check(p.Assists, other.Players[i].Assists),
                Knocks = Check(p.Knocks, other.Players[i].Knocks), Damage = Check(p.Damage, other.Players[i].Damage)
            }).ToArray()
        };
    }
}
