using System.Collections.Specialized;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Microsoft.Win32;

namespace ClipVault;

public partial class App : Application
{
    const string MutexName = @"Local\ClipVault.SingleInstance";
    const string ShowEventName = @"Local\ClipVault.Show";

    // Theme colors: (dark, light). Most are see-through so they sit nicely on the glass background.
    // Bg is solid (menus, tooltips, toasts). Surface is the window fill: tinted glass, or Bg without glass.
    static readonly (string Key, string Dark, string Light)[] Palette =
    [
        ("Bg", "#26272D", "#F9F9FB"),
        ("Panel", "#14FFFFFF", "#B3FFFFFF"),
        ("Line", "#1FFFFFFF", "#1A000000"),
        ("Fg", "#ECECF0", "#1B1B1F"),
        ("Muted", "#9A9CA6", "#62646E"),
        ("Hover", "#0FFFFFFF", "#0A000000"),
        ("Selected", "#1CFFFFFF", "#12000000"),
        ("Thumb", "#40FFFFFF", "#40000000"),
    ];
    static readonly (string Dark, string Light) GlassSurface = ("#A61C1D22", "#B8F5F5F7");
    static readonly (string Name, string Dark, string Light)[] TypeColors =
    [
        ("Text", "#A99BFF", "#6B5BD6"),
        ("Code", "#4FD1A5", "#0F8A66"),
        ("Link", "#6CB4FF", "#1F6FD1"),
        ("Color", "#F58BB8", "#C2386F"),
        ("Email", "#F59F7A", "#C0532A"),
        ("Image", "#F2B84B", "#A86A0A"),
        ("Files", "#A8A9B3", "#6A6C77"),
    ];
    static readonly (string Dark, string Light) DefaultAccent = ("#8AA2FF", "#3D5AFE");

    Mutex? _mutex;
    EventWaitHandle? _showEvent;
    MessageWindow? _messages;
    MainWindow? _popup;
    SettingsWindow? _settingsWindow;
    uint _ownClipboardSeq;
    string? _appliedTheme;
    bool _dark = true;

    public ClipStore Store { get; } = new();
    public AppSettings Settings { get; private set; } = new();
    public bool IsDark => _dark;

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // After an update, the new version waits for the old one to finish closing.
        int wait = Array.IndexOf(e.Args, "--wait-for");
        if (wait >= 0 && wait + 1 < e.Args.Length && int.TryParse(e.Args[wait + 1], out int oldPid))
        {
            try { System.Diagnostics.Process.GetProcessById(oldPid).WaitForExit(15000); } catch { /* already gone */ }
        }

        _mutex = new Mutex(true, MutexName, out bool firstInstance);
        if (!firstInstance)
        {
            // Already running: ask the running copy to show its window, then exit.
            Native.AllowSetForegroundWindow(Native.ASFW_ANY);
            try { EventWaitHandle.OpenExisting(ShowEventName).Set(); } catch { }
            _mutex = null;
            Shutdown();
            return;
        }

        _showEvent = new EventWaitHandle(false, EventResetMode.AutoReset, ShowEventName);
        new Thread(() =>
        {
            while (_showEvent.WaitOne())
                Dispatcher.BeginInvoke(() => _popup?.ShowCentered());
        }) { IsBackground = true }.Start();

        Updater.CleanUp();
        Settings = AppSettings.Load();
        ApplyTheme();
        Store.MaxUnpinned = Settings.MaxHistory;
        Store.Load();
        Store.RunOcr(); // scan any images that haven't been read yet
        _popup = new MainWindow(this);

        _messages = new MessageWindow();
        _messages.ClipboardChanged += OnClipboardChanged;
        _messages.HotkeyPressed += () => _popup.Toggle(Native.GetForegroundWindow());

        if (!RegisterHotkey())
        {
            MessageBox.Show(
                $"{Settings.HotkeyText} is already used by another app, so the shortcut won't work.\n\n" +
                "ClipVault will still record your clipboard. Launch ClipVault.exe again to open it, " +
                "then pick a different shortcut in Settings (the ⋯ button).",
                "ClipVault", MessageBoxButton.OK, MessageBoxImage.Warning);
        }

        if (e.Args.Contains("--updated"))
        {
            var area = SystemParameters.WorkArea;
            Dispatcher.BeginInvoke(() => Toast.Show($"ClipVault updated to {Updater.CurrentText}", area),
                System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        }

        // Look for updates shortly after starting, then every 6 hours.
        var updateTimer = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(30) };
        updateTimer.Tick += async (_, _) =>
        {
            updateTimer.Interval = TimeSpan.FromHours(6);
            if (!Settings.AutoUpdate) return;
            await Updater.CheckAsync();
            if (Updater.Ready != null && !_popup.IsVisible) Updater.ApplyAndRestart();
        };
        updateTimer.Start();

        if (!e.Args.Contains("--background"))
            _popup.ShowCentered();
        else
        {
            // Started with Windows: trim once loading and text recognition have settled.
            var startupTrim = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(20) };
            startupTrim.Tick += (_, _) => { startupTrim.Stop(); if (!_popup.IsVisible) TrimMemory(); };
            startupTrim.Start();
        }
    }

    protected override void OnExit(ExitEventArgs e)
    {
        // Only the running copy owns the history. A second launch just signals and exits,
        // and must not overwrite the file with its own empty list.
        if (_mutex != null)
        {
            Store.SaveNow();
            Store.PurgeDeleted();
            _messages?.Dispose();
            _mutex.ReleaseMutex();
        }
        base.OnExit(e);
    }

    // ---------- memory ----------

    /// <summary>
    /// Called a few seconds after the popup closes: drops cached thumbnails, compacts memory
    /// and returns unused pages to Windows, so ClipVault stays light while it waits in the background.
    /// </summary>
    public static void TrimMemory()
    {
        ThumbCache.Clear();
        System.Runtime.GCSettings.LargeObjectHeapCompactionMode = System.Runtime.GCLargeObjectHeapCompactionMode.CompactOnce;
        GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, blocking: true, compacting: true);
        GC.WaitForPendingFinalizers();
        Native.TrimWorkingSet();
    }

    // ---------- hotkey ----------

    public bool RegisterHotkey() => _messages?.RegisterHotkey(Settings.HotkeyModifiers, Settings.HotkeyKey) ?? false;
    public bool TryRegisterHotkey(uint mods, uint vk) => _messages?.RegisterHotkey(mods, vk) ?? false;
    public void SuspendHotkey() => _messages?.UnregisterHotkey();

    // ---------- theme ----------

    /// <summary>Applies the chosen theme. "Follow Windows" is re-checked every time the popup opens.</summary>
    public void ApplyTheme()
    {
        bool dark = Settings.Theme switch
        {
            AppTheme.Dark => true,
            AppTheme.Light => false,
            _ => SystemIsDark(),
        };
        var accent = (Settings.UseSystemAccent ? SystemAccent(dark) : null) ?? Hex(dark ? DefaultAccent.Dark : DefaultAccent.Light);
        var key = $"{dark}{accent}{Settings.UseSystemAccent}";
        if (_appliedTheme == key) return;
        _appliedTheme = key;
        _dark = dark;

        foreach (var (name, d, l) in Palette) SetBrush(name, Hex(dark ? d : l));
        SetBrush("Surface", Backdrop.GlassSupported ? Hex(dark ? GlassSurface.Dark : GlassSurface.Light) : Hex(dark ? Palette[0].Dark : Palette[0].Light));
        SetBrush("Accent", accent);
        SetBrush("AccentSoft", Color.FromArgb(dark ? (byte)0x40 : (byte)0x2E, accent.R, accent.G, accent.B));
        SetBrush("Mark", Color.FromArgb(dark ? (byte)0x5C : (byte)0x45, accent.R, accent.G, accent.B));
        // Dark text on light accents (Windows does the same), white text on dark ones.
        double luminance = (0.299 * accent.R + 0.587 * accent.G + 0.114 * accent.B) / 255;
        SetBrush("OnAccent", luminance > 0.6 ? Hex("#111114") : Colors.White);

        // One color per kind of item, for the row stripe and type tag.
        foreach (var (name, d, l) in TypeColors)
        {
            var c = Hex(dark ? d : l);
            SetBrush("Type" + name, c);
            SetBrush("Type" + name + "Soft", Color.FromArgb(dark ? (byte)0x30 : (byte)0x24, c.R, c.G, c.B));
        }

        foreach (Window w in Windows) Backdrop.Apply(w); // dark/light glass and title bars
    }

    void SetBrush(string key, Color c)
    {
        var brush = new SolidColorBrush(c);
        brush.Freeze();
        Resources[key] = brush;
    }

    static Color Hex(string s) => (Color)ColorConverter.ConvertFromString(s);

    /// <summary>The accent color picked in Windows Settings, in the shade Windows uses for this theme.</summary>
    static Color? SystemAccent(bool dark)
    {
        try
        {
            var c = new global::Windows.UI.ViewManagement.UISettings().GetColorValue(dark
                ? global::Windows.UI.ViewManagement.UIColorType.AccentLight2
                : global::Windows.UI.ViewManagement.UIColorType.AccentDark1);
            return Color.FromRgb(c.R, c.G, c.B);
        }
        catch { return null; }
    }

    static bool SystemIsDark()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
            return key?.GetValue("AppsUseLightTheme") is not int v || v == 0;
        }
        catch { return true; }
    }

    public void ShowSettings()
    {
        if (_settingsWindow != null) { _settingsWindow.Activate(); return; }
        _settingsWindow = new SettingsWindow(this);
        _settingsWindow.Closed += (_, _) => _settingsWindow = null;
        _settingsWindow.Show();
        Native.ForceForeground(new System.Windows.Interop.WindowInteropHelper(_settingsWindow).Handle);
        _settingsWindow.Activate();
    }

    /// <summary>Swaps in already-saved settings that don't affect theme or hotkey (popup size, position).</summary>
    public void ReplaceSettingsQuietly(AppSettings s) => Settings = s;

    public void SaveSettings(AppSettings s)
    {
        Settings = s;
        s.Save();
        Store.MaxUnpinned = s.MaxHistory;
        Store.Trim();
        _appliedTheme = null; // force re-apply
        ApplyTheme();
    }

    // ---------- reading the clipboard ----------

    void OnClipboardChanged()
    {
        // Ignore the change we caused ourselves when pasting an item.
        if (Native.GetClipboardSequenceNumber() == _ownClipboardSeq) return;

        // Another app may still have the clipboard open, so retry a few times.
        for (int attempt = 0; attempt < 5; attempt++)
        {
            try { Capture(); return; }
            catch (ExternalException) { Thread.Sleep(40); }
            catch { return; } // unreadable format: skip it
        }
    }

    void Capture()
    {
        var source = SourceApp.Detect();

        if (Clipboard.ContainsFileDropList())
        {
            var files = Clipboard.GetFileDropList().Cast<string>().ToArray();
            if (files.Length > 0) { Store.AddFiles(files, source); Sounds.Copy(); }
            return;
        }
        if (Clipboard.ContainsText())
        {
            var text = Clipboard.GetText();
            if (string.IsNullOrWhiteSpace(text)) return;
            string? html = Clipboard.ContainsText(TextDataFormat.Html) ? Clipboard.GetText(TextDataFormat.Html) : null;
            string? rtf = Clipboard.ContainsText(TextDataFormat.Rtf) ? Clipboard.GetText(TextDataFormat.Rtf) : null;

            // A copied link with tracking junk: keep the clean version, and put it on the clipboard too.
            if (Settings.CleanLinks && LinkCleaner.Clean(text) is { } clean)
            {
                Store.AddText(clean, null, null, source, linkCleaned: true);
                Dispatcher.BeginInvoke(() => PutText(clean));
                Sounds.Copy();
                return;
            }

            Store.AddText(text, string.IsNullOrEmpty(html) ? null : html, string.IsNullOrEmpty(rtf) ? null : rtf, source);
            Sounds.Copy();
            return;
        }
        var image = ReadImage();
        if (image != null) { Store.AddImage(image, source); Sounds.Copy(); }
    }

    static BitmapSource? ReadImage()
    {
        // Prefer the PNG format (keeps transparency); fall back to the standard bitmap format.
        if (Clipboard.ContainsData("PNG") && Clipboard.GetData("PNG") is MemoryStream png)
        {
            var decoder = BitmapDecoder.Create(png, BitmapCreateOptions.PreservePixelFormat, BitmapCacheOption.OnLoad);
            return decoder.Frames[0];
        }
        return Clipboard.ContainsImage() ? Clipboard.GetImage() : null;
    }

    // ---------- writing the clipboard ----------

    /// <summary>Puts a history item back on the clipboard and moves it to the top.</summary>
    public void PutOnClipboard(ClipItem item, bool plain)
    {
        SetClipboard(() => Clipboard.SetDataObject(BuildData(item, plain, forDrag: false), true));
        item.CopiedAt = DateTime.Now;
        Store.ScheduleSave();
    }

    /// <summary>The clipboard / drag-and-drop formats for an item.</summary>
    /// <param name="forDrag">Images also get a real .png file, which most apps accept when dropped.</param>
    public static DataObject BuildData(ClipItem item, bool plain, bool forDrag)
    {
        var data = new DataObject();
        switch (item.Kind)
        {
            case ClipKind.Text:
                data.SetText(item.Text ?? "", TextDataFormat.UnicodeText);
                if (!plain)
                {
                    if (item.Html != null) data.SetText(item.Html, TextDataFormat.Html);
                    if (item.Rtf != null) data.SetText(item.Rtf, TextDataFormat.Rtf);
                }
                break;
            case ClipKind.Files:
                data.SetText(item.Text ?? "", TextDataFormat.UnicodeText);
                if (!plain)
                {
                    var list = new StringCollection();
                    list.AddRange(item.FilePaths);
                    data.SetFileDropList(list);
                }
                break;
            case ClipKind.Image when item.ImageFile != null:
                var path = Path.Combine(ClipStore.ImageDir, item.ImageFile);
                var bytes = File.ReadAllBytes(path);
                var bmp = BitmapFrame.Create(new MemoryStream(bytes), BitmapCreateOptions.None, BitmapCacheOption.OnLoad);
                data.SetImage(bmp);
                data.SetData("PNG", new MemoryStream(bytes), false);
                if (forDrag)
                {
                    // Give the dropped file a readable name instead of the internal id.
                    var dir = Path.Combine(Path.GetTempPath(), "ClipVault");
                    Directory.CreateDirectory(dir);
                    var named = Path.Combine(dir, $"Image {item.CopiedAt:yyyy-MM-dd HHmmss}.png");
                    File.Copy(path, named, true);
                    data.SetFileDropList(new StringCollection { named });
                }
                break;
        }
        return data;
    }

    /// <summary>Puts plain text on the clipboard without adding it to history (used for "Paste as…").</summary>
    public void PutText(string text) =>
        SetClipboard(() => Clipboard.SetDataObject(new DataObject(DataFormats.UnicodeText, text), true));

    void SetClipboard(Action write)
    {
        for (int attempt = 0; attempt < 5; attempt++)
        {
            try { write(); break; }
            catch (ExternalException) { Thread.Sleep(40); }
        }
        _ownClipboardSeq = Native.GetClipboardSequenceNumber();
    }
}
