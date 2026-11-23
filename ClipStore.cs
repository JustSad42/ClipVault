using System.Collections.ObjectModel;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;

namespace ClipVault;

/// <summary>Holds the clipboard history and saves it to %APPDATA%\ClipVault.</summary>
public sealed class ClipStore
{
    // CLIPVAULT_DATA points tests at a scratch folder instead of the real history.
    public static readonly string Dir = Environment.GetEnvironmentVariable("CLIPVAULT_DATA") is { Length: > 0 } d ? d
        : Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "ClipVault");
    public static readonly string ImageDir = Path.Combine(Dir, "images");
    static readonly string FilePath = Path.Combine(Dir, "history.json");

    static readonly JsonSerializerOptions Json = new() { Converters = { new JsonStringEnumConverter() } };

    readonly DispatcherTimer _saveTimer = new() { Interval = TimeSpan.FromMilliseconds(600) };
    bool _ocrRunning;

    /// <summary>Oldest unpinned items beyond this count are dropped. Pinned items are never dropped.</summary>
    public int MaxUnpinned { get; set; } = 1000;

    public ObservableCollection<ClipItem> Items { get; } = new();

    public ClipStore()
    {
        _saveTimer.Tick += (_, _) => { _saveTimer.Stop(); SaveNow(); };
    }

    public void Load()
    {
        Directory.CreateDirectory(ImageDir);
        if (!File.Exists(FilePath)) return;
        try
        {
            var list = JsonSerializer.Deserialize<List<ClipItem>>(File.ReadAllText(FilePath), Json) ?? new();
            foreach (var item in list)
            {
                if (item.IsImage && (item.ImageFile == null || !File.Exists(Path.Combine(ImageDir, item.ImageFile))))
                    continue;
                Items.Add(item);
            }
            RememberHashes();
        }
        catch
        {
            // Keep a copy of the unreadable file instead of silently overwriting it.
            try { File.Copy(FilePath, FilePath + ".bad", true); } catch { }
        }
    }

    public void ScheduleSave() { _saveTimer.Stop(); _saveTimer.Start(); }

    public void SaveNow()
    {
        _saveTimer.Stop();
        try
        {
            Directory.CreateDirectory(Dir);
            var tmp = FilePath + ".tmp";
            File.WriteAllText(tmp, JsonSerializer.Serialize(Items, Json));
            File.Move(tmp, FilePath, true);
        }
        catch { /* disk full / locked: try again on next change */ }
    }

    public void AddText(string text, string? html, string? rtf, (string? Name, string? Path) source, bool linkCleaned = false)
    {
        var hash = TextHash(text);
        var existing = Items.FirstOrDefault(i => i.Hash == hash);
        if (existing != null)
        {
            // Same text again: move it to the top and keep the newest formatting.
            if (html != null || rtf != null) { existing.Html = html; existing.Rtf = rtf; }
            if (linkCleaned) existing.LinkCleaned = true;
            existing.InvalidateTags();
            existing.CopiedAt = DateTime.Now;
            ScheduleSave();
            return;
        }
        Add(new ClipItem
        {
            Kind = ClipKind.Text, Text = text, Html = html, Rtf = rtf, Hash = hash,
            SourceName = source.Name, SourcePath = source.Path, LinkCleaned = linkCleaned,
        });
    }

    public void AddFiles(string[] paths, (string? Name, string? Path) source)
    {
        var text = string.Join(Environment.NewLine, paths);
        var hash = "f:" + Sha(Encoding.UTF8.GetBytes(text));
        if (Bump(hash)) return;
        Add(new ClipItem { Kind = ClipKind.Files, Text = text, Hash = hash, SourceName = source.Name, SourcePath = source.Path });
    }

    /// <summary>
    /// Adds a copied image. Only the pixel copy happens on the UI thread; hashing and PNG
    /// encoding (a few hundred ms for a 4K screenshot) run in the background.
    /// </summary>
    public async Task AddImageAsync(BitmapSource src, (string? Name, string? Path) source)
    {
        BitmapSource bmp = src.Format == PixelFormats.Bgra32
            ? src
            : new FormatConvertedBitmap(src, PixelFormats.Bgra32, null, 0);
        int w = bmp.PixelWidth, h = bmp.PixelHeight;
        if (w <= 0 || h <= 0) return;

        var pixels = new byte[w * h * 4];
        bmp.CopyPixels(pixels, w * 4, 0);

        var item = new ClipItem
        {
            Kind = ClipKind.Image, PixelWidth = w, PixelHeight = h,
            SourceName = source.Name, SourcePath = source.Path,
        };
        item.ImageFile = item.Id + ".png";
        var file = Path.Combine(ImageDir, item.ImageFile);

        string? hash = null;
        try
        {
            hash = await Task.Run(() =>
            {
                var hash = $"i:{w}x{h}:" + Sha(pixels);
                lock (_hashes) if (_hashes.Contains(hash)) return hash; // duplicate: no need to encode
                Directory.CreateDirectory(ImageDir);
                using var fs = File.Create(file);
                var enc = new PngBitmapEncoder();
                var full = BitmapSource.Create(w, h, 96, 96, PixelFormats.Bgra32, null, pixels, w * 4);
                enc.Frames.Add(BitmapFrame.Create(full));
                enc.Save(fs);
                if (Math.Max(w, h) > Thumbs.Size) Thumbs.Save(full, Thumbs.PathFor(item.ImageFile));
                return hash;
            });
        }
        catch { try { File.Delete(file); } catch { } return; }

        if (Bump(hash)) { try { File.Delete(file); } catch { } Thumbs.Delete(item.ImageFile); return; }
        item.Hash = hash;
        Add(item);
        RunOcr();
    }

    // Image hashes, so the background task can skip encoding duplicates.
    readonly HashSet<string> _hashes = new();
    void RememberHashes() { lock (_hashes) { _hashes.Clear(); foreach (var i in Items) if (i.IsImage && i.Hash != null) _hashes.Add(i.Hash); } }

    void Add(ClipItem item)
    {
        item.CopiedAt = DateTime.Now;
        Items.Insert(0, item);
        Trim();
        ScheduleSave();
        if (item.IsImage) RememberHashes();
    }

    /// <summary>If this content is already in history, move it to the top instead of adding a duplicate.</summary>
    bool Bump(string hash)
    {
        var existing = Items.FirstOrDefault(i => i.Hash == hash);
        if (existing == null) return false;
        existing.CopiedAt = DateTime.Now;
        ScheduleSave();
        return true;
    }

    /// <summary>Replaces an item's text after editing. Formatting is dropped since it no longer matches.</summary>
    public void UpdateText(ClipItem item, string text)
    {
        item.Html = null;
        item.Rtf = null;
        item.Hash = TextHash(text);
        item.Text = text;
        ScheduleSave();
    }

    // Deleted items keep their image files for a while, so Undo can bring them back.
    readonly List<(ClipItem Item, DateTime When)> _deleted = new();

    public void Remove(ClipItem item)
    {
        Items.Remove(item);
        _deleted.Add((item, DateTime.Now));
        ScheduleSave();
    }

    /// <returns>The removed items (for Undo).</returns>
    public List<ClipItem> ClearUnpinned()
    {
        var removed = Items.Where(i => !i.Pinned).ToList();
        foreach (var i in removed)
        {
            Items.Remove(i);
            _deleted.Add((i, DateTime.Now));
        }
        ScheduleSave();
        return removed;
    }

    /// <summary>Puts deleted items back (Undo).</summary>
    public void Restore(IEnumerable<ClipItem> items)
    {
        foreach (var item in items)
        {
            if (Items.Contains(item)) continue;
            if (item.IsImage && (item.ImageFile == null || !File.Exists(Path.Combine(ImageDir, item.ImageFile)))) continue;
            _deleted.RemoveAll(d => d.Item == item);
            Items.Add(item);
        }
        ScheduleSave();
    }

    /// <summary>Deletes image files of items removed more than <paramref name="olderThan"/> ago (all when null).</summary>
    public void PurgeDeleted(TimeSpan? olderThan = null)
    {
        var cutoff = DateTime.Now - (olderThan ?? TimeSpan.Zero);
        foreach (var d in _deleted.Where(d => olderThan == null || d.When < cutoff).ToList())
        {
            DeleteImage(d.Item);
            _deleted.Remove(d);
        }
    }

    /// <summary>Replaces an item's image after editing it.</summary>
    public void UpdateImage(ClipItem item, BitmapSource bmp)
    {
        var old = item.ImageFile;
        var file = $"{item.Id}-{DateTime.Now.Ticks}.png";
        using (var fs = File.Create(Path.Combine(ImageDir, file)))
        {
            var enc = new PngBitmapEncoder();
            enc.Frames.Add(BitmapFrame.Create(bmp));
            enc.Save(fs);
        }
        int w = bmp.PixelWidth, h = bmp.PixelHeight;
        var px = new byte[w * h * 4];
        new FormatConvertedBitmap(bmp, PixelFormats.Bgra32, null, 0).CopyPixels(px, w * 4, 0);
        item.ImageFile = file;
        item.PixelWidth = w;
        item.PixelHeight = h;
        item.Hash = $"i:{w}x{h}:" + Sha(px);
        item.OcrText = null; // read the edited image again
        item.QrText = null;
        ThumbCache.Remove(item.Id);
        item.NotifyImageChanged();
        if (old != null) { try { File.Delete(Path.Combine(ImageDir, old)); } catch { } Thumbs.Delete(old); }
        ScheduleSave();
        RunOcr();
    }

    public void Trim()
    {
        var extra = Items.Where(i => !i.Pinned).OrderByDescending(i => i.CopiedAt).Skip(MaxUnpinned).ToList();
        if (extra.Count == 0) return;
        foreach (var i in extra)
        {
            Items.Remove(i);
            DeleteImage(i);
        }
        ScheduleSave();
        RememberHashes();
    }

    /// <summary>Scans images that haven't been read yet for text, one at a time in the background.</summary>
    public async void RunOcr()
    {
        if (_ocrRunning) return;
        _ocrRunning = true;
        try
        {
            while (Items.FirstOrDefault(i => i.IsImage && (i.OcrText == null || i.QrText == null)) is { } item)
            {
                var path = Path.Combine(ImageDir, item.ImageFile!);
                if (item.OcrText == null)
                {
                    string text = "";
                    try { text = await Ocr.ReadAsync(path) ?? ""; }
                    catch { /* unreadable image: mark as scanned */ }
                    item.OcrText = text;
                }
                if (item.QrText == null) item.QrText = await Qr.ReadAsync(path);
                ScheduleSave();
            }
        }
        finally { _ocrRunning = false; }
    }

    static void DeleteImage(ClipItem i)
    {
        if (i.ImageFile == null) return;
        try { File.Delete(Path.Combine(ImageDir, i.ImageFile)); } catch { }
        Thumbs.Delete(i.ImageFile);
    }

    static string TextHash(string text) => "t:" + Sha(Encoding.UTF8.GetBytes(text));
    static string Sha(byte[] data) => Convert.ToHexString(SHA256.HashData(data));
}
