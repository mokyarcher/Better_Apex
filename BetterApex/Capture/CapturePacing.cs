using System.Diagnostics;

namespace BetterApex.Capture;

internal sealed class CapturePacing
{
    private static readonly long NormalIntervalTicks = Stopwatch.Frequency * 2;
    private readonly object _sync = new();
    private long _next, _lastAttempt, _lastVerification;
    private long? _verification;
    public bool TryBegin(long now)
    {
        lock (_sync)
        {
            var due = _verification is { } extra ? Math.Min(_next, extra) : _next;
            if (now < due) return false;
            _lastAttempt = now;
            _next = now + NormalIntervalTicks; // At most one normal attempt every two seconds.
            _verification = null;
            return true;
        }
    }

    public void RequestVerification(long now)
    {
        lock (_sync)
        {
            if (_lastVerification != 0 && now - _lastVerification < Stopwatch.Frequency * 5) return;
            _lastVerification = now;
            _verification = Math.Max(now, _lastAttempt + Stopwatch.Frequency / 2);
        }
    }

    public void CompleteAttempt(long now)
    {
        lock (_sync) _next = Math.Max(_next, now + NormalIntervalTicks);
    }
}
