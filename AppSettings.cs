using System.Text.Json;
using System.Text.Json.Serialization;
using System.Windows.Input;

namespace ClipVault;

public enum AppTheme { System, Dark, Light }
public enum OpenPosition { TextCursor, Mouse, LastPosition }

/// <summary>User preferences, saved to %APPDATA%\ClipVault\settings.json.</summary>
public class AppSettings
{
    public uint HotkeyModifiers { get; set; } = Native.MOD_CONTROL | Native.MOD_SHIFT;
    public uint HotkeyKey { get; set; } = 0x56; // V
    public int MaxHistory { get; set; } = 1000;
    public AppTheme Theme { get; set; } = AppTheme.System;
    public OpenPosition OpenAt { get; set; } = OpenPosition.TextCursor;
    public bool UseSystemAccent { get; set; } = true;
    /// <summary>Download a link's site icon (straight from that site) for link cards.</summary>
    public bool LinkIcons { get; set; } = true;
    /// <summary>Remove tracking parameters (utm_source, fbclid…) from links as they're copied.</summary>
    public bool CleanLinks { get; set; } = true;
    /// <summary>Apps (exe names without .exe) that always get plain text, e.g. "code", "windowsterminal".</summary>
    public List<string> PlainTextApps { get; set; } = new();
    public bool Sounds { get; set; }
    public bool Compact { get; set; }
    public double PopupWidth { get; set; } = 440;
    public double PopupHeight { get; set; } = 580;
    public double? PopupLeft { get; set; }
    public double? PopupTop { get; set; }
    public bool AutoUpdate { get; set; } = true;

    static string FilePath => Path.Combine(ClipStore.Dir, "settings.json");
    static readonly JsonSerializerOptions Json = new() { WriteIndented = true, Converters = { new JsonStringEnumConverter() } };

    public static AppSettings Load()
    {
        try
        {
            if (File.Exists(FilePath))
                return JsonSerializer.Deserialize<AppSettings>(File.ReadAllText(FilePath), Json) ?? new();
        }
        catch { }
        return new();
    }

    public void Save()
    {
        try
        {
            Directory.CreateDirectory(ClipStore.Dir);
            File.WriteAllText(FilePath, JsonSerializer.Serialize(this, Json));
        }
        catch { }
    }

    /// <summary>A separate copy, so a settings window can change it without touching the live settings.</summary>
    public AppSettings Clone()
    {
        var c = (AppSettings)MemberwiseClone();
        c.PlainTextApps = new List<string>(PlainTextApps);
        return c;
    }

    public string HotkeyText => FormatHotkey(HotkeyModifiers, HotkeyKey);

    public static string FormatHotkey(uint mods, uint vk)
    {
        var parts = new List<string>();
        if ((mods & Native.MOD_CONTROL) != 0) parts.Add("Ctrl");
        if ((mods & Native.MOD_ALT) != 0) parts.Add("Alt");
        if ((mods & Native.MOD_SHIFT) != 0) parts.Add("Shift");
        if ((mods & Native.MOD_WIN) != 0) parts.Add("Win");
        var key = KeyInterop.KeyFromVirtualKey((int)vk);
        parts.Add(key switch
        {
            >= Key.D0 and <= Key.D9 => ((int)(key - Key.D0)).ToString(),
            >= Key.NumPad0 and <= Key.NumPad9 => "Num " + (int)(key - Key.NumPad0),
            Key.Oem3 => "`",
            Key.OemMinus => "-",
            Key.OemPlus => "=",
            Key.OemComma => ",",
            Key.OemPeriod => ".",
            Key.Oem2 => "/",
            Key.Oem1 => ";",
            Key.OemQuotes => "'",
            Key.OemOpenBrackets => "[",
            Key.Oem6 => "]",
            Key.Oem5 => "\\",
            _ => key.ToString(),
        });
        return string.Join(" + ", parts);
    }
}
