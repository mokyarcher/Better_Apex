namespace BetterApex.Capture;

internal interface IWindowCapture : IAsyncDisposable
{
    WindowTarget Target { get; }
    CapturedFrame? Latest { get; }
    string? Error { get; }
    string? PauseReason { get; }
    bool TargetClosed { get; }
    bool IsMinimized { get; }
    bool IsStopping { get; }
    bool CanAnalyze { get; }
    void RequestVerificationFrame();
}
