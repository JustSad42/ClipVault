using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace ClipVault;

/// <summary>Works out which app put something on the clipboard, and looks up its name and icon.</summary>
static class SourceApp
{
    [DllImport("user32.dll")] static extern IntPtr GetClipboardOwner();
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern bool QueryFullProcessImageName(IntPtr h, uint flags, StringBuilder name, ref uint size);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr SHGetFileInfo(string path, uint attrs, ref SHFILEINFO info, uint size, uint flags);
    [DllImport("user32.dll")] static extern bool DestroyIcon(IntPtr h);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct SHFILEINFO
    {
        public IntPtr hIcon;
        public int iIcon;
        public uint dwAttributes;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szDisplayName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 80)] public string szTypeName;
    }

    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000, SHGFI_ICON = 0x100, SHGFI_SMALLICON = 0x1;

    static readonly Dictionary<string, ImageSource?> IconCache = new(StringComparer.OrdinalIgnoreCase);
    static readonly Dictionary<string, string> NameCache = new(StringComparer.OrdinalIgnoreCase);

    public static (string? Name, string? Path) Detect()
    {
        var path = PathOf(GetClipboardOwner()) ?? PathOf(Native.GetForegroundWindow());
        return path == null ? (null, null) : (NameOf(path), path);
    }

    static string? PathOf(IntPtr hwnd)
    {
        if (hwnd == IntPtr.Zero) return null;
        Native.GetWindowThreadProcessId(hwnd, out var pid);
        if (pid == 0 || pid == (uint)Environment.ProcessId) return null;
        var h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (h == IntPtr.Zero) return null;
        try
        {
            var sb = new StringBuilder(1024);
            uint size = (uint)sb.Capacity;
            return QueryFullProcessImageName(h, 0, sb, ref size) ? sb.ToString() : null;
        }
        finally { CloseHandle(h); }
    }

    /// <summary>Full path of the exe that owns a window.</summary>
    public static string? ExeOf(IntPtr hwnd) => PathOf(hwnd);

    static readonly Dictionary<string, ImageSource?> FileIconCache = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>The 32px icon Windows shows for a file or folder.</summary>
    public static ImageSource? FileIcon(string path)
    {
        if (FileIconCache.TryGetValue(path, out var cached)) return cached;
        ImageSource? img = null;
        try
        {
            var info = new SHFILEINFO();
            if (SHGetFileInfo(path, 0, ref info, (uint)Marshal.SizeOf<SHFILEINFO>(), SHGFI_ICON) != IntPtr.Zero && info.hIcon != IntPtr.Zero)
            {
                img = Imaging.CreateBitmapSourceFromHIcon(info.hIcon, Int32Rect.Empty, BitmapSizeOptions.FromEmptyOptions());
                img.Freeze();
                DestroyIcon(info.hIcon);
            }
        }
        catch { }
        return FileIconCache[path] = img;
    }

    public static string NameOf(string path)
    {
        if (NameCache.TryGetValue(path, out var cached)) return cached;
        string name = Path.GetFileNameWithoutExtension(path);
        try
        {
            var desc = FileVersionInfo.GetVersionInfo(path).FileDescription?.Trim();
            if (!string.IsNullOrEmpty(desc) && desc.Length <= 40) name = desc;
        }
        catch { }
        return NameCache[path] = name;
    }

    public static ImageSource? IconOf(string? path)
    {
        if (string.IsNullOrEmpty(path)) return null;
        if (IconCache.TryGetValue(path, out var cached)) return cached;
        ImageSource? icon = null;
        var info = new SHFILEINFO();
        if (SHGetFileInfo(path, 0, ref info, (uint)Marshal.SizeOf<SHFILEINFO>(), SHGFI_ICON | SHGFI_SMALLICON) != IntPtr.Zero
            && info.hIcon != IntPtr.Zero)
        {
            try
            {
                var src = Imaging.CreateBitmapSourceFromHIcon(info.hIcon, Int32Rect.Empty, BitmapSizeOptions.FromEmptyOptions());
                src.Freeze();
                icon = src;
            }
            catch { }
            finally { DestroyIcon(info.hIcon); }
        }
        return IconCache[path] = icon;
    }
}
