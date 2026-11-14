using System.Windows.Interop;

namespace ClipVault;

/// <summary>Invisible message-only window that receives the global hotkey and clipboard-change notifications.</summary>
sealed class MessageWindow : IDisposable
{
    const int HotkeyId = 1;
    readonly HwndSource _source;

    public event Action? HotkeyPressed;
    public event Action? ClipboardChanged;

    public MessageWindow()
    {
        var p = new HwndSourceParameters("ClipVaultMessages")
        {
            ParentWindow = new IntPtr(-3), // HWND_MESSAGE
            WindowStyle = 0,
        };
        _source = new HwndSource(p);
        _source.AddHook(WndProc);
        Native.AddClipboardFormatListener(_source.Handle);
    }

    public bool RegisterHotkey(uint modifiers, uint vk)
    {
        UnregisterHotkey();
        return Native.RegisterHotKey(_source.Handle, HotkeyId, modifiers | Native.MOD_NOREPEAT, vk);
    }

    public void UnregisterHotkey() => Native.UnregisterHotKey(_source.Handle, HotkeyId);

    IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (msg == Native.WM_HOTKEY && wParam.ToInt32() == HotkeyId)
        {
            HotkeyPressed?.Invoke();
            handled = true;
        }
        else if (msg == Native.WM_CLIPBOARDUPDATE)
        {
            ClipboardChanged?.Invoke();
            handled = true;
        }
        return IntPtr.Zero;
    }

    public void Dispose()
    {
        Native.UnregisterHotKey(_source.Handle, HotkeyId);
        Native.RemoveClipboardFormatListener(_source.Handle);
        _source.Dispose();
    }
}
