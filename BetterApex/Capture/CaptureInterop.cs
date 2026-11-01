using System.Runtime.InteropServices;
using Windows.Graphics.Capture;
using Windows.Graphics.DirectX.Direct3D11;

namespace BetterApex.Capture;

internal static class CaptureInterop
{
    [ComImport, Guid("3628E81B-3CAC-4C60-B7F4-23CE0E0C3356"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface ICaptureItemInterop
    {
        [PreserveSig] int CreateForWindow(nint window, in Guid iid, out nint item);
        [PreserveSig] int CreateForMonitor(nint monitor, in Guid iid, out nint item);
    }

    [DllImport("combase.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern int WindowsCreateString(string value, uint length, out nint text);
    [DllImport("combase.dll", ExactSpelling = true)] private static extern int WindowsDeleteString(nint text);
    [DllImport("combase.dll", ExactSpelling = true)]
    private static extern int RoGetActivationFactory(nint className, in Guid iid, out nint factory);
    [DllImport("d3d11.dll", ExactSpelling = true)]
    private static extern int D3D11CreateDevice(nint adapter, int driverType, nint software, uint flags,
        nint levels, uint levelCount, uint sdkVersion, out nint device, out uint level, out nint context);
    [DllImport("d3d11.dll", ExactSpelling = true)]
    private static extern int CreateDirect3D11DeviceFromDXGIDevice(nint dxgiDevice, out nint graphicsDevice);

    public static GraphicsCaptureItem CreateItem(nint window)
    {
        const string className = "Windows.Graphics.Capture.GraphicsCaptureItem";
        nint text = 0, factory = 0, item = 0;
        ICaptureItemInterop? interop = null;
        try
        {
            Marshal.ThrowExceptionForHR(WindowsCreateString(className, (uint)className.Length, out text));
            Marshal.ThrowExceptionForHR(RoGetActivationFactory(text, typeof(ICaptureItemInterop).GUID, out factory));
            interop = (ICaptureItemInterop)Marshal.GetObjectForIUnknown(factory);
            var iid = new Guid("79C3F95B-31F7-4EC2-A464-632EF5D30760");
            Marshal.ThrowExceptionForHR(interop.CreateForWindow(window, iid, out item));
            return WinRT.MarshalInterface<GraphicsCaptureItem>.FromAbi(item);
        }
        finally
        {
            if (item != 0) Marshal.Release(item);
            if (interop is not null) Marshal.ReleaseComObject(interop);
            if (factory != 0) Marshal.Release(factory);
            if (text != 0) WindowsDeleteString(text);
        }
    }

    internal static nint GetActivationFactory(string className, Guid iid)
    {
        nint text = 0;
        try
        {
            Marshal.ThrowExceptionForHR(WindowsCreateString(className, (uint)className.Length, out text));
            Marshal.ThrowExceptionForHR(RoGetActivationFactory(text, iid, out var factory));
            return factory; // Caller owns this reference.
        }
        finally { if (text != 0) WindowsDeleteString(text); }
    }

    public static IDirect3DDevice CreateDevice()
    {
        nint native = 0, context = 0, dxgi = 0, wrapped = 0;
        try
        {
            // Hardware BGRA device; no game-process access or injected hooks.
            Marshal.ThrowExceptionForHR(D3D11CreateDevice(0, 1, 0, 0x20, 0, 0, 7, out native, out _, out context));
            var iid = new Guid("54EC77FA-1377-44E6-8C32-88FD5F44C84C");
            Marshal.ThrowExceptionForHR(Marshal.QueryInterface(native, in iid, out dxgi));
            Marshal.ThrowExceptionForHR(CreateDirect3D11DeviceFromDXGIDevice(dxgi, out wrapped));
            return WinRT.MarshalInterface<IDirect3DDevice>.FromAbi(wrapped);
        }
        finally
        {
            if (wrapped != 0) Marshal.Release(wrapped);
            if (dxgi != 0) Marshal.Release(dxgi);
            if (context != 0) Marshal.Release(context);
            if (native != 0) Marshal.Release(native);
        }
    }
}
