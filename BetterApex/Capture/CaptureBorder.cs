using System.Runtime.InteropServices;
using Windows.Foundation;
using Windows.Foundation.Metadata;
using Windows.Graphics.Capture;
using Windows.Security.Authorization.AppCapabilityAccess;

namespace BetterApex.Capture;

internal sealed record BorderAccess(bool Allowed, string Message);

internal static class CaptureBorder
{
    // Windows SDK 26100 windows.graphics.capture.h, UniversalApiContract v12.
    // Keep the existing 19041 target: the interfaces are queried only when the
    // runtime advertises them, so older Windows can still capture normally.
    private static readonly Guid Session3 = new("f2cdd966-22ae-5ea1-9596-3a289344c3be");
    private static readonly Guid AccessStatics = new("743ed370-06ec-5040-a58a-901f0f757095");
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int RequestAccess(nint self, int kind, out nint operation);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetBorder(nint self, out byte required);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int PutBorder(nint self, byte required);

    public static bool IsSupported => ApiInformation.IsPropertyPresent(
        "Windows.Graphics.Capture.GraphicsCaptureSession", "IsBorderRequired");

    public static async Task<BorderAccess> RequestAsync()
    {
        if (!IsSupported) return new(false, "当前 Windows 不支持隐藏采集边框；常规 Windows 10 会保留黄框。");
        nint factory = 0, operation = 0;
        try
        {
            factory = CaptureInterop.GetActivationFactory("Windows.Graphics.Capture.GraphicsCaptureAccess", AccessStatics);
            var request = Method<RequestAccess>(factory, 6); // IInspectable + RequestAccessAsync
            Marshal.ThrowExceptionForHR(request(factory, 0, out operation)); // Borderless = 0
            var pending = WinRT.MarshalInterface<IAsyncOperation<AppCapabilityAccessStatus>>.FromAbi(operation);
            var status = await pending;
            return status == AppCapabilityAccessStatus.Allowed
                ? new(true, "系统已允许隐藏采集边框")
                : new(false, "Windows 未允许隐藏采集边框，本次保留黄框。");
        }
        catch (Exception exception) when (exception is COMException or UnauthorizedAccessException or InvalidCastException)
        {
            System.Diagnostics.Trace.TraceError(exception.ToString());
            return new(false, $"系统未能开启隐藏边框，本次保留黄框（0x{exception.HResult:X8}）。");
        }
        finally
        {
            if (operation != 0) Marshal.Release(operation);
            if (factory != 0) Marshal.Release(factory);
        }
    }

    public static bool TrySet(GraphicsCaptureSession session, bool required)
    {
        if (!IsSupported) return false;
        nint pointer = 0;
        try
        {
            var original = ((WinRT.IWinRTObject)session).NativeObject;
            Marshal.ThrowExceptionForHR(Marshal.QueryInterface(original.ThisPtr, in Session3, out pointer));
            Marshal.ThrowExceptionForHR(Method<PutBorder>(pointer, 7)(pointer, required ? (byte)1 : (byte)0));
            Marshal.ThrowExceptionForHR(Method<GetBorder>(pointer, 6)(pointer, out var actual));
            return (actual != 0) == required;
        }
        catch (Exception exception) when (exception is COMException or InvalidCastException or UnauthorizedAccessException)
        { System.Diagnostics.Trace.TraceError(exception.ToString()); return false; }
        finally { if (pointer != 0) Marshal.Release(pointer); }
    }

    private static T Method<T>(nint instance, int slot) where T : Delegate
        => Marshal.GetDelegateForFunctionPointer<T>(Marshal.ReadIntPtr(Marshal.ReadIntPtr(instance), slot * IntPtr.Size));
}
