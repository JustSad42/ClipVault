using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Text.RegularExpressions;
using System.Text.Json.Serialization;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace ClipVault;

public enum ClipKind { Text, Image, Files }

/// <summary>How an item is shown: special cards for colors, links, emails, phone numbers and code.</summary>
public enum CardKind { Plain, Code, Link, Color, Email, Phone, Files, Image }

public class ClipItem : INotifyPropertyChanged
{
    bool _pinned;
    DateTime _copiedAt;
    string? _text;
    string? _preview;
    int _lineCount = -1;
    string? _ocrText;
    string? _qrText;
    DateTime _fileStatsAt;
    string? _fileStats;
    string? _badge;
    CardKind? _card;
    Brush? _swatch;
    HashSet<string>? _tokens;

    public string Id { get; set; } = Guid.NewGuid().ToString("N");
    public ClipKind Kind { get; set; }

    /// <summary>Plain text. For Files items: one full path per line.</summary>
    public string? Text
    {
        get => _text;
        set
        {
            _text = value;
            _preview = null;
            _lineCount = -1;
            _card = null;
            _swatch = null;
            _tokens = null;
            OnChanged();
            OnChanged(nameof(Card));
            OnChanged(nameof(Swatch));
            OnChanged(nameof(LinkHost));
            OnChanged(nameof(LinkIcon));
            OnChanged(nameof(HasQuickAction));
            OnChanged(nameof(LeadImage));
            OnChanged(nameof(LeadPhoto));
            OnChanged(nameof(PreviewLine));
            OnChanged(nameof(Preview));
            OnChanged(nameof(Meta));
            OnChanged(nameof(TipText));
            OnChanged(nameof(FullText));
        }
    }

    /// <summary>Formatted versions kept alongside the text (web pages, Word, etc.).</summary>
    public string? Html { get; set; }
    public string? Rtf { get; set; }

    public string? ImageFile { get; set; }
    public int PixelWidth { get; set; }
    public int PixelHeight { get; set; }
    /// <summary>Text recognized inside an image. Null until it has been scanned.</summary>
    public string? OcrText
    {
        get => _ocrText;
        set { _ocrText = value; _tokens = null; OnChanged(); OnChanged(nameof(HasOcrText)); }
    }

    /// <summary>Text of a QR code found in an image ("" = none). Null until scanned.</summary>
    public string? QrText
    {
        get => _qrText;
        set { _qrText = value; _tokens = null; OnChanged(); OnChanged(nameof(HasQr)); OnChanged(nameof(QrIsLink)); OnChanged(nameof(Meta)); }
    }

    /// <summary>True when tracking parameters were removed from this link as it was copied.</summary>
    public bool LinkCleaned { get; set; }

    public string? SourceName { get; set; }
    public string? SourcePath { get; set; }
    public string Hash { get; set; } = "";

    public DateTime CopiedAt
    {
        get => _copiedAt;
        set { _copiedAt = value; OnChanged(); OnChanged(nameof(Meta)); OnChanged(nameof(Group)); }
    }

    public bool Pinned
    {
        get => _pinned;
        set { _pinned = value; OnChanged(); OnChanged(nameof(Group)); }
    }

    /// <summary>Quick-paste number ("1"–"9") for the first rows on screen, set by the popup.</summary>
    [JsonIgnore]
    public string? Badge
    {
        get => _badge;
        set { if (_badge != value) { _badge = value; OnChanged(); } }
    }

    /// <summary>Heading this item is listed under.</summary>
    [JsonIgnore]
    public string Group
    {
        get
        {
            if (Pinned) return "Pinned";
            var days = (DateTime.Today - CopiedAt.Date).TotalDays;
            return days <= 0 ? "Today" : days <= 1 ? "Yesterday" : days < 7 ? "Earlier this week" : "Older";
        }
    }

    /// <summary>Search ranking (2 exact, 1 close match). Set while filtering, used for sorting.</summary>
    [JsonIgnore] public int SearchRank { get; set; }

    /// <summary>Distinct lowercase words of the item, cached for fuzzy search.</summary>
    [JsonIgnore]
    public HashSet<string> SearchTokens => _tokens ??= Search.Tokenize(
        (Text is { Length: > 20000 } t ? t[..20000] : Text) + " " + OcrText + " " + QrText + " " + SourceName);

    // ---------- smart cards ----------

    static readonly Regex HexColor = new(@"^#([0-9a-fA-F]{3}|[0-9a-fA-F]{4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$");
    static readonly Regex RgbColor = new(@"^rgba?\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*(?:,\s*([\d.]+%?)\s*)?\)$", RegexOptions.IgnoreCase);
    static readonly Regex EmailRx = new(@"^[^\s@<>]+@[^\s@<>]+\.[A-Za-z]{2,}$");
    static readonly Regex PhoneRx = new(@"^\+?[\d\s().-]{7,22}$");
    static readonly Regex CodeHint = new(@"[;{}]\s*$|=>|^\s*(def|class|function|const|let|var|import|from|using|public|private|return|if|for|while|#include|<\w+|SELECT|FROM)\b|^\s*[\]}),]+;?\s*$", RegexOptions.Multiline);

    [JsonIgnore]
    public CardKind Card => _card ??= DetectCard();

    CardKind DetectCard()
    {
        if (IsImage) return CardKind.Image;
        if (IsFiles) return CardKind.Files;
        var t = (Text ?? "").Trim();
        if (t.Length == 0) return CardKind.Plain;
        if (t.Length <= 40 && ParseColor(t) != null) return CardKind.Color;
        if (t.Length <= 2000 && !t.Contains(' ') && !t.Contains('\n') &&
            Uri.TryCreate(t, UriKind.Absolute, out var uri) && (uri.Scheme == "http" || uri.Scheme == "https") && uri.Host.Contains('.'))
            return CardKind.Link;
        if (t.Length <= 254 && EmailRx.IsMatch(t)) return CardKind.Email;
        if (t.Length <= 22 && PhoneRx.IsMatch(t))
        {
            int digits = t.Count(char.IsDigit);
            if (digits is >= 7 and <= 15 && (t.StartsWith('+') || t.IndexOfAny([' ', '-', '(', '.']) > 0)) return CardKind.Phone;
        }
        if (LineCount >= 2)
        {
            var sample = t.Length > 4000 ? t[..4000] : t;
            int lines = sample.Count(c => c == '\n') + 1;
            if (CodeHint.Matches(sample).Count * 3 >= lines) return CardKind.Code;
        }
        return CardKind.Plain;
    }

    /// <summary>Reads "#RGB", "#RRGGBB", "#RRGGBBAA" or "rgb()/rgba()" text as a color.</summary>
    public static Color? ParseColor(string t)
    {
        var m = HexColor.Match(t);
        if (m.Success)
        {
            var h = m.Groups[1].Value;
            if (h.Length is 3 or 4) h = string.Concat(h.Select(c => $"{c}{c}"));
            byte P(int i) => Convert.ToByte(h.Substring(i, 2), 16);
            return Color.FromArgb(h.Length == 8 ? P(6) : (byte)255, P(0), P(2), P(4));
        }
        m = RgbColor.Match(t);
        if (m.Success)
        {
            int r = int.Parse(m.Groups[1].Value), g = int.Parse(m.Groups[2].Value), b = int.Parse(m.Groups[3].Value);
            if (r > 255 || g > 255 || b > 255) return null;
            byte a = 255;
            if (m.Groups[4].Success)
            {
                var v = m.Groups[4].Value;
                var inv = System.Globalization.CultureInfo.InvariantCulture;
                if (v.EndsWith('%') && double.TryParse(v.TrimEnd('%'), inv, out var pct)) a = (byte)Math.Clamp(pct * 2.55, 0, 255);
                else if (double.TryParse(v, inv, out var f)) a = (byte)Math.Clamp(f * 255, 0, 255);
            }
            return Color.FromArgb(a, (byte)r, (byte)g, (byte)b);
        }
        return null;
    }

    [JsonIgnore]
    public Brush? Swatch
    {
        get
        {
            if (Card != CardKind.Color) return null;
            if (_swatch == null && ParseColor((Text ?? "").Trim()) is { } c)
            {
                _swatch = new SolidColorBrush(c);
                _swatch.Freeze();
            }
            return _swatch;
        }
    }

    Color? SwatchColor => (Swatch as SolidColorBrush)?.Color;

    /// <summary>The color written three ways, for "Copy color as".</summary>
    [JsonIgnore]
    public string? ColorHex => SwatchColor is { } c
        ? (c.A == 255 ? $"#{c.R:X2}{c.G:X2}{c.B:X2}" : $"#{c.R:X2}{c.G:X2}{c.B:X2}{c.A:X2}") : null;

    [JsonIgnore]
    public string? ColorRgb => SwatchColor is { } c
        ? (c.A == 255 ? $"rgb({c.R}, {c.G}, {c.B})" : $"rgba({c.R}, {c.G}, {c.B}, {Math.Round(c.A / 255.0, 2).ToString(System.Globalization.CultureInfo.InvariantCulture)})") : null;

    [JsonIgnore]
    public string? ColorHsl
    {
        get
        {
            if (SwatchColor is not { } c) return null;
            double r = c.R / 255.0, g = c.G / 255.0, b = c.B / 255.0;
            double max = Math.Max(r, Math.Max(g, b)), min = Math.Min(r, Math.Min(g, b));
            double l = (max + min) / 2, h = 0, sat = 0;
            if (max != min)
            {
                double d = max - min;
                sat = l > 0.5 ? d / (2 - max - min) : d / (max + min);
                h = max == r ? (g - b) / d + (g < b ? 6 : 0) : max == g ? (b - r) / d + 2 : (r - g) / d + 4;
                h *= 60;
            }
            return $"hsl({Math.Round(h)}, {Math.Round(sat * 100)}%, {Math.Round(l * 100)}%)";
        }
    }

    /// <summary>Text and images can be saved to a file; copied files already are files.</summary>
    [JsonIgnore] public bool CanSaveToFile => !IsFiles;

    /// <summary>Card types that get a one-click action button (open link, show file, save image).</summary>
    [JsonIgnore] public bool HasQuickAction => Card is CardKind.Link or CardKind.Files or CardKind.Image;

    Uri? LinkUri => Card == CardKind.Link && Uri.TryCreate((Text ?? "").Trim(), UriKind.Absolute, out var u) ? u : null;

    /// <summary>Site name for link cards, e.g. "github.com".</summary>
    [JsonIgnore]
    public string? LinkHost => LinkUri is { } u ? (u.Host.StartsWith("www.") ? u.Host[4..] : u.Host) : null;

    /// <summary>The site's icon for link cards; loads in the background and shows up when ready.</summary>
    [JsonIgnore]
    public ImageSource? LinkIcon => LinkUri is { } u ? Favicons.Get(u, () => { OnChanged(nameof(LinkIcon)); OnChanged(nameof(LeadImage)); }) : null;

    /// <summary>Thumbnail size in the list (fits 330×90, keeps the image's shape).</summary>
    [JsonIgnore] public double ThumbWidth => ThumbScale * PixelWidth;
    [JsonIgnore] public double ThumbHeight => ThumbScale * PixelHeight;
    double ThumbScale => PixelWidth <= 0 || PixelHeight <= 0 ? 0 : Math.Min(1, Math.Min(330.0 / PixelWidth, 90.0 / PixelHeight));

    [JsonIgnore] public bool IsText => Kind == ClipKind.Text;
    [JsonIgnore] public bool IsImage => Kind == ClipKind.Image;
    [JsonIgnore] public bool IsFiles => Kind == ClipKind.Files;
    [JsonIgnore] public bool IsTextLike => Kind != ClipKind.Image;
    [JsonIgnore] public bool HasFormatting => Html != null || Rtf != null;
    [JsonIgnore] public string[] FilePaths => (Text ?? "").Split(["\r\n", "\n"], StringSplitOptions.RemoveEmptyEntries);
    [JsonIgnore] public ImageSource? SourceIcon => SourceApp.IconOf(SourcePath);

    [JsonIgnore]
    int LineCount => _lineCount >= 0 ? _lineCount : _lineCount = (Text ?? "").Trim().Count(c => c == '\n') + 1;

    /// <summary>Up to 3 lines for the list, keeping line breaks and indentation shape.</summary>
    [JsonIgnore]
    public string Preview => _preview ??= IsFiles ? FilesPreview() : TextPreview(Text ?? "");

    string FilesPreview()
    {
        var paths = FilePaths;
        var names = paths.Take(3).Select(p => Path.GetFileName(p.TrimEnd('\\')) is { Length: > 0 } n ? n : p).ToList();
        if (paths.Length > 3) names.Add($"+{paths.Length - 3} more");
        return string.Join("\n", names);
    }

    static string TextPreview(string text)
    {
        var src = text.Length > 4000 ? text[..4000] : text;
        var lines = src.Replace("\r\n", "\n").Replace('\r', '\n').Replace("\t", "    ")
            .Split('\n').Select(l => l.TrimEnd()).ToList();
        while (lines.Count > 0 && lines[0].Length == 0) lines.RemoveAt(0);
        while (lines.Count > 0 && lines[^1].Length == 0) lines.RemoveAt(lines.Count - 1);

        // Collapse runs of blank lines so the 3 preview lines show content.
        var compact = new List<string>();
        foreach (var l in lines)
            if (l.Length > 0 || (compact.Count > 0 && compact[^1].Length > 0)) compact.Add(l);

        var shown = compact.Take(3).ToList();
        // Remove indentation shared by all shown lines (e.g. code copied from inside a function).
        var nonEmpty = shown.Where(l => l.Length > 0).ToList();
        int indent = nonEmpty.Count > 0 ? nonEmpty.Min(l => l.Length - l.TrimStart(' ').Length) : 0;
        return string.Join("\n", shown.Select(l => l.Length >= indent ? l[indent..] : l));
    }

    [JsonIgnore]
    public string Meta
    {
        get
        {
            var parts = new List<string>();
            if (!string.IsNullOrEmpty(SourceName)) parts.Add(SourceName);
            switch (Kind)
            {
                case ClipKind.Image:
                    parts.Add($"Image {PixelWidth}×{PixelHeight}");
                    if (HasQr) parts.Add("QR: " + (QrHost ?? (QrText!.Length > 28 ? QrText[..28] + "…" : QrText)));
                    break;
                case ClipKind.Files:
                    parts.Add(FilePaths.Length == 1 ? "1 file" : $"{FilePaths.Length} files");
                    if (FileStats is { Length: > 0 } fs) parts.Add(fs);
                    break;
                default: parts.Add(LineCount > 1 ? $"{LineCount:N0} lines" : $"{Text?.Length ?? 0:N0} chars"); break;
            }
            if (HasFormatting) parts.Add("formatted");
            if (LinkCleaned) parts.Add("tracking removed");
            parts.Add(Ago(CopiedAt));
            return string.Join(" · ", parts);
        }
    }

    /// <summary>Hover tooltip text (capped so huge clips stay fast).</summary>
    [JsonIgnore]
    public string TipText
    {
        get
        {
            var t = Text ?? "";
            var lines = t.Replace("\r\n", "\n").Split('\n');
            var cut = string.Join("\n", lines.Take(25));
            if (cut.Length > 1500) cut = cut[..1500];
            return cut.Length < t.Length ? cut.TrimEnd() + "\n…" : cut;
        }
    }

    /// <summary>Text for the full preview pane (capped at 100k characters).</summary>
    [JsonIgnore]
    public string FullText => Text is { Length: > 100_000 } t ? t[..100_000] + "\n…" : Text ?? "";

    /// <summary>Small image for the list and grid. Only recently shown thumbnails stay in memory.</summary>
    [JsonIgnore] public ImageSource? Thumbnail => ImageFile == null ? null : ThumbCache.Get(Id, () => LoadImage(480));

    /// <summary>Larger image for the preview pane and tooltip; loaded on demand, not cached.</summary>
    [JsonIgnore] public ImageSource? FullImage => LoadImage(1600);

    /// <summary>Path of the stored image file.</summary>
    [JsonIgnore] public string? ImagePath => ImageFile == null ? null : Path.Combine(ClipStore.ImageDir, ImageFile);

    [JsonIgnore] public bool HasOcrText => !string.IsNullOrWhiteSpace(OcrText);

    /// <summary>Decodes the image with its longest side at most <paramref name="maxSide"/> pixels.</summary>
    ImageSource? LoadImage(int maxSide)
    {
        if (ImagePath == null) return null;
        try
        {
            var b = new BitmapImage();
            b.BeginInit();
            b.CacheOption = BitmapCacheOption.OnLoad;              // don't keep the file locked
            b.CreateOptions = BitmapCreateOptions.IgnoreImageCache; // don't let WPF keep its own copy
            b.UriSource = new Uri(ImagePath);
            if (PixelWidth >= PixelHeight) { if (PixelWidth > maxSide) b.DecodePixelWidth = maxSide; }
            else if (PixelHeight > maxSide) b.DecodePixelHeight = maxSide;
            b.EndInit();
            b.Freeze();
            return b;
        }
        catch { return null; }
    }

    public void RefreshMeta() { OnChanged(nameof(Meta)); OnChanged(nameof(Group)); OnChanged(nameof(AgoText)); }

    /// <summary>Short "5m ago" text for compact rows.</summary>
    [JsonIgnore] public string AgoText => Ago(CopiedAt);

    /// <summary>One line for compact rows.</summary>
    [JsonIgnore]
    public string PreviewLine => Kind switch
    {
        ClipKind.Image => HasQr ? $"Image {PixelWidth}×{PixelHeight} · QR: {QrHost ?? QrText}" : $"Image {PixelWidth}×{PixelHeight}",
        ClipKind.Files => string.Join(", ", Preview.Split('\n')),
        _ => Preview.Split('\n').FirstOrDefault(l => l.Trim().Length > 0)?.Trim() ?? "",
    };

    [JsonIgnore] public bool CanEdit => IsText || IsImage;

    // ---------- QR codes ----------

    [JsonIgnore] public bool HasQr => !string.IsNullOrEmpty(QrText);
    [JsonIgnore]
    public bool QrIsLink => HasQr && Uri.TryCreate(QrText, UriKind.Absolute, out var u) && (u.Scheme == "http" || u.Scheme == "https");
    [JsonIgnore] public string? QrHost => QrIsLink && Uri.TryCreate(QrText, UriKind.Absolute, out var u) ? u.Host : null;

    // ---------- file previews ----------

    static readonly HashSet<string> PhotoExts = new(StringComparer.OrdinalIgnoreCase) { ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".tif", ".tiff", ".ico" };

    string? FirstFile => IsFiles ? FilePaths.FirstOrDefault() : null;

    /// <summary>A photo thumbnail when the first copied file is an image.</summary>
    [JsonIgnore]
    public bool LeadPhoto => FirstFile is { } f && PhotoExts.Contains(Path.GetExtension(f)) && File.Exists(f);

    /// <summary>Picture shown in the card's lead box: site icon for links; photo or file icon for files.</summary>
    [JsonIgnore]
    public ImageSource? LeadImage => Card switch
    {
        CardKind.Link => LinkIcon,
        CardKind.Files when FirstFile is { } f => LeadPhoto
            ? ThumbCache.Get(Id + ":f", () => LoadFileThumb(f))
            : (File.Exists(f) || Directory.Exists(f) ? SourceApp.FileIcon(f) : null),
        _ => null,
    };

    static ImageSource? LoadFileThumb(string path)
    {
        try
        {
            var b = new BitmapImage();
            b.BeginInit();
            b.CacheOption = BitmapCacheOption.OnLoad;
            b.CreateOptions = BitmapCreateOptions.IgnoreImageCache;
            b.UriSource = new Uri(path);
            b.DecodePixelHeight = 96;
            b.EndInit();
            b.Freeze();
            return b;
        }
        catch { return null; }
    }

    /// <summary>"2.3 MB · modified Nov 3" for one file, total size for several, or "missing". Re-read every 30 s.</summary>
    string? FileStats
    {
        get
        {
            if (!IsFiles) return null;
            if (_fileStats != null && (DateTime.Now - _fileStatsAt).TotalSeconds < 30) return _fileStats;
            _fileStatsAt = DateTime.Now;
            try
            {
                var paths = FilePaths;
                long total = 0; int found = 0, folders = 0; DateTime? modified = null;
                foreach (var p in paths.Take(200))
                {
                    if (File.Exists(p)) { var fi = new FileInfo(p); total += fi.Length; found++; modified = fi.LastWriteTime; }
                    else if (Directory.Exists(p)) { found++; folders++; }
                }
                if (found == 0) return _fileStats = paths.Length == 1 ? "missing" : "all missing";
                var parts = new List<string>();
                if (folders == found) parts.Add(folders == 1 ? "folder" : "folders");
                else parts.Add(FormatSize(total));
                if (paths.Length == 1 && modified is { } m) parts.Add("modified " + (m.Year == DateTime.Now.Year ? m.ToString("MMM d") : m.ToString("MMM d, yyyy")));
                if (found < paths.Length && paths.Length > 1) parts.Add($"{paths.Length - found} missing");
                return _fileStats = string.Join(" · ", parts);
            }
            catch { return _fileStats = ""; }
        }
    }

    static string FormatSize(long bytes) => bytes switch
    {
        < 1024 => $"{bytes} B",
        < 1024 * 1024 => $"{bytes / 1024.0:0.#} KB",
        < 1024L * 1024 * 1024 => $"{bytes / 1048576.0:0.#} MB",
        _ => $"{bytes / 1073741824.0:0.##} GB",
    };

    /// <summary>Tells the list the image changed after editing.</summary>
    public void NotifyImageChanged()
    {
        foreach (var p in new[] { nameof(Thumbnail), nameof(ThumbWidth), nameof(ThumbHeight), nameof(FullImage), nameof(Meta), nameof(PreviewLine) })
            OnChanged(p);
    }

    static string Ago(DateTime t)
    {
        var d = DateTime.Now - t;
        if (d.TotalSeconds < 60) return "just now";
        if (d.TotalMinutes < 60) return $"{(int)d.TotalMinutes}m ago";
        if (d.TotalHours < 24) return $"{(int)d.TotalHours}h ago";
        if (d.TotalDays < 7) return $"{(int)d.TotalDays}d ago";
        return t.ToString("MMM d, yyyy");
    }

    public event PropertyChangedEventHandler? PropertyChanged;
    void OnChanged([CallerMemberName] string? name = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}

/// <summary>Keeps the most recently shown thumbnails in memory; older ones are reloaded from disk when needed.</summary>
static class ThumbCache
{
    const int Max = 40;
    static readonly Dictionary<string, LinkedListNode<(string Key, ImageSource? Image)>> Map = new();
    static readonly LinkedList<(string Key, ImageSource? Image)> Order = new();

    public static ImageSource? Get(string key, Func<ImageSource?> load)
    {
        if (Map.TryGetValue(key, out var node))
        {
            Order.Remove(node);
            Order.AddFirst(node);
            return node.Value.Image;
        }
        var image = load();
        Map[key] = Order.AddFirst((key, image));
        while (Order.Count > Max)
        {
            Map.Remove(Order.Last!.Value.Key);
            Order.RemoveLast();
        }
        return image;
    }

    public static void Clear() { Map.Clear(); Order.Clear(); }

    public static void Remove(string key)
    {
        foreach (var k in new[] { key, key + ":f" })
            if (Map.Remove(k, out var node)) Order.Remove(node);
    }
}
