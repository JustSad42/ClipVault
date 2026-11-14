using System.Net.Http;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace ClipVault;

/// <summary>
/// Site icons for link cards. Each icon is downloaded once, straight from the site itself
/// (https://site/favicon.ico, no third-party service), and cached in %APPDATA%\ClipVault\icons.
/// </summary>
static class Favicons
{
    static readonly string Dir = Path.Combine(ClipStore.Dir, "icons");
    static readonly Dictionary<string, ImageSource?> Cache = new();
    static readonly Dictionary<string, List<Action>> Pending = new();
    static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(5) };

    static Favicons() => Http.DefaultRequestHeaders.UserAgent.ParseAdd("Mozilla/5.0 ClipVault");

    /// <summary>Returns the icon if it's ready; otherwise starts loading it and calls <paramref name="loaded"/> later.</summary>
    public static ImageSource? Get(Uri uri, Action loaded)
    {
        var host = uri.Host.ToLowerInvariant();
        if (Cache.TryGetValue(host, out var icon)) return icon;

        if (Pending.TryGetValue(host, out var waiting)) { waiting.Add(loaded); return null; }

        // From disk?
        var file = Path.Combine(Dir, host + ".ico");
        if (File.Exists(file)) return Cache[host] = Decode(File.ReadAllBytes(file));
        if (File.Exists(file + ".none") || !((App)Application.Current).Settings.LinkIcons) return null;

        Pending[host] = [loaded];
        _ = Download(uri, host, file);
        return null;
    }

    static async Task Download(Uri uri, string host, string file)
    {
        ImageSource? icon = null;
        try
        {
            var bytes = await Http.GetByteArrayAsync($"{uri.Scheme}://{uri.Host}/favicon.ico");
            if (bytes.Length is > 0 and < 512_000)
            {
                icon = Decode(bytes);
                Directory.CreateDirectory(Dir);
                if (icon != null) await File.WriteAllBytesAsync(file, bytes);
            }
        }
        catch { }
        if (icon == null)
        {
            // Remember the failure so we don't keep asking that site.
            try { Directory.CreateDirectory(Dir); File.WriteAllText(file + ".none", ""); } catch { }
        }

        Cache[host] = icon;
        if (Pending.Remove(host, out var callbacks))
            foreach (var cb in callbacks) cb();
    }

    /// <summary>Reads .ico or .png bytes and picks the frame closest to 32px.</summary>
    static ImageSource? Decode(byte[] bytes)
    {
        try
        {
            var decoder = BitmapDecoder.Create(new MemoryStream(bytes), BitmapCreateOptions.None, BitmapCacheOption.OnLoad);
            var frame = decoder.Frames.OrderBy(f => Math.Abs(f.PixelWidth - 32)).First();
            frame.Freeze();
            return frame;
        }
        catch { return null; }
    }
}
