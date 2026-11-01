using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows;

namespace BetterApex.Capture;

internal sealed record WindowTarget(nint Handle, int ProcessId, string Title, string ProcessName)
{
    public string DisplayLabel => $"{Title}  ·  {ProcessName}";
}

internal static class WindowCatalog
{
    public static IReadOnlyList<WindowTarget> List()
    {
        var targets = new List<WindowTarget>();
        NativeWindow.EnumWindows((window, _) =>
        {
            if (NativeWindow.IsWindowVisible(window) && NativeWindow.GetWindow(window, 4) == 0
                && Describe(window) is { } target && target.ProcessId != Environment.ProcessId)
                targets.Add(target);
            return true;
        }, 0);
        return targets.OrderByDescending(t => t.ProcessName.StartsWith("r5apex", StringComparison.OrdinalIgnoreCase))
            .ThenBy(t => t.Title, StringComparer.CurrentCultureIgnoreCase).ToArray();
    }

    internal static WindowTarget? Describe(nint window)
    {
        var title = new StringBuilder(512);
        NativeWindow.GetWindowText(window, title, title.Capacity);
        if (title.Length == 0) return null;
        NativeWindow.GetWindowThreadProcessId(window, out var pid);
        try
        {
            using var process = Process.GetProcessById((int)pid);
            return new(window, (int)pid, title.ToString(), process.ProcessName);
        }
        catch (Exception e) when (e is ArgumentException or InvalidOperationException or System.ComponentModel.Win32Exception)
        { return null; }
    }
}

internal readonly record struct WindowGeometry(Int32Rect Window, Int32Rect Client)
{
    public Int32Rect? Crop(int frameWidth, int frameHeight)
    {
        // Reject mismatched resize/DPI frames rather than silently cropping the
        // wrong area. WGC window frames use the visible DWM window bounds.
        if (Math.Abs(Window.Width - frameWidth) > 2 || Math.Abs(Window.Height - frameHeight) > 2)
            return null;
        var x = Client.X - Window.X;
        var y = Client.Y - Window.Y;
        if (x < 0 || y < 0 || Client.Width <= 0 || Client.Height <= 0
            || x + Client.Width > frameWidth || y + Client.Height > frameHeight) return null;
        return new Int32Rect(x, y, Client.Width, Client.Height);
    }
}

internal static class NativeWindow
{
    internal delegate bool EnumWindowsCallback(nint window, nint parameter);
    [StructLayout(LayoutKind.Sequential)] internal struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] internal struct Point { public int X, Y; }
    [DllImport("user32.dll")] internal static extern bool EnumWindows(EnumWindowsCallback callback, nint parameter);
    [DllImport("user32.dll")] internal static extern bool IsWindow(nint window);
    [DllImport("user32.dll")] internal static extern bool IsWindowVisible(nint window);
    [DllImport("user32.dll")] internal static extern bool IsIconic(nint window);
    [DllImport("user32.dll")] internal static extern nint GetForegroundWindow();
    [DllImport("user32.dll")] internal static extern nint GetAncestor(nint window, uint flags);
    [DllImport("user32.dll")] internal static extern nint WindowFromPoint(Point point);
    [DllImport("user32.dll")] internal static extern nint MonitorFromWindow(nint window, uint flags);
    [DllImport("user32.dll")] internal static extern nint GetWindow(nint window, uint command);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] internal static extern int GetWindowText(nint window, StringBuilder text, int capacity);
    [DllImport("user32.dll")] internal static extern uint GetWindowThreadProcessId(nint window, out uint processId);
    [DllImport("user32.dll")] private static extern bool GetClientRect(nint window, out Rect rect);
    [DllImport("user32.dll")] private static extern bool ClientToScreen(nint window, ref Point point);
    [DllImport("dwmapi.dll")] private static extern int DwmGetWindowAttribute(nint window, int attribute, out Rect rect, int size);

    internal static bool IsSameWindow(WindowTarget target)
    {
        if (!IsWindow(target.Handle)) return false;
        GetWindowThreadProcessId(target.Handle, out var pid);
        return pid == target.ProcessId;
    }

    internal static WindowGeometry? Geometry(nint window)
    {
        if (!GetClientRect(window, out var client) || client.Right <= 0 || client.Bottom <= 0) return null;
        var origin = new Point();
        if (!ClientToScreen(window, ref origin)
            || DwmGetWindowAttribute(window, 9, out var bounds, Marshal.SizeOf<Rect>()) < 0) return null;
        if (bounds.Right <= bounds.Left || bounds.Bottom <= bounds.Top) return null;
        return new(new(bounds.Left, bounds.Top, bounds.Right - bounds.Left, bounds.Bottom - bounds.Top),
            new(origin.X, origin.Y, client.Right - client.Left, client.Bottom - client.Top));
    }

    internal static string? CapturePauseReason(WindowTarget target, out WindowGeometry geometry)
    {
        geometry = default;
        if (!IsSameWindow(target)) return "游戏窗口已关闭";
        if (IsIconic(target.Handle)) return "游戏已最小化，暂停截屏";
        if (GetAncestor(GetForegroundWindow(), 2) != target.Handle) return "请将游戏切回前台，当前暂停截屏";
        var current = Geometry(target.Handle);
        if (current is null) return "等待有效游戏窗口尺寸";
        geometry = current.Value;
        var client = geometry.Client;
        // Check both generic coverage and the areas used by our OCR. Occluding
        // windows must not supply another application's pixels to the detector.
        foreach (var point in new[] { (.02,.02), (.50,.50), (.98,.98), (.87,.06), (.82,.18),
            (.12,.738), (.12,.798), (.15,.904), (.15,.43), (.43,.43), (.71,.43) })
        {
            var sample = new Point { X = client.X + (int)(client.Width * point.Item1), Y = client.Y + (int)(client.Height * point.Item2) };
            if (GetAncestor(WindowFromPoint(sample), 2) != target.Handle) return "游戏画面被其他窗口遮挡，暂停截屏";
        }
        return null;
    }
}
