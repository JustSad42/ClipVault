using System.Runtime.InteropServices;

namespace ClipVault;

static class Native
{
    public const int WM_HOTKEY = 0x0312;
    public const int WM_CLIPBOARDUPDATE = 0x031D;
    public const uint MOD_ALT = 0x1, MOD_CONTROL = 0x2, MOD_SHIFT = 0x4, MOD_WIN = 0x8, MOD_NOREPEAT = 0x4000;
    public const uint ASFW_ANY = 0xFFFFFFFF;

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool RegisterHotKey(IntPtr hWnd, int id, uint fsModifiers, uint vk);

    [DllImport("user32.dll")]
    public static extern bool UnregisterHotKey(IntPtr hWnd, int id);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool AddClipboardFormatListener(IntPtr hwnd);

    [DllImport("user32.dll")]
    public static extern bool RemoveClipboardFormatListener(IntPtr hwnd);

    [DllImport("user32.dll")]
    public static extern uint GetClipboardSequenceNumber();

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool AllowSetForegroundWindow(uint dwProcessId);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    [DllImport("kernel32.dll")]
    static extern uint GetCurrentThreadId();

    [DllImport("user32.dll")]
    static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);

    [DllImport("user32.dll")]
    static extern bool BringWindowToTop(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool GetCursorPos(out POINT p);

    [DllImport("user32.dll")]
    public static extern IntPtr MonitorFromPoint(POINT pt, uint flags);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern bool GetMonitorInfo(IntPtr hMonitor, ref MONITORINFO mi);

    [DllImport("user32.dll")]
    static extern bool GetGUIThreadInfo(uint idThread, ref GUITHREADINFO info);

    [DllImport("user32.dll")]
    static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);

    [DllImport("oleacc.dll")]
    static extern int AccessibleObjectFromWindow(IntPtr hwnd, uint id, ref Guid iid,
        [MarshalAs(UnmanagedType.IUnknown)] out object obj);

    [DllImport("dwmapi.dll")]
    static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);

    [DllImport("user32.dll", SetLastError = true)]
    static extern uint SendInput(uint nInputs, INPUT[] inputs, int cbSize);

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X, Y; }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    public struct MONITORINFO
    {
        public int cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct GUITHREADINFO
    {
        public int cbSize;
        public uint flags;
        public IntPtr hwndActive, hwndFocus, hwndCapture, hwndMenuOwner, hwndMoveSize, hwndCaret;
        public RECT rcCaret;
    }

    /// <summary>Just enough of the accessibility interface to ask an app where its text cursor is.</summary>
    [ComImport, Guid("618736E0-3C3D-11CF-810C-00AA00389B71"), InterfaceType(ComInterfaceType.InterfaceIsDual)]
    interface IAccessible
    {
        // Placeholders for the 15 members that come before accLocation (never called).
        void _1(); void _2(); void _3(); void _4(); void _5(); void _6(); void _7(); void _8();
        void _9(); void _10(); void _11(); void _12(); void _13(); void _14(); void _15();
        void accLocation(out int left, out int top, out int width, out int height,
            [MarshalAs(UnmanagedType.Struct)] object childId);
    }

    const uint OBJID_CARET = 0xFFFFFFF8;

    /// <summary>
    /// Finds the screen position of the text cursor in another app's window.
    /// Tries the classic Windows caret first, then the accessibility caret (used by Chrome, Edge, Office…).
    /// </summary>
    public static bool TryGetCaretRect(IntPtr target, out RECT rect)
    {
        rect = default;
        if (target == IntPtr.Zero) return false;
        uint thread = GetWindowThreadProcessId(target, out _);
        var info = new GUITHREADINFO { cbSize = Marshal.SizeOf<GUITHREADINFO>() };
        if (!GetGUIThreadInfo(thread, ref info)) return false;

        if (info.hwndCaret != IntPtr.Zero && info.rcCaret.Bottom > info.rcCaret.Top)
        {
            var tl = new POINT { X = info.rcCaret.Left, Y = info.rcCaret.Top };
            var br = new POINT { X = info.rcCaret.Right, Y = info.rcCaret.Bottom };
            ClientToScreen(info.hwndCaret, ref tl);
            ClientToScreen(info.hwndCaret, ref br);
            rect = new RECT { Left = tl.X, Top = tl.Y, Right = br.X, Bottom = br.Y };
            return true;
        }

        var focus = info.hwndFocus != IntPtr.Zero ? info.hwndFocus : target;
        try
        {
            var iid = typeof(IAccessible).GUID;
            if (AccessibleObjectFromWindow(focus, OBJID_CARET, ref iid, out var obj) == 0 && obj is IAccessible acc)
            {
                acc.accLocation(out int l, out int t, out int w, out int h, 0);
                if ((l != 0 || t != 0) && h > 0)
                {
                    rect = new RECT { Left = l, Top = t, Right = l + w, Bottom = t + h };
                    return true;
                }
            }
        }
        catch { /* app doesn't support it */ }
        return false;
    }

    /// <summary>
    /// Brings our window to the front even when Windows' focus-stealing protection would block it,
    /// by briefly sharing input state with the current foreground window's thread.
    /// </summary>
    public static void ForceForeground(IntPtr hwnd)
    {
        var fg = GetForegroundWindow();
        uint fgThread = GetWindowThreadProcessId(fg, out _);
        uint myThread = GetCurrentThreadId();
        if (fg != IntPtr.Zero && fgThread != myThread && AttachThreadInput(myThread, fgThread, true))
        {
            BringWindowToTop(hwnd);
            SetForegroundWindow(hwnd);
            AttachThreadInput(myThread, fgThread, false);
        }
        else
        {
            SetForegroundWindow(hwnd);
        }
    }

    public static void SetDarkTitleBar(IntPtr hwnd, bool dark) => SetDwm(hwnd, 20 /* DWMWA_USE_IMMERSIVE_DARK_MODE */, dark ? 1 : 0);

    public static void SetDwm(IntPtr hwnd, int attribute, int value) =>
        DwmSetWindowAttribute(hwnd, attribute, ref value, sizeof(int));

    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();

    [DllImport("psapi.dll", EntryPoint = "EmptyWorkingSet")]
    static extern bool EmptyWorkingSet(IntPtr process);

    /// <summary>Hands unused memory pages back to Windows (they're reloaded if needed again).</summary>
    public static void TrimWorkingSet() => EmptyWorkingSet(GetCurrentProcess());

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
    static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int index);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
    static extern IntPtr SetWindowLongPtr(IntPtr hWnd, int index, IntPtr value);

    const int GWL_EXSTYLE = -20;
    public const long WS_EX_TRANSPARENT = 0x20, WS_EX_TOOLWINDOW = 0x80, WS_EX_NOACTIVATE = 0x08000000;

    public static void AddExStyle(IntPtr hwnd, long flags) =>
        SetWindowLongPtr(hwnd, GWL_EXSTYLE, (IntPtr)((long)GetWindowLongPtr(hwnd, GWL_EXSTYLE) | flags));

    [StructLayout(LayoutKind.Sequential)]
    struct INPUT { public uint type; public InputUnion u; }

    [StructLayout(LayoutKind.Explicit)]
    struct InputUnion
    {
        [FieldOffset(0)] public MOUSEINPUT mi;
        [FieldOffset(0)] public KEYBDINPUT ki;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }

    [StructLayout(LayoutKind.Sequential)]
    struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr dwExtraInfo; }

    const uint INPUT_KEYBOARD = 1, KEYEVENTF_KEYUP = 0x2;
    const ushort VK_SHIFT = 0x10, VK_CONTROL = 0x11, VK_MENU = 0x12, VK_V = 0x56, VK_LWIN = 0x5B, VK_RWIN = 0x5C;

    static INPUT Key(ushort vk, bool up) => new()
    {
        type = INPUT_KEYBOARD,
        u = new InputUnion { ki = new KEYBDINPUT { wVk = vk, dwFlags = up ? KEYEVENTF_KEYUP : 0 } }
    };

    /// <summary>Simulates Ctrl+V in the foreground window. Releases Shift/Alt/Win first so only Ctrl+V is sent.</summary>
    public static void SendCtrlV()
    {
        var inputs = new[]
        {
            Key(VK_SHIFT, true), Key(VK_MENU, true), Key(VK_LWIN, true), Key(VK_RWIN, true),
            Key(VK_CONTROL, false), Key(VK_V, false), Key(VK_V, true), Key(VK_CONTROL, true),
        };
        SendInput((uint)inputs.Length, inputs, Marshal.SizeOf<INPUT>());
    }
}
