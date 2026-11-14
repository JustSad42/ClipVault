using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;

namespace ClipVault;

public partial class SettingsWindow : Window
{
    readonly App _app;
    uint _mods, _vk;

    public SettingsWindow(App app)
    {
        _app = app;
        InitializeComponent();

        var s = app.Settings;
        _mods = s.HotkeyModifiers;
        _vk = s.HotkeyKey;
        HotkeyBox.Text = s.HotkeyText;
        HistoryBox.Text = s.MaxHistory.ToString();
        (s.Theme switch { AppTheme.Dark => ThemeDark, AppTheme.Light => ThemeLight, _ => ThemeSystem }).IsChecked = true;
        (s.OpenAt switch { OpenPosition.Mouse => OpenMouse, OpenPosition.LastPosition => OpenLast, _ => OpenCaret }).IsChecked = true;
        StartupBox.IsChecked = Startup.IsEnabled;
        AccentBox.IsChecked = s.UseSystemAccent;
        LinkIconsBox.IsChecked = s.LinkIcons;
        CleanLinksBox.IsChecked = s.CleanLinks;
        PlainAppsBox.Text = string.Join(", ", s.PlainTextApps);
        CompactBox.IsChecked = s.Compact;
        SoundsBox.IsChecked = s.Sounds;
        AutoUpdateBox.IsChecked = s.AutoUpdate;
        VersionText.Text = "Version " + Updater.CurrentText;
        UpdateVersion.Text = "ClipVault " + Updater.CurrentText;
        ShowUpdateStatus();
        Updater.StatusChanged += ShowUpdateStatus;
        Closed += (_, _) => Updater.StatusChanged -= ShowUpdateStatus;

        Backdrop.Attach(this);
    }

    void OnNav(object sender, RoutedEventArgs e)
    {
        if (PageGeneral == null) return; // still loading
        PageGeneral.Visibility = NavGeneral.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
        PageAppearance.Visibility = NavAppearance.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
        PageHistory.Visibility = NavHistory.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
        PageUpdates.Visibility = NavUpdates.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
        PageTitle.Text = (string)((RadioButton)sender).Content;
    }

    // ---------- shortcut recorder ----------

    void OnHotkeyFocus(object sender, KeyboardFocusChangedEventArgs e)
    {
        // Turn the current shortcut off while recording, so pressing it here doesn't open the popup.
        _app.SuspendHotkey();
        HotkeyHint.Text = "Press the new shortcut now…";
    }

    void OnHotkeyBlur(object sender, KeyboardFocusChangedEventArgs e)
    {
        _app.RegisterHotkey();
        HotkeyHint.Text = "Click the box, then press the new key combination.";
    }

    void OnHotkeyKeyDown(object sender, KeyEventArgs e)
    {
        var key = e.Key == Key.System ? e.SystemKey : e.Key;
        if (key is Key.Tab or Key.Escape) return; // let these move focus / close the window
        e.Handled = true;
        if (key is Key.LeftCtrl or Key.RightCtrl or Key.LeftAlt or Key.RightAlt or Key.LeftShift
            or Key.RightShift or Key.LWin or Key.RWin)
            return; // wait for the main key

        uint mods = 0;
        var m = Keyboard.Modifiers;
        if (m.HasFlag(ModifierKeys.Control)) mods |= Native.MOD_CONTROL;
        if (m.HasFlag(ModifierKeys.Alt)) mods |= Native.MOD_ALT;
        if (m.HasFlag(ModifierKeys.Shift)) mods |= Native.MOD_SHIFT;
        if (m.HasFlag(ModifierKeys.Windows)) mods |= Native.MOD_WIN;
        if ((mods & ~Native.MOD_SHIFT) == 0)
        {
            HotkeyHint.Text = "Include Ctrl, Alt or Win in the shortcut.";
            return;
        }
        _mods = mods;
        _vk = (uint)KeyInterop.VirtualKeyFromKey(key);
        HotkeyBox.Text = AppSettings.FormatHotkey(_mods, _vk);
        HotkeyHint.Text = "Click Save to use this shortcut.";
    }

    void OnDigitsOnly(object sender, TextCompositionEventArgs e) => e.Handled = !e.Text.All(char.IsDigit);

    // ---------- save / cancel ----------

    void OnSave(object sender, RoutedEventArgs e)
    {
        if (!int.TryParse(HistoryBox.Text, out int max) || max < 10)
        {
            NavHistory.IsChecked = true;
            MessageBox.Show(this, "History size must be a number of at least 10.", "ClipVault");
            return;
        }
        max = Math.Min(max, 100_000);

        var old = _app.Settings;
        if (_mods != old.HotkeyModifiers || _vk != old.HotkeyKey)
        {
            if (!_app.TryRegisterHotkey(_mods, _vk))
            {
                _app.RegisterHotkey(); // put the old one back
                MessageBox.Show(this, $"{AppSettings.FormatHotkey(_mods, _vk)} is already used by Windows or another app. Try a different one.",
                    "ClipVault", MessageBoxButton.OK, MessageBoxImage.Warning);
                return;
            }
        }

        var n = old.Clone();
        n.HotkeyModifiers = _mods;
        n.HotkeyKey = _vk;
        n.MaxHistory = max;
        n.Theme = ThemeDark.IsChecked == true ? AppTheme.Dark : ThemeLight.IsChecked == true ? AppTheme.Light : AppTheme.System;
        n.OpenAt = OpenMouse.IsChecked == true ? OpenPosition.Mouse : OpenLast.IsChecked == true ? OpenPosition.LastPosition : OpenPosition.TextCursor;
        n.UseSystemAccent = AccentBox.IsChecked == true;
        n.LinkIcons = LinkIconsBox.IsChecked == true;
        n.CleanLinks = CleanLinksBox.IsChecked == true;
        n.PlainTextApps = PlainAppsBox.Text.Split([',', ';', ' '], StringSplitOptions.RemoveEmptyEntries)
            .Select(a => (a.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ? a[..^4] : a).ToLowerInvariant())
            .Distinct().ToList();
        n.Compact = CompactBox.IsChecked == true;
        n.Sounds = SoundsBox.IsChecked == true;
        n.AutoUpdate = AutoUpdateBox.IsChecked == true;
        _app.SaveSettings(n);
        _app.RegisterHotkey();

        bool startup = StartupBox.IsChecked == true;
        if (startup != Startup.IsEnabled) Startup.Set(startup);
        Close();
    }

    void OnCancel(object sender, RoutedEventArgs e) => Close();

    // ---------- updates ----------

    void ShowUpdateStatus()
    {
        UpdateStatus.Text = Updater.Status.Length > 0 ? Updater.Status : "Not checked yet.";
        InstallButton.Visibility = Updater.Ready != null ? Visibility.Visible : Visibility.Collapsed;
    }

    async void OnCheckUpdates(object sender, RoutedEventArgs e)
    {
        CheckNowButton.IsEnabled = false;
        await Updater.CheckAsync(manual: true);
        CheckNowButton.IsEnabled = true;
    }

    void OnInstallUpdate(object sender, RoutedEventArgs e) => Updater.ApplyAndRestart();

    void OnReleaseNotes(object sender, RoutedEventArgs e)
    {
        try { System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(Updater.ReleasesPage) { UseShellExecute = true }); } catch { }
    }
}
