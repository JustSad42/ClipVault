using System.Windows;
using System.Windows.Controls;
using System.Windows.Documents;
using System.Windows.Media;

namespace ClipVault;

/// <summary>
/// Search that forgives typos: every word you type must appear in the item, either exactly
/// or as a close match ("invc" or "invioce" finds "invoice").
/// </summary>
static class Search
{
    public static string[] Words(string query) =>
        query.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);

    /// <summary>
    /// Splits a query into search words and filters:
    /// app:chrome, type:link|code|color|email|phone|image|file|text|pinned, today, yesterday,
    /// date:today|yesterday|week|month. Anything else is a search word.
    /// </summary>
    public static (string[] Words, List<Func<ClipItem, bool>> Filters) Parse(string query)
    {
        var words = new List<string>();
        var filters = new List<Func<ClipItem, bool>>();
        foreach (var w in Words(query))
        {
            var lw = w.ToLowerInvariant();
            int colon = lw.IndexOf(':');
            if (colon > 0 && colon < lw.Length - 1)
            {
                var key = lw[..colon];
                var val = lw[(colon + 1)..];
                Func<ClipItem, bool>? f = key switch
                {
                    "app" or "from" => i => i.SourceName?.Contains(val, StringComparison.OrdinalIgnoreCase) == true,
                    "type" or "is" => TypeFilter(val),
                    "date" or "when" => DateFilter(val),
                    _ => null,
                };
                if (f != null) { filters.Add(f); continue; }
            }
            if (lw is "today" or "yesterday") { filters.Add(DateFilter(lw)!); continue; }
            words.Add(w);
        }
        return (words.ToArray(), filters);
    }

    static readonly (string Name, Func<ClipItem, bool> Test)[] Types =
    [
        ("link", i => i.Card == CardKind.Link), ("url", i => i.Card == CardKind.Link),
        ("code", i => i.Card == CardKind.Code),
        ("color", i => i.Card == CardKind.Color), ("colour", i => i.Card == CardKind.Color),
        ("email", i => i.Card == CardKind.Email), ("mail", i => i.Card == CardKind.Email),
        ("phone", i => i.Card == CardKind.Phone),
        ("image", i => i.IsImage), ("img", i => i.IsImage), ("picture", i => i.IsImage), ("screenshot", i => i.IsImage),
        ("file", i => i.IsFiles),
        ("text", i => i.IsText),
        ("pinned", i => i.Pinned),
        ("qr", i => i.HasQr),
    ];

    /// <summary>Matches a type by its start, so "type:img", "type:pic" and "type:links" all work.</summary>
    static Func<ClipItem, bool>? TypeFilter(string val)
    {
        var v = val.TrimEnd('s');
        if (v.Length < 2) return null;
        foreach (var (name, test) in Types)
            if (name.StartsWith(v) || v.StartsWith(name)) return test;
        return null;
    }

    static Func<ClipItem, bool>? DateFilter(string val)
    {
        var today = DateTime.Today;
        return val switch
        {
            "today" => i => i.CopiedAt.Date == today,
            "yesterday" => i => i.CopiedAt.Date == today.AddDays(-1),
            "week" => i => i.CopiedAt.Date >= today.AddDays(-6),
            "month" => i => i.CopiedAt.Date >= today.AddDays(-30),
            _ => null,
        };
    }

    public static HashSet<string> Tokenize(string text)
    {
        var set = new HashSet<string>();
        int start = -1;
        for (int i = 0; i <= text.Length; i++)
        {
            bool word = i < text.Length && char.IsLetterOrDigit(text[i]);
            if (word && start < 0) start = i;
            else if (!word && start >= 0)
            {
                if (i - start <= 40) set.Add(text[start..i].ToLowerInvariant());
                start = -1;
            }
        }
        return set;
    }

    /// <summary>0 = no match, 1 = matched with typos, 2 = every word found exactly.</summary>
    public static int Score(ClipItem item, string[] words)
    {
        if (words.Length == 0) return 2;
        int score = 2;
        foreach (var w in words)
        {
            if (Exact(item, w)) continue;
            if (w.Length >= 3 && item.SearchTokens.Any(t => Close(w.ToLowerInvariant(), t))) { score = 1; continue; }
            return 0;
        }
        return score;
    }

    static bool Exact(ClipItem item, string w)
    {
        bool Has(string? s) => s != null && s.Contains(w, StringComparison.OrdinalIgnoreCase);
        return Has(item.Text) || Has(item.SourceName) || Has(item.OcrText) || Has(item.QrText) || (item.IsImage && Has("image"));
    }

    /// <summary>True when a word is a close match for a token: letters in order with a few skipped, or a small typo.</summary>
    public static bool Close(string w, string t)
    {
        if (t.Length == 0 || t[0] != w[0]) return false;
        if (t.Length >= w.Length && t.Length <= w.Length * 2 + 1 && IsSubsequence(w, t)) return true;
        int maxEdits = w.Length >= 7 ? 2 : w.Length >= 4 ? 1 : 0;
        return maxEdits > 0 && Math.Abs(t.Length - w.Length) <= maxEdits && Distance(w, t, maxEdits) <= maxEdits;
    }

    static bool IsSubsequence(string w, string t)
    {
        int j = 0;
        for (int i = 0; i < t.Length && j < w.Length; i++)
            if (t[i] == w[j]) j++;
        return j == w.Length;
    }

    /// <summary>Edit distance with swapped letters counting as one edit. Stops early past the limit.</summary>
    static int Distance(string a, string b, int limit)
    {
        var d = new int[a.Length + 1, b.Length + 1];
        for (int i = 0; i <= a.Length; i++) d[i, 0] = i;
        for (int j = 0; j <= b.Length; j++) d[0, j] = j;
        for (int i = 1; i <= a.Length; i++)
        {
            int rowMin = int.MaxValue;
            for (int j = 1; j <= b.Length; j++)
            {
                int cost = a[i - 1] == b[j - 1] ? 0 : 1;
                int v = Math.Min(Math.Min(d[i - 1, j] + 1, d[i, j - 1] + 1), d[i - 1, j - 1] + cost);
                if (i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1]) v = Math.Min(v, d[i - 2, j - 2] + 1);
                d[i, j] = v;
                rowMin = Math.Min(rowMin, v);
            }
            if (rowMin > limit) return limit + 1;
        }
        return d[a.Length, b.Length];
    }

    /// <summary>Parts of the text to highlight: exact occurrences, or whole words that closely match.</summary>
    public static List<(int Start, int Length)> Ranges(string text, string[] words)
    {
        var ranges = new List<(int, int)>();
        foreach (var w in words)
        {
            bool found = false;
            for (int i = text.IndexOf(w, StringComparison.OrdinalIgnoreCase); i >= 0;
                 i = text.IndexOf(w, i + w.Length, StringComparison.OrdinalIgnoreCase))
            {
                ranges.Add((i, w.Length));
                found = true;
            }
            if (found || w.Length < 3) continue;
            var lw = w.ToLowerInvariant();
            int start = -1;
            for (int i = 0; i <= text.Length; i++)
            {
                bool letter = i < text.Length && char.IsLetterOrDigit(text[i]);
                if (letter && start < 0) start = i;
                else if (!letter && start >= 0)
                {
                    if (Close(lw, text[start..i].ToLowerInvariant())) ranges.Add((start, i - start));
                    start = -1;
                }
            }
        }
        // Sort and merge overlaps.
        ranges.Sort();
        var merged = new List<(int Start, int Length)>();
        foreach (var (s, l) in ranges)
        {
            if (merged.Count > 0 && s <= merged[^1].Start + merged[^1].Length)
            {
                var last = merged[^1];
                merged[^1] = (last.Start, Math.Max(last.Start + last.Length, s + l) - last.Start);
            }
            else merged.Add((s, l));
        }
        return merged;
    }
}

/// <summary>
/// Attached property that shows a TextBlock's text with the current search words highlighted.
/// Usage: local:Highlighter.Text="{Binding Preview}"
/// </summary>
public static class Highlighter
{
    static string[] _words = [];
    static event Action? QueryChanged;

    public static void SetQuery(string query)
    {
        var words = Search.Words(query);
        if (words.SequenceEqual(_words)) return;
        _words = words;
        QueryChanged?.Invoke();
    }

    public static readonly DependencyProperty TextProperty = DependencyProperty.RegisterAttached(
        "Text", typeof(string), typeof(Highlighter), new PropertyMetadata(null, OnTextChanged));

    public static string? GetText(DependencyObject d) => (string?)d.GetValue(TextProperty);
    public static void SetText(DependencyObject d, string? value) => d.SetValue(TextProperty, value);

    static readonly DependencyProperty HookedProperty = DependencyProperty.RegisterAttached(
        "Hooked", typeof(Action), typeof(Highlighter));

    static void OnTextChanged(DependencyObject d, DependencyPropertyChangedEventArgs e)
    {
        if (d is not TextBlock tb) return;
        if (tb.GetValue(HookedProperty) == null)
        {
            Action refresh = () => Build(tb);
            tb.SetValue(HookedProperty, refresh);
            tb.Loaded += (_, _) => { QueryChanged -= refresh; QueryChanged += refresh; Build(tb); };
            tb.Unloaded += (_, _) => QueryChanged -= refresh;
            if (tb.IsLoaded) QueryChanged += refresh;
        }
        Build(tb);
    }

    static void Build(TextBlock tb)
    {
        var text = GetText(tb) ?? "";
        tb.Inlines.Clear();
        var ranges = _words.Length == 0 ? [] : Search.Ranges(text, _words);
        if (ranges.Count == 0) { tb.Inlines.Add(new Run(text)); return; }

        var mark = Application.Current.TryFindResource("Mark") as Brush;
        int pos = 0;
        foreach (var (start, length) in ranges)
        {
            if (start > pos) tb.Inlines.Add(new Run(text[pos..start]));
            tb.Inlines.Add(new Run(text.Substring(start, length)) { Background = mark, FontWeight = FontWeights.SemiBold });
            pos = start + length;
        }
        if (pos < text.Length) tb.Inlines.Add(new Run(text[pos..]));
    }
}
